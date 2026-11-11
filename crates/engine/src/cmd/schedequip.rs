//! Scheduled equipment the plans place (Q6313): the equipment a mechanical schedule lists
//! (`HEAT PUMP SCHEDULE`, `OUTDOOR AIR UNIT SCHEDULE`) and a plan of a level tags, built on that
//! level, a roof's on its curb or stand, each unit's refrigerant line set to the indoor unit its
//! schedule row pairs it with, and each electrical connection whose mark names a unit moved to
//! that unit (on its level, never in the room under it).
//!
//! The schedules are read off the mechanical sheets that aren't plans
//! ([`cadaddy_mepfix::equip_reader::read_tables`]); a row's tag is found on the mechanical plans
//! as its text (`OAU-2`), as its letters with its number printed by them (`HP` over `7`, a block
//! attribute or a text), and as each tag of a shared row (`FC/CU-2`: `FC-2` and `CU-2`). The
//! unit's footprint is the plan's outline within reach of its tag whose size agrees with the
//! schedule's size (else the usual size of its kind, rules data), never the tag's own bubble;
//! with no such outline a box of that size goes at the tag, said so. What it is by the documents
//! is on it (`STATUS` drawn / derived / inferred, `BASIS`): a curb or stand the documents don't
//! give is inferred from the trade practice rules (`M-ROOF-CURB`, `M-ROOF-STAND`), a line set
//! no plan routes from `M-REFRIG-LINESET`. Equipment already built (a typical room's stamped
//! unit, an exhaust fan) is never built twice. A scheduled tag no plan places is listed.
//!
//! A kind the schedule lists once for many units (a relief hood: one row, a hood over every
//! chase; rules data `equipment.every_tag_kinds`) is built at every place its tag is printed.

use super::printmep::SheetRef;
use cadaddy_doc::{Drawing, EntityKind};
use cadaddy_mepfix::equip_reader::{self, Scheduled};
use cadaddy_walk3d::{BuildingSpec, Made, MepEquipment, MepRun};
use serde_json::json;
use std::collections::{BTreeMap, BTreeSet};

/// Texts and outlines looked at on one plan, at most (hostile drawings).
const MAX_ITEMS: usize = 200_000;
/// Scheduled tags looked for, at most.
const MAX_SCHEDULED: usize = 2_000;
/// How far round its letters a tag's number may be printed, in text heights.
const NUMBER_REACH_H: f64 = 3.0;
/// How far from its tag a unit's outline may be: text heights, plus feet.
const OUTLINE_REACH_H: f64 = 6.0;
const OUTLINE_REACH_FT: f64 = 2.0;
/// An outline this many times the expected size either way (or less than its inverse) is not
/// the unit.
const SIZE_RATIO: f64 = 2.0;
/// An outline holding the tag and no bigger than this many text heights is the tag's bubble.
const BUBBLE_H: f64 = 8.0;
/// The system a line set is put on.
pub(crate) const REFRIGERANT: &str = "REFRIGERANT";

/// One plan of a level to look for tags on: its sheet and its objects.
pub(crate) struct PlanIn<'a> {
    pub sheet: &'a SheetRef,
    pub drawing: &'a Drawing,
    /// Drawing units per foot the sheet's objects are in (a sheet left at paper size: its
    /// printed scale's), and the job's (what the reading is scaled to).
    pub sheet_upf: f64,
    pub upf: f64,
}

/// What [`read`] found.
#[derive(Default)]
pub(crate) struct Read {
    /// Each unit (and a roof unit's curb or stand), on its sheet's place in the drawing (the
    /// build registers it onto its level's plan with the rest of the sheet's reading): filled
    /// by [`Read::choose`].
    pub equipment: Vec<MepEquipment>,
    /// Each scheduled tag the plans print, with every place they print it, best first: a place
    /// with a unit's outline by the tag before one without (a duct's unit label on a floor
    /// plan is no unit), then in plan order. Each place: the unit and its support.
    pub candidates: Vec<(String, Vec<Vec<MepEquipment>>)>,
    /// A kind built at every place its tag is printed (`equipment.every_tag_kinds`): each place's
    /// pieces, on its sheet's place in the drawing; [`Read::choose`] keeps those that fit.
    pub every: Vec<Vec<MepEquipment>>,
    /// Places of [`Read::every`] that fit no plan.
    pub every_not_placed: usize,
    /// Scheduled tags the plans print but no place of which fits a plan (with where).
    pub not_placeable: Vec<String>,
    /// Tables and scheduled rows read.
    pub tables: usize,
    pub scheduled: usize,
    /// Scheduled tags already built (left as they are).
    pub already: usize,
    /// Scheduled tags no plan places, with why.
    pub unplaced: Vec<(String, String)>,
    /// The schedules' rows (for [`type_built`]: the equipment built some other way that a row
    /// names).
    pub rows: Vec<Scheduled>,
    /// Equipment tags the plans print that no schedule row lists (letters a schedule uses, a
    /// number it doesn't), with the sheets.
    pub unscheduled: Vec<(String, String)>,
}

impl Read {
    /// Each tag's first place `fits` takes (it may move the pieces onto their level's plan; all
    /// of a place's pieces must fit), into [`Read::equipment`]; a tag none of whose places fits
    /// into [`Read::not_placeable`].
    pub(crate) fn choose(&mut self, mut fits: impl FnMut(&mut MepEquipment) -> bool) {
        for (tag, places) in std::mem::take(&mut self.candidates) {
            let mut sheets: Vec<String> = Vec::new();
            let mut done = false;
            for mut place in places {
                if place.iter_mut().all(&mut fits) {
                    self.equipment.extend(place);
                    done = true;
                    break;
                }
                if let Some(s) = place.first().and_then(|e| e.sources.get("sheet")) {
                    sheets.push(s.clone());
                }
            }
            if !done {
                self.not_placeable.push(format!("{tag} ({})", sheets.join(", ")));
            }
        }
        for mut place in std::mem::take(&mut self.every) {
            if place.iter_mut().all(&mut fits) {
                self.equipment.extend(place);
            } else {
                self.every_not_placed += 1;
            }
        }
    }

    pub(crate) fn report(&self) -> serde_json::Value {
        let mut by: BTreeMap<(String, String), usize> = BTreeMap::new();
        for e in &self.equipment {
            *by.entry((e.level.clone(), e.kind.clone())).or_default() += 1;
        }
        json!({
            "tables": self.tables,
            "scheduled": self.scheduled,
            "already_built": self.already,
            "built": by.iter().map(|((l, k), n)| json!({"level": l, "kind": k, "count": n})).collect::<Vec<_>>(),
            "unplaced": self.unplaced.iter().take(50).map(|(t, why)| json!({"tag": t, "why": why})).collect::<Vec<_>>(),
            "not_placeable": self.not_placeable.iter().take(50).collect::<Vec<_>>(),
            "every_tag_not_placed": self.every_not_placed,
        })
    }
}

/// The tags one schedule row names: its tag, and each tag of a row two units share
/// (`FC/CU-2` → `FC-2`, `CU-2`: the indoor and outdoor sections of one split system).
pub(crate) fn tag_variants(tag: &str) -> Vec<String> {
    let t = tag.trim().to_ascii_uppercase();
    if !t.contains('/') {
        return vec![t];
    }
    let parts: Vec<&str> = t.split('/').map(str::trim).filter(|p| !p.is_empty()).collect();
    let Some(last) = parts.last() else { return vec![t] };
    // The number the last part ends with (`-1`), which the parts with letters only share.
    let letters: String = last.chars().take_while(char::is_ascii_alphabetic).collect();
    let number = last.get(letters.len()..).unwrap_or("").to_string();
    let mut out: Vec<String> = parts
        .iter()
        .map(|p| if !number.is_empty() && p.chars().all(|c| c.is_ascii_alphabetic()) { format!("{p}{number}") } else { (*p).to_string() })
        .collect();
    out.dedup();
    out
}

/// A tag's letters and its number (`HP-7` → `HP`, `7`); none for a tag with no number.
fn split_tag(tag: &str) -> Option<(String, String)> {
    let letters: String = tag.chars().take_while(char::is_ascii_alphabetic).collect();
    let number = tag.get(letters.len()..)?.trim_start_matches(['-', ' ']).to_string();
    (!letters.is_empty() && !number.is_empty() && number.chars().all(|c| c.is_ascii_alphanumeric())).then_some((letters, number))
}

/// A text on a plan: its words, point and height.
pub(crate) struct Txt {
    pub value: String,
    pub at: [f64; 2],
    pub h: f64,
}

/// The plan's texts, block attributes too (a mark's tag is often its symbol's attribute).
pub(crate) fn plan_texts(d: &Drawing) -> Vec<Txt> {
    let mut out = Vec::new();
    let mut push = |v: &str, at: cadaddy_geom::Vec2, h: f64| {
        let v = v.trim();
        if !v.is_empty() && at.is_finite() && h.is_finite() && h > 0.0 && out.len() < MAX_ITEMS {
            out.push(Txt { value: v.to_ascii_uppercase(), at: [at.x, at.y], h });
        }
    };
    for e in d.model.iter().take(MAX_ITEMS) {
        match &e.kind {
            EntityKind::Text(t) => push(&t.value, t.insert.xy(), t.height),
            EntityKind::MText(t) => push(&t.contents, t.insert.xy(), t.height),
            EntityKind::Insert(ins) => {
                for a in ins.attribs.iter().take(16) {
                    push(&a.text.value, a.text.insert.xy(), a.text.height);
                }
            }
            _ => {}
        }
    }
    out
}

/// The plan's outlines: the boxes of its closed polylines and of open ones of four corners or
/// more (a PDF's rectangle often comes unclosed).
pub(crate) fn plan_outlines(d: &Drawing) -> Vec<([f64; 2], [f64; 2])> {
    let mut out = Vec::new();
    for e in d.model.iter().take(MAX_ITEMS) {
        let EntityKind::LwPolyline(pl) = &e.kind else { continue };
        if pl.vertices.len() < 3 || (!pl.closed && pl.vertices.len() < 4) {
            continue;
        }
        let b = pl.vertices.iter().fold([f64::INFINITY, f64::INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY], |b, v| {
            [b[0].min(v.p.x), b[1].min(v.p.y), b[2].max(v.p.x), b[3].max(v.p.y)]
        });
        if b.iter().all(|v| v.is_finite()) && b[2] > b[0] && b[3] > b[1] {
            out.push(([b[0], b[1]], [b[2], b[3]]));
        }
    }
    out
}

/// Where a tag is printed on a plan: its point (between its letters and number when they are
/// apart) and its text height. Its text whole first, else its letters with its number by them.
fn find_tag(texts: &[Txt], tag: &str) -> Option<([f64; 2], f64)> {
    find_tags(texts, tag).into_iter().next()
}

/// Every place a tag is printed ([`find_tag`]'s order: its text whole, then its letters with its
/// number by them).
pub(crate) fn find_tags(texts: &[Txt], tag: &str) -> Vec<([f64; 2], f64)> {
    let mut out: Vec<([f64; 2], f64)> = texts.iter().filter(|t| t.value == tag).map(|t| (t.at, t.h)).collect();
    let Some((letters, number)) = split_tag(tag) else { return out };
    out.extend(texts.iter().filter(|t| t.value == letters).filter_map(|t| {
        let reach = NUMBER_REACH_H * t.h;
        let n = texts
            .iter()
            .filter(|n| n.value == number)
            .map(|n| (((n.at[0] - t.at[0]).hypot(n.at[1] - t.at[1])), n))
            .filter(|(d, _)| *d <= reach)
            .min_by(|a, b| a.0.total_cmp(&b.0))?
            .1;
        // The number nearest these letters, not another tag's letters.
        let nearer_other = texts
            .iter()
            .filter(|o| o.value == letters && !std::ptr::eq(*o, t))
            .any(|o| (o.at[0] - n.at[0]).hypot(o.at[1] - n.at[1]) < (t.at[0] - n.at[0]).hypot(t.at[1] - n.at[1]));
        (!nearer_other).then_some(([(t.at[0] + n.at[0]) / 2.0, (t.at[1] + n.at[1]) / 2.0], t.h))
    }));
    out
}

/// How far an outline's size is from `want` (feet, plan, either way round): the larger log
/// ratio of its sides; none past [`SIZE_RATIO`].
fn size_error(w: f64, h: f64, want: [f64; 2]) -> Option<f64> {
    if !(w > 0.0 && h > 0.0 && want[0] > 0.0 && want[1] > 0.0) {
        return None;
    }
    let err = |a: f64, b: f64| (a / b).ln().abs();
    let e = err(w, want[0]).max(err(h, want[1])).min(err(w, want[1]).max(err(h, want[0])));
    (e.is_finite() && e <= SIZE_RATIO.ln()).then_some(e)
}

fn box_dist(p: [f64; 2], lo: [f64; 2], hi: [f64; 2]) -> f64 {
    let dx = (lo[0] - p[0]).max(0.0).max(p[0] - hi[0]);
    let dy = (lo[1] - p[1]).max(0.0).max(p[1] - hi[1]);
    dx.hypot(dy)
}

/// Whether a level is a roof (`Roof`, `R`, `HIGH ROOF`).
pub(crate) fn is_roof(level: &str) -> bool {
    let l = level.trim();
    l.eq_ignore_ascii_case("R") || cadaddy_walk3d::levels::canonical_level(l).is_some_and(|(_, k)| k == cadaddy_walk3d::levels::DatumKind::Roof)
}

/// The trade practice rule a unit's mounting comes from when the documents give none (Q6494).
pub(crate) const MOUNT_RULE: &str = "M-EQUIP-MOUNT";
/// The ceiling a ceiling-hung unit goes to when its level has none, feet.
const CEILING_FT: f64 = 8.0;

/// One mounting row of [`MOUNT_RULE`]: the kinds (and schedule words) it is for and where they go.
#[derive(Clone, Debug, Default, serde::Deserialize)]
#[serde(default)]
struct MountRow {
    kinds: Vec<String>,
    words: Vec<String>,
    /// `above_ceiling` (bottom `gap_in` over the ceiling) or `floor` (bottom `bottom_in` over it).
    at: String,
    gap_in: f64,
    bottom_in: f64,
    height_in: f64,
    basis: String,
}

/// Where a unit sits by [`mounting`]: its bottom over its level's floor, its height when the
/// row gives one, and why (the rule, its row's reason and the numbers).
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Mount {
    pub bottom_ft: f64,
    pub height_ft: Option<f64>,
    pub basis: String,
}

/// Where a unit of `kind` goes when no schedule, submittal or plan note gives its height: the
/// first row of [`MOUNT_RULE`] naming its kind whose words (if it has any) `words` has (its
/// schedule's title, type and mounting words), with its level's (or room's) ceiling and its
/// printed height (feet, when the schedule gives one). None when no row names it: it stands on
/// the floor.
pub(crate) fn mounting(kind: &str, words: &str, ceiling_ft: f64, printed_ft: Option<f64>) -> Option<Mount> {
    let row = mount_row(kind, words)?;
    let inches = |v: f64| (v.is_finite() && v > 0.0).then_some(v / 12.0);
    let height_ft = printed_ft.filter(|h| h.is_finite() && *h > 0.0).or_else(|| inches(row.height_in));
    let ceiling = if ceiling_ft.is_finite() && ceiling_ft > 0.0 { ceiling_ft } else { CEILING_FT };
    let (bottom_ft, said) = match row.at.as_str() {
        "above_ceiling" => {
            let b = ceiling + inches(row.gap_in).unwrap_or(0.0);
            (b, format!("bottom {b:.2} ft, in the {ceiling:.2} ft ceiling"))
        }
        "floor" => match inches(row.bottom_in) {
            Some(b) => {
                // Raised off the floor, its top at the ceiling at most.
                let b = height_ft.map_or(b, |h| b.min(ceiling - h)).max(0.0);
                (b, format!("bottom {b:.2} ft over the floor"))
            }
            None => (0.0, "on the floor".to_string()),
        },
        _ => return None,
    };
    let rule = super::provenance::rule(MOUNT_RULE);
    let basis = format!("{}: trade practice: {} ({kind}: {said})", rule.id, row.basis);
    Some(Mount { bottom_ft, height_ft, basis })
}

/// The first row of [`MOUNT_RULE`] naming `kind` whose words (if it has any) `words` has.
fn mount_row(kind: &str, words: &str) -> Option<MountRow> {
    let data = super::provenance::rule_data(MOUNT_RULE);
    let rows: Vec<MountRow> = data.get("mountings").and_then(|m| serde_json::from_value(m.clone()).ok()).unwrap_or_default();
    let spaced =
        |s: &str| format!(" {} ", s.to_ascii_uppercase().split(|c: char| !c.is_ascii_alphanumeric() && c != '-').collect::<Vec<_>>().join(" "));
    let up = spaced(words);
    rows.into_iter().take(200).find(|r| {
        r.kinds.iter().any(|k| k.eq_ignore_ascii_case(kind))
            && (r.words.is_empty() || r.words.iter().any(|w| !w.trim().is_empty() && up.contains(&spaced(w.trim()))))
    })
}

/// Whether a unit of `kind` (its schedule's or title's `words`) hangs above the ceiling by
/// [`MOUNT_RULE`] (a ceiling exhaust fan's housing, a damper in its duct), the same row its
/// mounting comes from.
pub(crate) fn hangs_above_ceiling(kind: &str, words: &str) -> bool {
    mount_row(kind, words).is_some_and(|r| r.at == "above_ceiling")
}

/// The schedule columns whose words say how a unit is mounted (`TYPE: CEILING`, `MOUNTING: WALL`).
const MOUNT_COLUMNS: [&str; 8] = ["TYPE", "DESCRIPTION", "MOUNTING", "MOUNT", "ARRANGEMENT", "CONFIGURATION", "STYLE", "LOCATION"];

/// The words a unit's mounting row is matched with: its kind, its schedule's title and its row's
/// type and mounting words.
fn mount_words(x: &Scheduled) -> String {
    let said: Vec<&str> = x
        .properties
        .iter()
        .filter(|(c, _)| {
            let c = c.to_ascii_uppercase();
            MOUNT_COLUMNS.iter().any(|m| c.split(|ch: char| !ch.is_ascii_alphanumeric()).any(|w| w == *m))
        })
        .map(|(_, v)| v.as_str())
        .take(16)
        .collect();
    format!("{} {}", words_of(x), said.join(" "))
}

/// The bottom over the floor (feet) a schedule row prints for its unit, with its column: a
/// mounting height or AFF column (`MTG HT`, `MOUNTING HEIGHT`, `BOTTOM AFF`: `8'-0"`), or a
/// mounting column giving a height AFF (`MOUNTING`: `WALL @ 7'-0" AFF`). A remark's height is
/// some other part's (`THERMOSTAT AT 48" AFF`), never the unit's.
pub(crate) fn schedule_bottom_ft(x: &Scheduled) -> Option<(f64, String)> {
    let height_column = |c: &str| ["AFF", "A.F.F", "MOUNTING HEIGHT", "MOUNTING HT", "MTG HT", "MTG. HT", "MTG HEIGHT"].iter().any(|w| c.contains(w));
    x.properties.iter().take(64).find_map(|(c, v)| {
        let c = c.to_ascii_uppercase();
        let v = v.to_ascii_uppercase().replace("A.F.F.", "AFF");
        if !(height_column(&c) || (c.split(|ch: char| !ch.is_ascii_alphanumeric()).any(|w| w == "MOUNTING") && v.contains("AFF"))) {
            return None;
        }
        // The length just before its `AFF` (or the cell's last): `7'-0"`, `84"`, `8'`.
        let toks: Vec<&str> = v.split("AFF").next().unwrap_or("").split_whitespace().collect();
        let last = toks.last()?.trim_matches([',', ';', '@', '(', ')']);
        let f = equip_reader::parse_length_ft(last, c.contains("(IN") || c.contains("INCH")).filter(|f| *f < 60.0)?;
        Some((f, c.trim().to_string()))
    })
}

/// The kind words a rule's size rows are matched with: the kind and its schedule's title.
fn words_of(x: &Scheduled) -> String {
    format!("{} {}", x.kind.replace('_', " "), x.title).to_ascii_uppercase()
}

/// The rule row a unit's words fall under, if any (a stand's kinds).
fn rule_names(rule: &super::provenance::PracticeRule, words: &str) -> bool {
    let up = format!(" {} ", words.to_ascii_uppercase());
    rule.sizes
        .iter()
        .any(|r| r.words.iter().any(|w| up.contains(&format!(" {} ", w.trim().to_ascii_uppercase())) || up.contains(&w.trim().to_ascii_uppercase())))
}

/// What a roof unit stands on: (`roof_curb` or `equipment_stand`, height in feet, made, basis).
/// A plan note naming the unit and a curb (`SET OAU-2 ON ITS ROOF CURB`) says it is a curb; the
/// height is the trade practice rule's unless the documents give one.
fn roof_support(x: &Scheduled, tag: &str, notes: &[Txt]) -> (&'static str, f64, Made, String) {
    let noted = notes.iter().find(|t| t.value.contains("CURB") && t.value.split(|c: char| !c.is_ascii_alphanumeric() && c != '-').any(|w| w == tag));
    let stand = super::provenance::rule("M-ROOF-STAND");
    let curb = super::provenance::rule("M-ROOF-CURB");
    let words = words_of(x);
    let on_stand = noted.is_none() && rule_names(&stand, &words);
    let (kind, rule) = if on_stand { ("equipment_stand", stand) } else { ("roof_curb", curb) };
    let inches = if rule.sizes.is_empty() { rule.default_size_in } else { rule.size_for(&words) };
    let inches = if inches.is_finite() && inches > 0.0 { inches } else { 14.0 };
    match noted {
        Some(n) => {
            (kind, inches / 12.0, Made::Derived, format!("the plan's note: {}; its height {}", n.value, rule.basis_for(&format!("{inches} in"))))
        }
        None => (kind, inches / 12.0, Made::Inferred, rule.basis_for(&format!("{inches} in"))),
    }
}

/// The scheduled equipment the plans tag, built where they tag it (see the module doc).
/// `schedules`: the mechanical sheets that aren't plans (number, objects); `plans`: the
/// mechanical plans of the levels; `have`: what is built already; `levels`: the spec's.
pub(crate) fn read(schedules: &[(String, Drawing)], plans: &[PlanIn], have: &[MepEquipment], levels: &[cadaddy_walk3d::Level]) -> Read {
    let mut tables = Vec::new();
    for (number, page) in schedules {
        tables.extend(equip_reader::read_tables(page, number));
    }
    let sched: Vec<Scheduled> = equip_reader::scheduled(&tables).into_iter().take(MAX_SCHEDULED).collect();
    let mut out = place(&sched, plans, have, levels);
    out.tables = tables.len();
    out.unscheduled = unscheduled_tags(&sched, plans);
    out.rows = sched;
    out
}

/// Most unscheduled tags listed.
const MAX_UNSCHEDULED: usize = 200;

/// The equipment tags the plans print (`EF-9`: letters a schedule's tags use, then a number)
/// that no schedule row lists, each with the sheets printing it: drawn equipment with no row
/// to say what it is.
fn unscheduled_tags(sched: &[Scheduled], plans: &[PlanIn]) -> Vec<(String, String)> {
    let head = |t: &str| -> Option<(String, String)> {
        let t = t.trim().to_ascii_uppercase();
        let i = t.char_indices().rev().take_while(|(_, c)| c.is_ascii_digit()).last().map(|(i, _)| i)?;
        let (h, n) = t.split_at(i);
        let letters = h.trim_end_matches('-');
        (letters.len() >= 2 && letters.len() <= 5 && letters.chars().all(|c| c.is_ascii_alphabetic()) && n.len() <= 3)
            .then(|| (letters.to_string(), t.clone()))
    };
    let listed: BTreeSet<String> = sched.iter().flat_map(|x| tag_variants(&x.tag)).collect();
    // The letters the schedules' tags use, and how (`EF-1`, `EF1`).
    let letters: BTreeSet<String> = listed.iter().filter_map(|t| head(t).map(|(l, _)| l)).collect();
    let mut out: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for plan in plans {
        for t in plan_texts(plan.drawing) {
            let Some((l, tag)) = head(&t.value) else { continue };
            if !letters.contains(&l) || listed.contains(&tag) || t.value.trim().len() != tag.len() {
                continue;
            }
            // Written as the schedules write theirs (`EF-9` beside `EF-1`).
            if !listed.iter().any(|x| head(x).is_some_and(|(xl, _)| xl == l) && x.contains('-') == tag.contains('-')) {
                continue;
            }
            if out.len() < MAX_UNSCHEDULED || out.contains_key(&tag) {
                out.entry(tag).or_default().insert(plan.sheet.number.clone());
            }
        }
    }
    out.into_iter().map(|(t, s)| (t, s.into_iter().collect::<Vec<_>>().join(", "))).collect()
}

/// How far past a unit's footprint a drawn run's end may stop and still be joined to it, feet.
const JOIN_REACH_FT: f64 = 1.5;

/// Each unit built from a schedule (`equipment[from..]`) joined to the drawn runs on its level
/// that end at it (a water heater's hot and cold water and gas, within [`JOIN_REACH_FT`] of its
/// footprint): their systems on it, said so. How many units got one.
pub(crate) fn join_runs(sp: &mut BuildingSpec, from: usize) -> usize {
    let upf = if sp.units_per_foot.is_finite() && sp.units_per_foot > 0.0 { sp.units_per_foot } else { 12.0 };
    let reach = JOIN_REACH_FT * upf;
    let runs = &sp.mep.runs;
    let mut joined = 0usize;
    for e in sp.mep.equipment.iter_mut().skip(from) {
        if !e.sources.contains_key("schedule") || e.kind == "roof_curb" || e.kind == "equipment_stand" {
            continue;
        }
        let near = |p: &[f64; 2]| p[0] >= e.min[0] - reach && p[0] <= e.max[0] + reach && p[1] >= e.min[1] - reach && p[1] <= e.max[1] + reach;
        let mut systems: BTreeSet<String> = BTreeSet::new();
        for r in runs.iter().filter(|r| r.level == e.level).take(MAX_ITEMS) {
            let ends = [r.points.first(), r.points.last()];
            if ends.iter().flatten().any(|p| near(p)) && !r.system.trim().is_empty() {
                systems.insert(r.system.clone());
            }
        }
        if systems.is_empty() {
            continue;
        }
        let new: Vec<String> = systems.into_iter().filter(|s| !e.systems.contains(s)).collect();
        if new.is_empty() {
            continue;
        }
        e.sources.insert("systems".to_string(), format!("the drawn runs ending at it ({})", new.join(", ")));
        e.systems.extend(new);
        joined += 1;
    }
    joined
}

/// What [`type_built`] did.
#[derive(Default, Debug, Clone, PartialEq)]
pub(crate) struct Typed {
    /// Equipment whose own tag a row lists, its row's data put on it.
    pub by_tag: usize,
    /// Equipment tagged only with its kind's letters (a typical room's `EF`) given the one row
    /// of that kind that serves its room (or the only row of its kind).
    pub by_room: usize,
    /// Equipment tagged only with its kind's letters that no one row could be told for, with
    /// the rows it could be.
    pub untold: Vec<(String, String)>,
}

/// A row's own data put on equipment already built some other way (a typical room's stamped
/// fan): every column it doesn't have yet, sourced to the schedule.
fn put_row(e: &mut MepEquipment, x: &Scheduled) {
    for (k, v) in &x.properties {
        e.properties.entry(k.clone()).or_insert_with(|| v.clone());
    }
    e.sources.entry("schedule".to_string()).or_insert_with(|| format!("{} {}", x.sheet, x.title).trim().to_string());
}

/// The words of a text, upper case, a plural's `S` off (`BATHROOMS` → `BATHROOM`).
fn stem_words(t: &str) -> BTreeSet<String> {
    t.to_ascii_uppercase()
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|w| w.len() >= 3 && !w.chars().all(|c| c.is_ascii_digit()))
        .map(|w| if w.len() > 3 && w.ends_with('S') && !w.ends_with("SS") { w.trim_end_matches('S').to_string() } else { w.to_string() })
        .collect()
}

/// A row that serves many rooms: its `SERVES` in the plural (`GUEST TOILET ROOMS`) or said typical.
fn serves_many(serves: &str) -> bool {
    let u = serves.to_ascii_uppercase();
    let last = u.split(|c: char| !c.is_ascii_alphanumeric()).rfind(|w| !w.is_empty()).unwrap_or("");
    u.contains("TYP") || (last.len() > 3 && last.ends_with('S') && !last.ends_with("SS"))
}

/// The schedules' rows put on the equipment built some other way (a typical room's stamped
/// unit, an exhaust fan the plans draw): equipment whose tag a row lists takes that row's data;
/// equipment tagged only with its kind's letters (a typical room's `EF`) takes the one row of
/// its kind that serves its room: the row whose `SERVES` shares the most words with the room's
/// name and the view it came from, a row serving many rooms (plural, typical) before one that
/// serves one; the only row of its kind; else none, listed. Its tag becomes that row's, its
/// type said derived with the reason. `room_name` gives a room's name from its level and number.
pub(crate) fn type_built(rows: &[Scheduled], equipment: &mut [MepEquipment], room_name: &dyn Fn(&str, &str) -> String) -> Typed {
    let mut out = Typed::default();
    let by_tag: BTreeMap<String, &Scheduled> = rows.iter().flat_map(|x| tag_variants(&x.tag).into_iter().map(move |t| (t, x))).collect();
    for e in equipment.iter_mut() {
        // Built from its row already.
        if e.sources.contains_key("schedule") {
            continue;
        }
        let tag = e.tag.trim().to_ascii_uppercase();
        if tag.is_empty() {
            continue;
        }
        if let Some(x) = by_tag.get(&tag) {
            put_row(e, x);
            out.by_tag += 1;
            continue;
        }
        // Only letters: the kind's mark, no type.
        if !tag.chars().all(|c| c.is_ascii_alphabetic()) {
            continue;
        }
        let letters = |t: &str| -> String { t.chars().take_while(|c| c.is_ascii_alphabetic()).collect() };
        let same: Vec<&Scheduled> = rows.iter().filter(|x| x.kind == e.kind && letters(&x.tag) == tag).collect();
        if same.is_empty() {
            continue;
        }
        let room = e.properties.get("room").cloned().unwrap_or_default();
        // The room's name and its view's title (less the words naming the drawing), less the
        // words that say nothing about the room.
        let stem = |w: &str| stem_words(w).into_iter().next().unwrap_or_default();
        let generic: BTreeSet<String> = cadaddy_mepfix::wordrules::words("equipment.serves_generic_words").iter().map(|w| stem(w)).collect();
        let drafting: BTreeSet<String> = cadaddy_mepfix::wordrules::words("equipment.view_title_drafting_words").iter().map(|w| stem(w)).collect();
        let mut words = stem_words(&room_name(&e.level, &room));
        words.extend(stem_words(&e.title).into_iter().filter(|w| !drafting.contains(w)));
        words.retain(|w| !generic.contains(w));
        // What a row serves: its `SERVES` / `AREA SERVED` column, else its `SERVICE`.
        let serves = |x: &Scheduled| {
            let col = |w: &str| x.properties.iter().find(|(k, _)| k.to_ascii_uppercase().contains(w)).map(|(_, v)| v.clone());
            col("SERVE").or_else(|| col("SERVICE"))
        };
        let score = |x: &Scheduled| {
            let s = serves(x).unwrap_or_default();
            let shared = stem_words(&s).intersection(&words).count();
            (shared, usize::from(serves_many(&s)))
        };
        let mut ranked: Vec<((usize, usize), &Scheduled)> = same.iter().map(|x| (score(x), *x)).collect();
        ranked.sort_by(|a, b| b.0.cmp(&a.0));
        let pick = match ranked.as_slice() {
            [only] => Some((only.1, "the only row of its kind".to_string())),
            [first, second, ..] if first.0 > second.0 && (first.0.0 > 0 || first.0.1 > 0) => {
                let s = serves(first.1).unwrap_or_default();
                let why = if first.0.0 > 0 && first.0.0 > second.0.0 {
                    format!("its row serves {s}, the room's use")
                } else {
                    format!("its row serves {s}, the one row of its kind for many rooms")
                };
                Some((first.1, why))
            }
            _ => None,
        };
        match pick {
            Some((x, why)) => {
                let new_tag = tag_variants(&x.tag).into_iter().next().unwrap_or_else(|| x.tag.clone());
                put_row(e, x);
                e.properties.insert("TYPE".to_string(), new_tag.clone());
                let basis = format!("drawn tagged {tag} with no number; {} {} lists {new_tag}: {why}", x.sheet, x.title);
                e.sources.insert("type".to_string(), basis.trim().to_string());
                e.tag = new_tag;
                out.by_room += 1;
            }
            None => {
                if out.untold.len() < MAX_UNSCHEDULED {
                    let can: Vec<String> = same.iter().map(|x| x.tag.clone()).collect();
                    out.untold.push((format!("{tag} ({} {room})", e.level), can.join(", ")));
                }
            }
        }
    }
    out
}

/// [`read`] for rows already read off their schedules.
#[cfg(test)]
pub(crate) fn read_scheduled(sched: &[Scheduled], plans: &[PlanIn], have: &[MepEquipment], levels: &[cadaddy_walk3d::Level]) -> Read {
    place(sched, plans, have, levels)
}

/// [`read`] once the schedules are read: each scheduled row's tags found on the plans and built.
fn place(sched: &[Scheduled], plans: &[PlanIn], have: &[MepEquipment], levels: &[cadaddy_walk3d::Level]) -> Read {
    let mut out = Read { scheduled: sched.len(), ..Read::default() };
    let built: BTreeSet<String> = have.iter().map(|e| e.tag.trim().to_ascii_uppercase()).collect();
    let all_tags: BTreeSet<String> = sched.iter().flat_map(|x| tag_variants(&x.tag)).chain(built.iter().cloned()).collect();
    let every_kinds = cadaddy_mepfix::wordrules::words("equipment.every_tag_kinds");
    let every = |x: &Scheduled| every_kinds.iter().any(|k| x.kind.eq_ignore_ascii_case(k));
    // (tag, its row) still to place.
    let mut todo: Vec<(String, &Scheduled)> = Vec::new();
    for x in sched {
        for t in tag_variants(&x.tag) {
            if built.contains(&t) {
                out.already += 1;
            } else if !todo.iter().any(|(o, _)| *o == t) {
                todo.push((t, x));
            }
        }
    }
    // Each todo tag's places: (has the unit's outline, plan order, its pieces).
    let mut places: BTreeMap<usize, Vec<(bool, usize, Vec<MepEquipment>)>> = BTreeMap::new();
    // The kinds built at every tag that some plan prints.
    let mut typed_found: BTreeSet<usize> = BTreeSet::new();
    for (plan_i, plan) in plans.iter().enumerate() {
        let texts = plan_texts(plan.drawing);
        let outlines = plan_outlines(plan.drawing);
        let su = if plan.sheet_upf.is_finite() && plan.sheet_upf > 0.0 { plan.sheet_upf } else { 12.0 };
        let k = if plan.upf.is_finite() && plan.upf > 0.0 { plan.upf / su } else { 1.0 };
        let corner = plan.sheet.view.unwrap_or(plan.sheet.rect);
        let to_drawing = |p: [f64; 2]| [corner[0] + (p[0] - corner[0]) * k, corner[1] + (p[1] - corner[1]) * k];
        let level = super::printmep::level_name(&plan.sheet.level);
        let lv = levels.iter().find(|l| l.name.eq_ignore_ascii_case(&level));
        let roof = is_roof(&plan.sheet.level) || is_roof(&level);
        // Each tag found here: (todo index, its point, its text height); a kind built at every
        // tag, each place.
        let found: Vec<(usize, [f64; 2], f64)> = todo
            .iter()
            .enumerate()
            .flat_map(|(i, (t, x))| {
                let at: Vec<([f64; 2], f64)> = if every(x) { find_tags(&texts, t) } else { find_tag(&texts, t).into_iter().collect() };
                at.into_iter().map(move |(at, h)| (i, at, h))
            })
            .collect();
        // Candidate outlines, scored: nearness to the tag (as a share of the reach) plus how far
        // its size is from the expected one; the best pairs first, each outline and tag once.
        let mut cands: Vec<(f64, usize, usize)> = Vec::new();
        for (fi, (ti, at, h)) in found.iter().enumerate() {
            let Some((_, x)) = todo.get(*ti) else { continue };
            let want = x.size_ft.filter(|s| s[0] > 0.0 && s[1] > 0.0).unwrap_or_else(|| equip_reader::rule_size(&x.kind));
            let reach = OUTLINE_REACH_H * h + OUTLINE_REACH_FT * su;
            for (oi, (lo, hi)) in outlines.iter().enumerate() {
                let d = box_dist(*at, *lo, *hi);
                if d > reach {
                    continue;
                }
                let (w, ht) = (hi[0] - lo[0], hi[1] - lo[1]);
                // The tag's own bubble: holds the tag, a few text heights across.
                if d <= 0.0 && w.max(ht) <= BUBBLE_H * h {
                    continue;
                }
                if let Some(e) = size_error(w / su, ht / su, [want[0], want[1]]) {
                    cands.push((d / reach.max(1e-9) + e, fi, oi));
                }
            }
        }
        cands.sort_by(|a, b| a.0.total_cmp(&b.0));
        let mut outline_of: BTreeMap<usize, usize> = BTreeMap::new();
        let mut taken: BTreeSet<usize> = BTreeSet::new();
        let overlaps = |a: usize, b: usize| match (outlines.get(a), outlines.get(b)) {
            (Some((alo, ahi)), Some((blo, bhi))) => alo[0] < bhi[0] && blo[0] < ahi[0] && alo[1] < bhi[1] && blo[1] < ahi[1],
            _ => false,
        };
        for (_, fi, oi) in &cands {
            // One outline per unit, and no two units in one place.
            if outline_of.contains_key(fi) || taken.contains(oi) || taken.iter().any(|t| overlaps(*t, *oi)) {
                continue;
            }
            outline_of.insert(*fi, *oi);
            taken.insert(*oi);
        }
        for (fi, (ti, at, _)) in found.iter().enumerate() {
            let Some((tag, x)) = todo.get(*ti) else { continue };
            let size = x.size_ft.filter(|s| s[0] > 0.0 && s[1] > 0.0);
            let rule = equip_reader::rule_size(&x.kind);
            let outlined = outline_of.get(&fi).and_then(|oi| outlines.get(*oi));
            let mut pieces: Vec<MepEquipment> = Vec::new();
            let (lo, hi, footprint, made) = match outlined {
                Some((lo, hi)) => {
                    (*lo, *hi, "outline: the plan's outline by its tag, its size the schedule's (or its kind's usual)".to_string(), Made::Drawn)
                }
                None => {
                    let s = size.unwrap_or(rule);
                    let (w, d) = (s[0] * su / 2.0, s[1] * su / 2.0);
                    let from = if size.is_some() { "the schedule's size" } else { "the usual size of its kind (rules data)" };
                    (
                        [at[0] - w, at[1] - d],
                        [at[0] + w, at[1] + d],
                        format!("no outline by its tag: {from} at its tag, which way it faces a guess"),
                        if size.is_some() { Made::Derived } else { Made::Inferred },
                    )
                }
            };
            let printed = size.map(|s| s[2]).filter(|h| *h > 0.0);
            let (mut h_ft, mut height_from) = match printed {
                Some(h) => (h, "schedule".to_string()),
                None => (rule[2], "rule: the usual height of its kind".to_string()),
            };
            let hung = equip_reader::hangs(x);
            let ceiling = lv.map_or(CEILING_FT, |l| l.ceiling_ft);
            // Where it sits off a roof: the schedule's mounting height, else its kind's mounting
            // (M-EQUIP-MOUNT), else on the floor (Q6494).
            let mut elevation: Option<(f64, String)> = None;
            if !hung && !roof {
                if let Some((b, column)) = schedule_bottom_ft(x) {
                    elevation = Some((b, format!("schedule: its {column} {b:.2} ft")));
                } else if let Some(m) = mounting(&x.kind, &mount_words(x), ceiling, printed) {
                    if printed.is_none()
                        && let Some(h) = m.height_ft
                    {
                        h_ft = h;
                        height_from = m.basis.clone();
                    }
                    elevation = Some((m.bottom_ft, m.basis));
                }
            }
            let (a, b) = (to_drawing(lo), to_drawing(hi));
            let (min, max) = ([a[0].min(b[0]), a[1].min(b[1])], [a[0].max(b[0]), a[1].max(b[1])]);
            let mut sources = BTreeMap::new();
            sources.insert("sheet".to_string(), plan.sheet.number.clone());
            sources.insert("schedule".to_string(), format!("{} {}", x.sheet, x.title).trim().to_string());
            sources.insert("position".to_string(), format!("its tag {tag} on {}", plan.sheet.number));
            sources.insert("outline".to_string(), footprint.clone());
            sources.insert("height".to_string(), height_from);
            if let Some((_, how)) = &elevation {
                sources.insert("elevation".to_string(), how.clone());
            }
            let mut properties: BTreeMap<String, String> = x.properties.clone();
            properties.insert("STATUS".to_string(), made.as_str().to_string());
            if !made.is_drawn() {
                properties.insert("BASIS".to_string(), format!("{} {} lists {tag}; {footprint}", x.sheet, x.title).trim().to_string());
            }
            // The other unit its row names (a heat pump's fan coil unit): what it serves.
            // A shared row's other unit (`FC/CU-2`: the outdoor unit serves the indoor one), else
            // another unit its row names.
            let variants = tag_variants(&x.tag);
            let other = variants.iter().find(|v| *v != tag).cloned();
            let named =
                || x.properties.values().map(|v| v.trim().to_ascii_uppercase()).find(|v| *v != *tag && !variants.contains(v) && all_tags.contains(v));
            if let Some(o) = other {
                properties.insert("PAIRED".to_string(), o);
            } else if let Some(serves) = named() {
                properties.insert("SERVES".to_string(), serves);
            }
            let mut bottom = if hung { ceiling } else { elevation.as_ref().map_or(0.0, |(b, _)| *b) };
            if roof && !hung {
                let (part, ft, smade, basis) = roof_support(x, tag, &texts);
                bottom = ft;
                properties.insert("SUPPORT".to_string(), format!("{} {ft:.2} ft", part.replace('_', " ")));
                let mut sp = BTreeMap::new();
                sp.insert("STATUS".to_string(), smade.as_str().to_string());
                sp.insert("BASIS".to_string(), basis.clone());
                sp.insert("UNIT".to_string(), tag.clone());
                let mut ss = BTreeMap::new();
                ss.insert("sheet".to_string(), plan.sheet.number.clone());
                ss.insert("height".to_string(), basis);
                pieces.push(MepEquipment {
                    tag: format!("{tag} {}", if part == "roof_curb" { "CURB" } else { "STAND" }),
                    kind: part.to_string(),
                    level: level.clone(),
                    min,
                    max,
                    bottom_ft: 0.0,
                    height_ft: ft,
                    title: x.title.clone(),
                    properties: sp,
                    sources: ss,
                    confidence: 0.5,
                    ..Default::default()
                });
            }
            pieces.push(MepEquipment {
                tag: tag.clone(),
                kind: x.kind.clone(),
                level: level.clone(),
                min,
                max,
                bottom_ft: bottom,
                height_ft: h_ft,
                round: x.round,
                title: x.title.clone(),
                properties,
                sources,
                confidence: if made.is_drawn() { 0.8 } else { 0.5 },
                ..Default::default()
            });
            if every(x) {
                out.every.push(pieces);
                typed_found.insert(*ti);
            } else {
                places.entry(*ti).or_default().push((outlined.is_some(), plan_i, pieces));
            }
        }
    }
    for (ti, (tag, _)) in todo.iter().enumerate() {
        if let Some(mut p) = places.remove(&ti) {
            p.sort_by_key(|(outlined, plan_i, _)| (!*outlined, *plan_i));
            out.candidates.push((tag.clone(), p.into_iter().map(|(_, _, pieces)| pieces).collect()));
        }
    }
    let placed: BTreeSet<&String> = out.candidates.iter().map(|(t, _)| t).collect();
    out.unplaced = todo
        .iter()
        .enumerate()
        .filter(|(i, (t, _))| !placed.contains(t) && !typed_found.contains(i))
        .map(|(_, tx)| tx)
        .map(|(t, x)| (t.clone(), format!("{} {} lists it; no mechanical or plumbing plan of a level tags it", x.sheet, x.title).trim().to_string()))
        .collect();
    out
}

/// A level's own value of `get` (its plan's base point, its plan region's corner), else (a
/// roof with no plan of its own, on the plan of the storey under it) the nearest one's below.
fn own_or_below(sp: &BuildingSpec, level: &str, get: &dyn Fn(&cadaddy_walk3d::Level) -> Option<[f64; 2]>) -> Option<[f64; 2]> {
    let l = sp.level(level)?;
    if let Some(v) = get(l) {
        return Some(v);
    }
    sp.levels
        .iter()
        .filter(|o| o.elevation_ft < l.elevation_ft && get(o).is_some())
        .max_by(|a, b| a.elevation_ft.total_cmp(&b.elevation_ft))
        .and_then(get)
}

/// Two levels' plan origins, measured the same way: their plans' base points when both have
/// one, else their plan regions' corners (each level's plan is drawn in its own place).
pub(crate) fn origins(sp: &BuildingSpec, a: &str, b: &str) -> Option<([f64; 2], [f64; 2])> {
    let base = |l: &cadaddy_walk3d::Level| l.base.filter(|p| p.iter().all(|v| v.is_finite()));
    let corner = |l: &cadaddy_walk3d::Level| l.region.filter(|r| r.iter().all(|v| v.is_finite())).map(|r| [r[0].min(r[2]), r[1].min(r[3])]);
    match (own_or_below(sp, a, &base), own_or_below(sp, b, &base)) {
        (Some(x), Some(y)) => Some((x, y)),
        _ => Some((own_or_below(sp, a, &corner)?, own_or_below(sp, b, &corner)?)),
    }
}

fn centre(e: &MepEquipment) -> [f64; 2] {
    [(e.min[0] + e.max[0]) / 2.0, (e.min[1] + e.max[1]) / 2.0]
}

/// Each built roof unit's refrigerant line set to the unit its schedule row pairs it with
/// (`SERVES`, or the other unit of a shared row: `PAIRED`), when that unit is built: over the roof from the unit to above it, then down to it
/// (`M-REFRIG-LINESET`, inferred). Units on the plans already (`have`): none twice. How many.
pub(crate) fn line_sets(sp: &mut BuildingSpec) -> usize {
    let rule = super::provenance::rule("M-REFRIG-LINESET");
    let size = if rule.default_size_in.is_finite() && rule.default_size_in > 0.0 { rule.default_size_in } else { 0.875 };
    let mut runs: Vec<MepRun> = Vec::new();
    for e in sp.mep.equipment.iter().filter(|e| is_roof(&e.level)) {
        let Some(serves) = e.properties.get("SERVES").or_else(|| e.properties.get("PAIRED")) else { continue };
        let Some(to) =
            sp.mep.equipment.iter().find(|o| o.tag.eq_ignore_ascii_case(serves) && !matches!(o.kind.as_str(), "roof_curb" | "equipment_stand"))
        else {
            continue;
        };
        if sp.mep.runs.iter().any(|r| r.system == REFRIGERANT && r.id == format!("{}>{}", e.tag, to.tag)) {
            continue;
        }
        let (Some(roof), Some(below), Some((b_roof, b_to))) = (sp.level(&e.level), sp.level(&to.level), origins(sp, &e.level, &to.level)) else {
            continue;
        };
        let from = centre(e);
        let at_to = centre(to);
        // The served unit's spot on the roof's plan, by the two plans' origins.
        let over = [at_to[0] - b_to[0] + b_roof[0], at_to[1] - b_to[1] + b_roof[1]];
        let basis = rule.basis_for(&format!("{} to {}", e.tag, to.tag));
        let id = format!("{}>{}", e.tag, to.tag);
        let up = roof.elevation_ft - below.elevation_ft + e.bottom_ft;
        let top_of_to = to.bottom_ft + to.height_ft;
        let mut pieces: Vec<serde_json::Value> = vec![json!({
            "system": REFRIGERANT, "level": e.level, "points": [from, over], "size_in": [size, 0.0], "bottoms_ft": [e.bottom_ft],
            "size_source": "rule", "height_source": "rule: at the unit's connections, on its support", "source_sheet": e.sources.get("sheet").cloned().unwrap_or_default(), "id": id,
        })];
        if (roof.name != below.name) && up.is_finite() && up > top_of_to {
            pieces.push(json!({
                "system": REFRIGERANT, "level": to.level, "points": [at_to], "size_in": [size, 0.0], "bottoms_ft": [top_of_to], "top_ft": up,
                "size_source": "rule", "height_source": "rule: down to the unit it serves", "source_sheet": e.sources.get("sheet").cloned().unwrap_or_default(), "id": id,
            }));
        }
        for p in pieces {
            if let Ok(mut r) = serde_json::from_value::<MepRun>(p) {
                super::provenance::set_run(&mut r, Made::Inferred, basis.clone());
                runs.push(r);
            }
        }
    }
    if runs.is_empty() {
        return 0;
    }
    if !sp.mep.systems.iter().any(|s| s.name == REFRIGERANT) {
        sp.mep.systems.push(cadaddy_walk3d::MepSystem {
            name: REFRIGERANT.into(),
            layer: "M-REFR".into(),
            kind: cadaddy_walk3d::MepKind::Pipe,
            size_in: [size, 0.0],
            bottom_ft: 0.0,
            show: true,
            duct_type: None,
            color_index: 0,
        });
    }
    let n = runs.iter().filter(|r| r.top_ft.is_none()).count();
    sp.mep.runs.extend(runs);
    n
}

/// Each device whose mark names a built unit (`HP` over `3` on an electrical plan: heat pump 3)
/// moved to that unit: its level, the unit's side nearest where its plan drew it, half way up
/// the unit (`E-CONN-AT-UNIT`). A mark on the plan of the storey under a roof unit follows the
/// unit onto the roof. Devices a room's own unit takes (`connect_to_equipment`) are left alone.
/// How many moved.
pub(crate) fn follow_units(equipment: &[MepEquipment], devices: &mut [super::readers::ReadDevice]) -> usize {
    let rule = super::provenance::rule("E-CONN-AT-UNIT");
    let mut n = 0usize;
    for dv in devices.iter_mut() {
        let tag = dv.equipment.trim();
        if tag.is_empty() || dv.aff_note.starts_with("connection") {
            continue;
        }
        let Some(e) = equipment.iter().find(|e| e.tag.eq_ignore_ascii_case(tag) && !matches!(e.kind.as_str(), "roof_curb" | "equipment_stand"))
        else {
            continue;
        };
        // Only a unit built from a schedule row (one a plan placed); a typical room's stamped
        // unit has its own connection rules.
        if !e.sources.contains_key("schedule") {
            continue;
        }
        let (x0, x1) = (e.min[0].min(e.max[0]), e.min[0].max(e.max[0]));
        let (y0, y1) = (e.min[1].min(e.max[1]), e.min[1].max(e.max[1]));
        if !(x0.is_finite() && x1.is_finite() && y0.is_finite() && y1.is_finite()) {
            continue;
        }
        // The side nearest its drawn spot, else (drawn over it) the nearest edge.
        let p = [dv.at[0].clamp(x0, x1), dv.at[1].clamp(y0, y1)];
        let to_edge = [(p[0] - x0, [x0, p[1]]), (x1 - p[0], [x1, p[1]]), (p[1] - y0, [p[0], y0]), (y1 - p[1], [p[0], y1])];
        let at = to_edge.iter().min_by(|a, b| a.0.total_cmp(&b.0)).map_or(p, |(_, q)| *q);
        let sheet = e.sources.get("sheet").cloned().unwrap_or_default();
        let from = if dv.level.is_empty() { String::from("?") } else { dv.level.clone() };
        dv.at = at;
        dv.level.clone_from(&e.level);
        let mid = e.bottom_ft + e.height_ft / 2.0;
        dv.aff_in = Some((mid * 12.0).clamp(1.0, 239.0));
        dv.aff_note = format!("connection to {} at the unit ({sheet}; its mark drawn on {} level {from}); {}", e.tag, dv.sheet, rule.basis_for(""));
        n += 1;
    }
    n
}

#[cfg(test)]
mod tests {
    use super::*;
    use cadaddy_doc::{Common, Space};

    fn chosen(mut r: Read) -> Read {
        r.choose(|_| true);
        r
    }

    #[test]
    fn a_shared_row_names_both_units() {
        assert_eq!(tag_variants("FC/CU-2"), vec!["FC-2".to_string(), "CU-2".to_string()]);
        assert_eq!(tag_variants("hp-7"), vec!["HP-7".to_string()]);
        assert_eq!(tag_variants("CU-1/CU-2"), vec!["CU-1".to_string(), "CU-2".to_string()]);
    }

    #[test]
    fn a_roof_is_known_by_its_name_or_code() {
        assert!(is_roof("Roof"));
        assert!(is_roof("R"));
        assert!(is_roof("HIGH ROOF"));
        assert!(!is_roof("Level 4"));
        assert!(!is_roof("4"));
    }

    fn text(d: &mut Drawing, v: &str, x: f64, y: f64) {
        let t = cadaddy_doc::Text {
            insert: cadaddy_geom::Vec3::new(x, y, 0.0),
            align_pt: None,
            height: 9.0,
            value: v.into(),
            rotation: 0.0,
            width_factor: 1.0,
            oblique: 0.0,
            style: "Standard".into(),
            halign: cadaddy_doc::HAlign::Left,
            valign: cadaddy_doc::VAlign::Baseline,
        };
        let _ = d.add(&Space::Model, Common::default(), EntityKind::Text(t));
    }

    fn rect(d: &mut Drawing, lo: [f64; 2], hi: [f64; 2]) {
        let vertices = [lo, [hi[0], lo[1]], hi, [lo[0], hi[1]]]
            .iter()
            .map(|p| cadaddy_geom::PolyVertex::with_bulge(cadaddy_geom::Vec2::new(p[0], p[1]), 0.0))
            .collect();
        let pl = cadaddy_doc::LwPolyline { vertices, closed: true, const_width: 0.0, elevation: 0.0, plinegen: false };
        let _ = d.add(&Space::Model, Common::default(), EntityKind::LwPolyline(pl));
    }

    #[test]
    fn a_split_tag_is_found_by_its_letters_and_number() {
        let mut d = Drawing::default();
        // Two tags side by side, each number under its letters.
        text(&mut d, "HP", 100.0, 100.0);
        text(&mut d, "8", 102.0, 82.0);
        text(&mut d, "HP", 172.0, 100.0);
        text(&mut d, "9", 174.0, 82.0);
        let t = plan_texts(&d);
        let (a, _) = find_tag(&t, "HP-8").unwrap_or_else(|| panic!("HP-8"));
        let (b, _) = find_tag(&t, "HP-9").unwrap_or_else(|| panic!("HP-9"));
        assert!(a[0] < 110.0 && b[0] > 160.0, "{a:?} {b:?}");
        assert!(find_tag(&t, "HP-5").is_none());
    }

    #[test]
    fn the_units_outline_is_the_one_its_size_agrees_with_not_the_tag_bubble() {
        let mut d = Drawing::default();
        // Tag box round `OAU` / `2`, a 2 ft square nearer, and the 8 x 7 ft unit (inches).
        text(&mut d, "OAU", 100.0, 100.0);
        text(&mut d, "2", 110.0, 82.0);
        rect(&mut d, [94.0, 76.0], [144.0, 114.0]);
        rect(&mut d, [150.0, 60.0], [174.0, 84.0]);
        rect(&mut d, [160.0, 0.0], [256.0, 84.0]);
        let sheet = SheetRef {
            number: "M-9".into(),
            level: "R".into(),
            discipline: "M".into(),
            kind: "plan".into(),
            rect: [0.0, -500.0, 1000.0, 500.0],
            ..Default::default()
        };
        let x = Scheduled {
            tag: "OAU-2".into(),
            kind: "outdoor_air_unit".into(),
            kind_from: "title".into(),
            sheet: "M-0".into(),
            title: "OUTDOOR AIR UNIT SCHEDULE".into(),
            ..Default::default()
        };
        let plan = PlanIn { sheet: &sheet, drawing: &d, sheet_upf: 12.0, upf: 12.0 };
        let r = chosen(place(std::slice::from_ref(&x), std::slice::from_ref(&plan), &[], &[])).equipment;
        let unit = r.iter().find(|e| e.tag == "OAU-2").unwrap_or_else(|| panic!("no unit: {:?}", r.iter().map(|e| &e.tag).collect::<Vec<_>>()));
        assert_eq!(unit.level, "Roof");
        assert_eq!((unit.min, unit.max), ([160.0, 0.0], [256.0, 84.0]), "the 8 x 7 ft outline");
        assert_eq!(unit.properties.get("STATUS").map(String::as_str), Some("drawn"));
        // On the roof, on a curb (a ducted unit), its bottom the curb's top, the curb inferred.
        let curb = r.iter().find(|e| e.kind == "roof_curb").unwrap_or_else(|| panic!("no curb"));
        assert!(unit.bottom_ft > 0.9 && (unit.bottom_ft - curb.height_ft).abs() < 1e-9, "{} {}", unit.bottom_ft, curb.height_ft);
        assert_eq!(curb.properties.get("STATUS").map(String::as_str), Some("inferred"));
        assert!(curb.properties.get("BASIS").is_some_and(|b| b.starts_with("M-ROOF-CURB")));
    }

    #[test]
    fn a_heat_pump_on_the_roof_goes_on_a_stand_and_its_connection_follows_it() {
        let mut d = Drawing::default();
        text(&mut d, "HP", 100.0, 100.0);
        text(&mut d, "7", 102.0, 82.0);
        // The 3 ft unit below its tag.
        rect(&mut d, [90.0, 20.0], [126.0, 56.0]);
        let sheet = SheetRef {
            number: "M-9".into(),
            level: "R".into(),
            discipline: "M".into(),
            kind: "plan".into(),
            rect: [0.0, -500.0, 1000.0, 500.0],
            ..Default::default()
        };
        let x = Scheduled {
            tag: "HP-7".into(),
            kind: "heat_pump".into(),
            kind_from: "title".into(),
            sheet: "M-0".into(),
            title: "HEAT PUMP SCHEDULE".into(),
            ..Default::default()
        };
        let plan = PlanIn { sheet: &sheet, drawing: &d, sheet_upf: 12.0, upf: 12.0 };
        let r = chosen(place(std::slice::from_ref(&x), std::slice::from_ref(&plan), &[], &[]));
        let unit = r.equipment.iter().find(|e| e.tag == "HP-7").unwrap_or_else(|| panic!("no unit"));
        assert_eq!((unit.min, unit.max), ([90.0, 20.0], [126.0, 56.0]));
        assert!(r.equipment.iter().any(|e| e.kind == "equipment_stand" && e.tag == "HP-7 STAND"));
        // Its electrical mark, read on the plan of the storey under the roof.
        let mut dv = vec![super::super::readers::ReadDevice {
            tag: "HP".into(),
            at: [300.0, 40.0],
            sheet: "E-9".into(),
            level: "4".into(),
            equipment: "HP-7".into(),
            ..Default::default()
        }];
        assert_eq!(follow_units(&r.equipment, &mut dv), 1);
        let dv = dv.first().unwrap_or_else(|| panic!("device"));
        assert_eq!(dv.level, "Roof");
        assert_eq!(dv.at, [126.0, 40.0], "at the unit's side nearest where it was drawn");
        let mid = (unit.bottom_ft + unit.height_ft / 2.0) * 12.0;
        assert!(dv.aff_in.is_some_and(|h| (h - mid).abs() < 1e-6), "{:?} vs {mid}", dv.aff_in);
        assert!(dv.aff_note.starts_with("connection to HP-7 at the unit"));
    }

    #[test]
    fn built_equipment_is_not_built_twice_and_an_untagged_row_is_listed() {
        let d = Drawing::default();
        let sheet = SheetRef {
            number: "M-9".into(),
            level: "2".into(),
            discipline: "M".into(),
            kind: "plan".into(),
            rect: [0.0, 0.0, 10.0, 10.0],
            ..Default::default()
        };
        let plan = PlanIn { sheet: &sheet, drawing: &d, sheet_upf: 12.0, upf: 12.0 };
        let rows = vec![
            Scheduled { tag: "PTAC-1".into(), kind: "ptac".into(), sheet: "M-0".into(), title: "PTAC SCHEDULE".into(), ..Default::default() },
            Scheduled {
                tag: "WH-9".into(),
                kind: "water_heater".into(),
                sheet: "M-0".into(),
                title: "WATER HEATER SCHEDULE".into(),
                ..Default::default()
            },
        ];
        let have = vec![MepEquipment { tag: "PTAC-1".into(), kind: "ptac".into(), level: "Level 2".into(), ..Default::default() }];
        let r = chosen(place(&rows, std::slice::from_ref(&plan), &have, &[]));
        assert_eq!(r.already, 1);
        assert!(r.equipment.is_empty());
        assert_eq!(r.unplaced.len(), 1);
        assert_eq!(r.unplaced.first().map(|u| u.0.as_str()), Some("WH-9"));
    }

    #[test]
    fn a_line_set_runs_over_the_roof_and_down_to_the_unit_it_serves() {
        let lv = |name: &str, el: f64, base: Option<[f64; 2]>| cadaddy_walk3d::Level {
            name: name.into(),
            elevation_ft: el,
            height_ft: 10.0,
            ceiling_ft: 8.0,
            region: None,
            base,
            attr: None,
            sheet: None,
        };
        let mut sp = BuildingSpec {
            units_per_foot: 12.0,
            levels: vec![lv("Level 4", 30.0, Some([1000.0, 0.0])), lv("Roof", 40.0, None)],
            ..Default::default()
        };
        let mut props = BTreeMap::new();
        props.insert("SERVES".to_string(), "FCU-7".to_string());
        let mut src = BTreeMap::new();
        src.insert("sheet".to_string(), "M-9".to_string());
        sp.mep.equipment.push(MepEquipment {
            tag: "HP-7".into(),
            kind: "heat_pump".into(),
            level: "Roof".into(),
            min: [1000.0, 0.0],
            max: [1036.0, 36.0],
            bottom_ft: 1.5,
            height_ft: 3.0,
            properties: props,
            sources: src,
            ..Default::default()
        });
        sp.mep.equipment.push(MepEquipment {
            tag: "FCU-7".into(),
            kind: "fan_coil_unit".into(),
            level: "Level 4".into(),
            min: [1200.0, 100.0],
            max: [1248.0, 124.0],
            bottom_ft: 8.0,
            height_ft: 1.5,
            ..Default::default()
        });
        assert_eq!(line_sets(&mut sp), 1);
        let runs: Vec<&MepRun> = sp.mep.runs.iter().filter(|r| r.system == REFRIGERANT).collect();
        assert_eq!(runs.len(), 2, "over the roof, then down");
        let roof = runs.iter().find(|r| r.level == "Roof").unwrap_or_else(|| panic!("roof run"));
        assert_eq!(roof.points, vec![[1018.0, 18.0], [1224.0, 112.0]]);
        let riser = runs.iter().find(|r| r.top_ft.is_some()).unwrap_or_else(|| panic!("riser"));
        assert_eq!(riser.level, "Level 4");
        assert!(riser.top_ft.is_some_and(|t| (t - 11.5).abs() < 1e-9), "{:?}", riser.top_ft);
        assert_eq!(super::super::provenance::made_of_run(roof), Made::Inferred);
        assert!(sp.mep.systems.iter().any(|s| s.name == REFRIGERANT));
        // Not twice.
        assert_eq!(line_sets(&mut sp), 0);
        // Levels with no base point yet (early in a build): their plan regions' corners (the roof
        // shares the plan region of the storey under it).
        let mut sp2 = sp.clone();
        sp2.mep.runs.clear();
        for l in &mut sp2.levels {
            l.base = None;
            l.region = Some([1000.0, 0.0, 2000.0, 900.0]);
        }
        assert_eq!(line_sets(&mut sp2), 1);
        let roof2 = sp2.mep.runs.iter().find(|r| r.level == "Roof").unwrap_or_else(|| panic!("roof run"));
        assert_eq!(roof2.points.last(), Some(&[1224.0, 112.0]));
    }

    /// A floor plan's duct label naming the unit (`S OAU-2`, no unit by it) is no place for it:
    /// the roof plan's tag with the unit's outline is, whatever order the plans come in; its
    /// curb, which the roof plan's note names, is derived from that note. A place whose sheet
    /// doesn't line up with a plan is passed over for the next.
    #[test]
    fn a_duct_label_on_a_floor_plan_does_not_take_the_roof_units_place() {
        let mut floor = Drawing::default();
        text(&mut floor, "S", 90.0, 300.0);
        text(&mut floor, "OAU-2", 100.0, 300.0);
        let mut roof = Drawing::default();
        text(&mut roof, "OAU", 100.0, 100.0);
        text(&mut roof, "2", 110.0, 82.0);
        rect(&mut roof, [160.0, 0.0], [256.0, 72.0]);
        text(&mut roof, "HORIZONTAL PLENUM CURB TO OAU-2 UNIT", 400.0, 400.0);
        let sheet = |n: &str, level: &str| SheetRef {
            number: n.into(),
            level: level.into(),
            discipline: "M".into(),
            kind: "plan".into(),
            rect: [0.0, -500.0, 1000.0, 500.0],
            ..Default::default()
        };
        let (s1, s9) = (sheet("M-1", "1"), sheet("M-9", "R"));
        let x = Scheduled {
            tag: "OAU-2".into(),
            kind: "outdoor_air_unit".into(),
            sheet: "M-0".into(),
            title: "OUTDOOR AIR UNIT SCHEDULE".into(),
            ..Default::default()
        };
        let plans =
            [PlanIn { sheet: &s1, drawing: &floor, sheet_upf: 12.0, upf: 12.0 }, PlanIn { sheet: &s9, drawing: &roof, sheet_upf: 12.0, upf: 12.0 }];
        let r = chosen(place(std::slice::from_ref(&x), &plans, &[], &[]));
        let unit = r.equipment.iter().find(|e| e.tag == "OAU-2").unwrap_or_else(|| panic!("no unit"));
        assert_eq!(unit.level, "Roof");
        assert_eq!(unit.sources.get("sheet").map(String::as_str), Some("M-9"));
        assert_eq!((unit.min, unit.max), ([160.0, 0.0], [256.0, 72.0]));
        let curb = r.equipment.iter().find(|e| e.kind == "roof_curb").unwrap_or_else(|| panic!("no curb"));
        assert_eq!(curb.properties.get("STATUS").map(String::as_str), Some("derived"));
        assert_eq!(r.equipment.iter().filter(|e| e.tag == "OAU-2").count(), 1, "built once");
        // The roof sheet doesn't line up with a plan: the floor plan's place is the one left.
        let mut r = place(std::slice::from_ref(&x), &plans, &[], &[]);
        r.choose(|e| e.sources.get("sheet").map(String::as_str) != Some("M-9"));
        assert_eq!(r.equipment.iter().map(|e| e.level.as_str()).collect::<Vec<_>>(), vec!["Level 1"]);
        // Neither lines up: not placeable, with both sheets named.
        let mut r = place(std::slice::from_ref(&x), &plans, &[], &[]);
        r.choose(|_| false);
        assert!(r.equipment.is_empty());
        assert_eq!(r.not_placeable, vec!["OAU-2 (M-9, M-1)".to_string()]);
    }

    /// Q6494: a fan schedule's ceiling exhaust fan, a PTAC schedule's unit, a unit heater whose
    /// schedule prints its mounting height, a roof fan tagged on a floor plan and a water heater,
    /// none with a printed height, each tagged on a Level 2 plan with a 9 ft ceiling.
    #[test]
    fn equipment_with_no_printed_height_is_mounted_where_its_kind_goes() {
        let mut d = Drawing::default();
        for (i, tag) in ["EF-1", "PTAC-1", "UH-1", "EF-9", "WH-1"].iter().enumerate() {
            text(&mut d, tag, 100.0 + 200.0 * i as f64, 100.0);
        }
        let sheet = SheetRef {
            number: "M-2".into(),
            level: "2".into(),
            discipline: "M".into(),
            kind: "plan".into(),
            rect: [0.0, -500.0, 2000.0, 500.0],
            ..Default::default()
        };
        let plan = PlanIn { sheet: &sheet, drawing: &d, sheet_upf: 12.0, upf: 12.0 };
        let row = |tag: &str, kind: &str, title: &str, props: &[(&str, &str)]| Scheduled {
            tag: tag.into(),
            kind: kind.into(),
            kind_from: "title".into(),
            sheet: "M-0".into(),
            title: title.into(),
            properties: props.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect(),
            ..Default::default()
        };
        let rows = vec![
            row("EF-1", "exhaust_fan", "FAN SCHEDULE", &[("CFM", "80"), ("REMARKS", "THERMOSTAT AT 48\" AFF")]),
            row("PTAC-1", "ptac", "PACKAGED TERMINAL AIR CONDITIONER SCHEDULE", &[]),
            row("UH-1", "unit_heater", "UNIT HEATER SCHEDULE", &[("MTG HT", "7'-6\"")]),
            row("EF-9", "exhaust_fan", "FAN SCHEDULE", &[("TYPE", "ROOF UPBLAST")]),
            row("WH-1", "water_heater", "WATER HEATER SCHEDULE", &[]),
        ];
        let levels = vec![cadaddy_walk3d::Level {
            name: "Level 2".into(),
            elevation_ft: 10.0,
            height_ft: 10.0,
            ceiling_ft: 9.0,
            region: None,
            base: None,
            attr: None,
            sheet: None,
        }];
        let r = chosen(place(&rows, std::slice::from_ref(&plan), &[], &levels)).equipment;
        let got = |tag: &str| r.iter().find(|e| e.tag == tag).unwrap_or_else(|| panic!("{tag}: {r:?}"));
        let near = |a: f64, b: f64| (a - b).abs() < 1e-6;
        // Its housing above the ceiling, its grille in it; never a box on the floor.
        let ef = got("EF-1");
        assert!(near(ef.bottom_ft, 9.0) && near(ef.height_ft, 10.0 / 12.0), "{ef:?}");
        assert!(ef.sources.get("elevation").is_some_and(|s| s.starts_with("M-EQUIP-MOUNT")), "the rule named: {:?}", ef.sources);
        // On its 4 in sub-base, 16 in high.
        let ptac = got("PTAC-1");
        assert!(near(ptac.bottom_ft, 4.0 / 12.0) && near(ptac.height_ft, 16.0 / 12.0), "{ptac:?}");
        // The schedule's mounting height governs.
        let uh = got("UH-1");
        assert!(near(uh.bottom_ft, 7.5), "{uh:?}");
        assert!(uh.sources.get("elevation").is_some_and(|s| s.contains("MTG HT")), "{:?}", uh.sources);
        // A roof fan tagged on a floor plan, and a kind no row names, stand on the floor.
        assert!(near(got("EF-9").bottom_ft, 0.0));
        let wh = got("WH-1");
        assert!(near(wh.bottom_ft, 0.0) && near(wh.height_ft, equip_reader::rule_size("water_heater")[2]), "{wh:?}");
    }

    #[test]
    fn a_mounting_rule_reads_its_kind_and_words_and_keeps_a_raised_unit_under_the_ceiling() {
        let m = |kind: &str, words: &str, ceiling: f64, h: Option<f64>| mounting(kind, words, ceiling, h);
        // A unit heater 18 in high under an 8 ft ceiling: its top at the ceiling, not at 8'-6".
        let uh = m("unit_heater", "unit heater", 8.0, Some(1.5)).unwrap_or_else(|| panic!("unit heater"));
        assert!((uh.bottom_ft - 6.5).abs() < 1e-9, "{uh:?}");
        // Its words pick the row: a cabinet unit heater stands on the floor.
        assert_eq!(m("unit_heater", "UNIT HEATER CABINET", 8.0, None).map(|x| x.bottom_ft), Some(0.0));
        // A word inside another word is not it (`WALL` in `DRYWALL`).
        let fan = m("exhaust_fan", "EXHAUST FAN DRYWALL CEILING", 9.0, None).unwrap_or_else(|| panic!("fan"));
        assert!((fan.bottom_ft - 9.0).abs() < 1e-9, "{fan:?}");
        // A damper sits in its duct just above the ceiling; a printed height is kept.
        let fd = m("fire_damper", "", 9.0, Some(0.5)).unwrap_or_else(|| panic!("damper"));
        assert!((fd.bottom_ft - (9.0 + 2.0 / 12.0)).abs() < 1e-9 && fd.height_ft == Some(0.5), "{fd:?}");
        // No ceiling known: the usual one.
        assert_eq!(m("vav", "", f64::NAN, None).map(|x| x.bottom_ft), Some(8.5));
        assert_eq!(m("water_heater", "", 9.0, None), None);
        assert_eq!(m("", "", 9.0, None), None);
    }

    #[test]
    fn a_schedule_mounting_height_is_read_from_its_column_never_a_remark() {
        let row = |props: &[(&str, &str)]| Scheduled {
            properties: props.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect(),
            ..Default::default()
        };
        assert_eq!(schedule_bottom_ft(&row(&[("MOUNTING HEIGHT", "8'-0\" AFF")])).map(|x| x.0), Some(8.0));
        assert_eq!(schedule_bottom_ft(&row(&[("BOTTOM AFF (IN)", "84")])).map(|x| x.0), Some(7.0));
        assert_eq!(schedule_bottom_ft(&row(&[("MOUNTING", "WALL @ 7'-0\" A.F.F.")])).map(|x| x.0), Some(7.0));
        assert_eq!(schedule_bottom_ft(&row(&[("MOUNTING", "CEILING")])), None);
        assert_eq!(schedule_bottom_ft(&row(&[("REMARKS", "THERMOSTAT AT 48\" AFF")])), None);
        assert_eq!(schedule_bottom_ft(&row(&[("MTG HT", "")])), None);
    }

    fn plumbing_plan() -> SheetRef {
        SheetRef {
            number: "P-9".into(),
            level: "1".into(),
            discipline: "P".into(),
            kind: "plan".into(),
            rect: [0.0, -500.0, 2000.0, 500.0],
            ..Default::default()
        }
    }

    fn table(title: &str, columns: &[&str], rows: &[&[&str]]) -> equip_reader::Table {
        equip_reader::Table {
            sheet: "P-0".into(),
            title: title.into(),
            columns: columns.iter().map(|c| c.to_string()).collect(),
            rows: rows.iter().map(|r| columns.iter().zip(r.iter()).map(|(c, v)| (c.to_string(), v.to_string())).collect()).collect(),
        }
    }

    /// Q6316: a plumbing fixture schedule lists the water heaters (one row for three) and the
    /// expansion tank among its fixtures; the plumbing plan tags each: each one is built, of its
    /// kind, with its row's data; the fixtures stay fixtures.
    #[test]
    fn a_plumbing_schedules_equipment_rows_are_built_where_the_plumbing_plan_tags_them() {
        let t = table(
            "PLUMBING FIXTURE SCHEDULE",
            &["TAG", "FIXTURE", "MANUFACTURER", "MODEL NO."],
            &[
                &["P-1", "WATER CLOSET", "MAKER A", "WC-10"],
                &["WH-4,5,6", "GAS WATER HEATER", "MAKER B", "TANK-100"],
                &["ET-2", "EXPANSION TANK", "MAKER C", "XT-20"],
            ],
        );
        let sched = equip_reader::scheduled(std::slice::from_ref(&t));
        let tags: Vec<(&str, &str)> = sched.iter().map(|x| (x.tag.as_str(), x.kind.as_str())).collect();
        assert_eq!(tags, vec![("WH-4", "water_heater"), ("WH-5", "water_heater"), ("WH-6", "water_heater"), ("ET-2", "expansion_tank")]);
        let mut d = Drawing::default();
        for (i, tag) in ["WH-4", "WH-5", "WH-6", "ET-2", "P-1"].iter().enumerate() {
            text(&mut d, tag, 100.0 + 200.0 * i as f64, 100.0);
        }
        let sheet = plumbing_plan();
        let plan = PlanIn { sheet: &sheet, drawing: &d, sheet_upf: 12.0, upf: 12.0 };
        let r = chosen(place(&sched, std::slice::from_ref(&plan), &[], &[]));
        let built: Vec<(&str, &str, &str)> =
            r.equipment.iter().map(|e| (e.tag.as_str(), e.kind.as_str(), e.properties.get("MODEL NO.").map_or("", String::as_str))).collect();
        assert_eq!(
            built,
            vec![
                ("WH-4", "water_heater", "TANK-100"),
                ("WH-5", "water_heater", "TANK-100"),
                ("WH-6", "water_heater", "TANK-100"),
                ("ET-2", "expansion_tank", "XT-20")
            ]
        );
        assert!(r.unplaced.is_empty(), "{:?}", r.unplaced);
        assert!(r.equipment.iter().all(|e| e.level == "Level 1"));
    }

    fn row(tag: &str, kind: &str, serves: &str) -> Scheduled {
        let mut properties = BTreeMap::new();
        properties.insert("MARK".to_string(), tag.to_string());
        properties.insert(if tag.ends_with('8') { "AREA SERVED" } else { "SERVES" }.to_string(), serves.to_string());
        properties.insert("MODEL".to_string(), format!("M-{tag}"));
        Scheduled { tag: tag.into(), kind: kind.into(), sheet: "M-0".into(), title: "FAN SCHEDULE".into(), properties, ..Default::default() }
    }

    fn fan(tag: &str, room: &str, title: &str) -> MepEquipment {
        let mut properties = BTreeMap::new();
        properties.insert("room".to_string(), room.to_string());
        MepEquipment { tag: tag.into(), kind: "exhaust_fan".into(), level: "Level 2".into(), title: title.into(), properties, ..Default::default() }
    }

    /// Q6316: exhaust fans built from the plans (a typical room's stamped fan marked only `EF`, a
    /// fan tagged with its number) take their schedule row: by their tag, else the one row of
    /// their kind that serves their room (its use named, else the row for many rooms); a fan
    /// two rows could equally be is left untyped and listed.
    #[test]
    fn built_fans_take_their_schedule_rows_type_by_tag_or_by_the_room_they_serve() {
        let rows = vec![
            row("EF-1", "exhaust_fan", "GUEST TOILET ROOMS"),
            row("EF-2", "exhaust_fan", "PUBLIC RESTROOM"),
            row("EF-3", "exhaust_fan", "MECH ROOM"),
            row("EF-8", "exhaust_fan", "CORRIDOR"),
        ];
        let mut built =
            vec![fan("EF", "201", "ENLARGED GUEST ROOM PLAN"), fan("EF", "202", "MECH ENLARGED TYPE B"), fan("EF", "210", ""), fan("EF-3", "", "")];
        let names = |_: &str, room: &str| match room {
            "210" => "CORRIDOR".to_string(),
            // A room named with ROOM, a view title with MECH: neither makes it the mechanical
            // room's fan.
            _ => "SUITE ROOM".to_string(),
        };
        let typed = type_built(&rows, &mut built, &names);
        assert_eq!((typed.by_tag, typed.by_room), (1, 3), "{typed:?}");
        let tags: Vec<&str> = built.iter().map(|e| e.tag.as_str()).collect();
        assert_eq!(tags, vec!["EF-1", "EF-1", "EF-8", "EF-3"]);
        assert_eq!(built.first().and_then(|e| e.properties.get("MODEL")).map(String::as_str), Some("M-EF-1"));
        assert!(built.first().and_then(|e| e.sources.get("type")).is_some_and(|b| b.contains("GUEST TOILET ROOMS")), "{:?}", built.first());
        assert_eq!(built.get(3).and_then(|e| e.properties.get("MODEL")).map(String::as_str), Some("M-EF-3"));
        assert_eq!(built.get(3).and_then(|e| e.properties.get("room")).map(String::as_str), Some(""), "its own data kept");

        // Two rows for one room each, neither its use: no type, listed.
        let rows = vec![row("EF-5", "exhaust_fan", "FITNESS"), row("EF-6", "exhaust_fan", "STAFF LAUNDRY")];
        let mut built = vec![fan("EF", "105", "")];
        let typed = type_built(&rows, &mut built, &|_: &str, _: &str| "OFFICE".to_string());
        assert_eq!((typed.by_tag, typed.by_room, typed.untold.len()), (0, 0, 1), "{typed:?}");
        assert_eq!(built.first().map(|e| e.tag.as_str()), Some("EF"));
    }

    /// Q6316: a plan's equipment tag no schedule row lists (letters the schedule uses, a number
    /// it doesn't) is listed; other plan text is not.
    #[test]
    fn a_plan_tag_no_schedule_row_lists_is_found() {
        let sched = vec![row("EF-1", "exhaust_fan", "GUEST TOILET ROOMS"), row("WH-2", "water_heater", "")];
        let mut d = Drawing::default();
        for (i, tag) in ["EF-1", "EF-9", "WH-7", "P-3", "AB-2", "EF-9 ABOVE", "12"].iter().enumerate() {
            text(&mut d, tag, 100.0 + 200.0 * i as f64, 100.0);
        }
        let sheet = plumbing_plan();
        let plan = PlanIn { sheet: &sheet, drawing: &d, sheet_upf: 12.0, upf: 12.0 };
        let found = unscheduled_tags(&sched, std::slice::from_ref(&plan));
        assert_eq!(found, vec![("EF-9".to_string(), "P-9".to_string()), ("WH-7".to_string(), "P-9".to_string())]);
    }

    /// Q6316: a unit built from its schedule is joined to the drawn runs that end at it on its
    /// level; a run elsewhere, or on another level, is not its.
    #[test]
    fn a_scheduled_unit_takes_the_systems_of_the_runs_ending_at_it() {
        let mut sp = BuildingSpec { units_per_foot: 12.0, ..BuildingSpec::default() };
        let mut sources = BTreeMap::new();
        sources.insert("schedule".to_string(), "P-0 PLUMBING FIXTURE SCHEDULE".to_string());
        sp.mep.equipment.push(MepEquipment {
            tag: "WH-4".into(),
            kind: "water_heater".into(),
            level: "Level 1".into(),
            min: [0.0, 0.0],
            max: [24.0, 24.0],
            sources,
            ..Default::default()
        });
        let pipe = |system: &str, level: &str, a: [f64; 2], b: [f64; 2]| super::super::printcond::run(system, level, "P-9", vec![a, b], 0.75, 8.0);
        sp.mep.runs = vec![
            pipe("HW", "Level 1", [30.0, 12.0], [300.0, 12.0]),
            pipe("CW", "Level 1", [300.0, 0.0], [10.0, -8.0]),
            pipe("G", "Level 1", [500.0, 0.0], [900.0, 0.0]),
            pipe("SAN", "Level 2", [12.0, 12.0], [200.0, 12.0]),
        ];
        assert_eq!(join_runs(&mut sp, 0), 1);
        let wh = sp.mep.equipment.first().unwrap_or_else(|| panic!("no unit"));
        assert_eq!(wh.systems, vec!["CW".to_string(), "HW".to_string()]);
        assert!(wh.sources.get("systems").is_some_and(|s| s.contains("drawn runs")));
    }
}
