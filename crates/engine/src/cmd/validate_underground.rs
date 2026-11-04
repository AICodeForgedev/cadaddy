//! G-UNDERGROUND (Q6149): the work the documents call for below grade or under the slab,
//! checked against the model and the underground sheets.
//!
//! Reading:
//! - the callouts ([`read_callouts`]): every print sheet's notes calling for buried work, its
//!   numbered lists' notes and the paragraphs outside them (`keynotes::buried_notes`), each read
//!   for what it calls for ([`Called::of`]): the trade (conduit or pipe), the count (`TWO (2)`),
//!   the size, the material (`SCH. 40 PVC`), the least depth (`MIN. 48" BELOW GRADE`) and what it
//!   serves (a utility transformer's primaries, the telephone service, a floor box, a charger). A
//!   legend's line type naming buried work (`WATER PIPING (BELOW SLAB)`) calls for no work and is
//!   left out, counted.
//! - the model's buried work ([`Buried::of`]): conduit and pipe runs below the floor, the utility
//!   services the drawings lay out, and where each comes up (a riser out of the slab, else a
//!   run's end at the gear it feeds): the stub-ups.
//! - the underground sheets ([`UgSheet`]): the set's own (a plan whose title names buried work)
//!   and the sheets CADaddy generates (`ugsheets.build`: a layout with an underground run
//!   schedule and a stub-up schedule, read back as any print's tables are).
//!
//! Checking ([`check`]):
//! - each callout: the model's run that answers it (the run its note built, else one serving
//!   what it names, else one of its size), with its size, count, material and least depth, and
//!   on an underground sheet: `pass`; another size, fewer conduits, another material or too
//!   shallow, `fail`; no run, or one no underground sheet shows, `missing`; a material the model
//!   doesn't say, `unsure`. A run CADaddy proposes never answers a callout.
//! - each run CADaddy proposes (none of the documents calls it): `proposed`, never a pass, with
//!   the review item that asks the engineer to confirm it (none: said so);
//! - each run below the slab that no callout, drawing or proposal accounts for: `extra`;
//! - each stub-up: two dimensions, across and along, from a slab edge or corner or a grid line
//!   on its underground sheet's stub-up schedule (`pass`); one or none, `fail`; a stub-up on a
//!   set's own print, `unsure` (its dimensions are read on the crop).
//!
//! Packets of a level with buried work carry the generated underground sheets as pictures and
//! the callouts, sheets and stub-ups as `underground` ([`for_packet`]).

use std::collections::{BTreeMap, BTreeSet};

use cadaddy_doc::{Drawing, Entity, EntityKind};
use cadaddy_geom::Vec2;
use cadaddy_mepfix::sheet_facts::TextItem;
use cadaddy_walk3d::{BuildingSpec, MepKind};
use serde_json::{Value, json};

use super::validate_keynote::{Checked, Ctx};
use super::validate_packet::{Area, grow, inside, point_in, pt, r2};
use super::validate_rules::Effective;

pub const RULE: &str = "G-UNDERGROUND";
/// The verdicts G-UNDERGROUND gives (`proposed`: a run CADaddy proposes, reported, never passed).
pub const VERDICTS: [&str; 6] = ["pass", "fail", "missing", "extra", "proposed", "unsure"];
const MAX_ENTITIES: usize = super::validate::MAX_ENTITIES;
/// Texts read off the sheets, at most.
const MAX_TEXTS: usize = 2_000_000;
/// Sheets read for callouts, callouts kept, at most.
const MAX_SHEETS: usize = 2_000;
const MAX_CALLOUTS: usize = 2_000;
/// Runs looked through, and buried runs and stub-ups kept, at most (the scan bound is far over
/// the cap: a job appends its buried runs after thousands of others).
const MAX_SCAN: usize = 200_000;
const MAX_BURIED: usize = 5_000;
/// Points of one run used, at most.
const MAX_POINTS: usize = 1_000;
/// Rows of a sheet's schedule read, at most.
const MAX_ROWS: usize = 2_000;
/// A run's end within this of gear feeds it; stub-ups this close are one (feet).
const FEEDS_WITHIN_FT: f64 = 3.0;
const SAME_STUB_FT: f64 = 0.5;
/// A stub-up schedule's coordinates (one decimal) match within this (feet).
const COORD_FT: f64 = 0.75;
/// How far a depth may be short, inches (the rule's `depth_in`); a size off (`size_in`).
const DEFAULT_DEPTH_IN: f64 = 1.0;
const DEFAULT_SIZE_IN: f64 = 0.1;
/// A schedule row's length matches within this share, or this many feet.
const LENGTH_SHARE: f64 = 0.05;
const LENGTH_FT: f64 = 2.0;
/// The longest note text quoted in a finding.
const QUOTE_CHARS: usize = 300;

// ---------------------------------------------------------------- what a callout calls for

/// What a callout calls for.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Called {
    /// `electrical` (conduit, conductors, a service's raceway) or `plumbing` (pipe); empty
    /// when the words say neither.
    pub trade: &'static str,
    /// How many runs (`TWO (2)`, `(3)`, `2-4"`).
    pub count: Option<u32>,
    /// Trade size, inches.
    pub size_in: Option<f64>,
    /// The material family (`PVC Sch 40`, `PVC Sch 80`, `PVC`, `RGS`, `HDPE`, `cast iron`,
    /// `copper`, `PEX`, `ductile iron`).
    pub material: Option<&'static str>,
    /// The least depth below grade or the floor, inches.
    pub min_depth_in: Option<f64>,
    /// What it serves, when it names it (`power_primary`, `telecom`, `catv`, `ev_charger`,
    /// `floor_box`, `site_lighting`, `fire_alarm`).
    pub serves: Option<&'static str>,
    /// It calls for work (a verb or a size, count or depth); a legend's line type calls for none.
    pub work: bool,
}

/// What a run may serve, and the words that name it (trade words, no job's).
const SERVES: [(&str, &[&str]); 7] = [
    ("power_primary", &["PRIMARY", "TRANSFORMER", "XFMR", "UTILITY POWER"]),
    ("telecom", &["TELCO", "TELEPHONE", "TELECOM", "COMMUNICATION", "MPOE", "MDF"]),
    ("catv", &["CATV", "CABLE TV", "CABLE TELEVISION"]),
    ("ev_charger", &["EV CHARG", "EVSE", "CHARGING STATION", "CAR CHARG", "ELECTRIC VEHICLE"]),
    ("floor_box", &["FLOOR BOX", "FLOORBOX", "FLOOR OUTLET", "POKE-THRU", "POKE THRU"]),
    ("site_lighting", &["POLE LIGHT", "LIGHT POLE", "SITE LIGHT", "BOLLARD"]),
    ("fire_alarm", &["FIRE ALARM", "FACP"]),
];
/// The services `mep.services` may give for each, by kind.
const SERVICE_KINDS: [(&str, &[&str]); 4] = [
    ("power_primary", &["power", "electric", "electrical"]),
    ("telecom", &["telephone", "telecom", "tel", "phone", "communications"]),
    ("catv", &["catv", "cable", "cable_tv"]),
    ("fire_alarm", &["fire_alarm", "fa", "fire"]),
];
const ELECTRICAL_WORDS: [&str; 10] =
    ["CONDUIT", "CONDUCTOR", "RACEWAY", "FEEDER", "DUCT BANK", "DUCTBANK", "ELECTRICAL", "PULLWIRE", "PULL WIRE", "PULL STRING"];
const PLUMBING_WORDS: [&str; 10] = ["PIPE", "PIPING", "WATER", "SANITARY", "WASTE", "VENT", "STORM", "SEWER", "GAS ", "CLEANOUT"];
const WORK_VERBS: [&str; 10] = ["PROVIDE", "INSTALL", "RUN ", "ROUTE", "EXTEND", "STUB", "FURNISH", "PLACE", "BURY", "ROUGH"];
/// Words after a value that make it a depth (`48" BELOW GRADE`), or before it (`MIN. 48"`).
const DEPTH_AFTER: [&str; 8] = ["BELOW", "DEEP", "COVER", "BURIAL", "BURY", "UNDER", "BFF", "BELOW-GRADE"];
const DEPTH_BEFORE: [&str; 7] = ["MIN", "MIN.", "MINIMUM", "DEPTH", "COVER", "AT", "OF"];
const NUMBER_WORDS: [(&str, u32); 10] =
    [("ONE", 1), ("TWO", 2), ("THREE", 3), ("FOUR", 4), ("FIVE", 5), ("SIX", 6), ("SEVEN", 7), ("EIGHT", 8), ("NINE", 9), ("TEN", 10)];

/// A value in inches as written: `4"`, `1-1/4"`, `3/4"`, `4'-0"` (feet and inches, `true`).
fn inches(word: &str) -> Option<(f64, bool)> {
    let w = word.trim_matches(|c: char| matches!(c, '(' | ')' | ',' | ';' | ':' | '.')).trim_end_matches('.');
    if let Some((ft, rest)) = w.split_once('\'') {
        let f: f64 = ft.trim().parse().ok()?;
        let i = rest.trim_start_matches('-').trim_end_matches('"');
        let i = if i.is_empty() { 0.0 } else { fraction(i)? };
        return Some((f * 12.0 + i, true)).filter(|(v, _)| v.is_finite() && *v >= 0.0 && *v <= 1_200.0);
    }
    let v = w.strip_suffix('"').or_else(|| w.strip_suffix("IN"))?;
    // `2-4"`: a count before the size is not a mixed number.
    let v = fraction(v)?;
    (v.is_finite() && v > 0.0 && v <= 120.0).then_some((v, false))
}

/// `1-1/4` → 1.25, `3/4` → 0.75, `4` → 4.
fn fraction(s: &str) -> Option<f64> {
    let s = s.trim();
    let part = |p: &str| -> Option<f64> {
        match p.split_once('/') {
            Some((a, b)) => {
                let (a, b): (f64, f64) = (a.trim().parse().ok()?, b.trim().parse().ok()?);
                (b > 0.0).then(|| a / b)
            }
            None => p.trim().parse().ok(),
        }
    };
    match s.split_once('-') {
        Some((w, f)) if f.contains('/') => Some(part(w)? + part(f)?),
        Some(_) => None,
        None => part(s),
    }
}

fn count_word(w: &str) -> Option<u32> {
    let w = w.trim_matches(|c: char| !c.is_ascii_alphanumeric());
    NUMBER_WORDS.iter().find(|(n, _)| *n == w).map(|(_, v)| *v).or_else(|| w.parse::<u32>().ok().filter(|v| (1..=50).contains(v)))
}

fn has_any(up: &str, words: &[&str]) -> bool {
    words.iter().any(|w| up.contains(w))
}

impl Called {
    /// What a callout's words call for.
    pub fn of(text: &str) -> Called {
        let up = format!(" {} ", text.to_ascii_uppercase().split_whitespace().collect::<Vec<_>>().join(" "));
        let mut c = Called::default();
        let electrical = has_any(&up, &ELECTRICAL_WORDS);
        let plumbing = has_any(&up, &PLUMBING_WORDS);
        c.trade = if electrical {
            "electrical"
        } else if plumbing {
            "plumbing"
        } else {
            ""
        };
        c.serves = SERVES.iter().find(|(_, ws)| has_any(&up, ws)).map(|(k, _)| *k);
        // `SCH. 40 PVC`, `PVC SCHEDULE 80`, `RIGID STEEL`.
        let compact: String = up.chars().filter(|c| c.is_ascii_alphanumeric()).collect();
        let tokens: Vec<&str> = up.split(|c: char| !c.is_ascii_alphanumeric()).filter(|t| !t.is_empty()).collect();
        c.material = if tokens.iter().any(|t| matches!(*t, "RGS" | "RMC" | "GRC" | "RIGID")) {
            Some("RGS")
        } else if up.contains("PVC") || compact.contains("SCH40") || compact.contains("SCHEDULE40") || compact.contains("SCH80") {
            Some(if compact.contains("SCH80") || compact.contains("SCHEDULE80") {
                "PVC Sch 80"
            } else if compact.contains("SCH40") || compact.contains("SCHEDULE40") {
                "PVC Sch 40"
            } else {
                "PVC"
            })
        } else if up.contains("HDPE") {
            Some("HDPE")
        } else if up.contains("CAST IRON") || up.contains(" C.I. ") {
            Some("cast iron")
        } else if up.contains("DUCTILE") {
            Some("ductile iron")
        } else if up.contains("COPPER") || up.contains(" TYPE K ") {
            Some("copper")
        } else if up.contains(" PEX") {
            Some("PEX")
        } else {
            None
        };
        // `48 IN`, `24 INCHES`: one value, as `48"`.
        let mut merged: Vec<String> = Vec::new();
        for w in up.split_whitespace() {
            let unit = matches!(w.trim_end_matches([',', ';', '.']), "IN" | "INCH" | "INCHES");
            match merged.last_mut() {
                Some(prev) if unit && prev.parse::<f64>().is_ok_and(|v| v.is_finite()) => prev.push('"'),
                _ => merged.push(w.to_string()),
            }
        }
        let words: Vec<&str> = merged.iter().map(String::as_str).collect();
        let mut sizes: Vec<(usize, f64)> = Vec::new();
        for (i, w) in words.iter().enumerate() {
            let Some((v, ft_in)) = inches(w) else {
                // `2-4"`, `3-1-1/4"`: a count of runs before their size.
                if let Some((n, size)) = w.split_once('-').filter(|(n, s)| !n.is_empty() && s.ends_with('"') && !n.contains('\''))
                    && let (Ok(n), Some((v, false))) = (n.parse::<u32>(), inches(size))
                    && (1..=50).contains(&n)
                {
                    c.count = c.count.or(Some(n));
                    sizes.push((i, v));
                }
                continue;
            };
            // A depth: a depth word right after it (`48" BELOW GRADE`, `24" MIN. COVER`), or
            // before it (`MIN. 48"`).
            let next = words.get(i + 1).copied().unwrap_or_default();
            let after = DEPTH_AFTER.iter().any(|d| next.starts_with(d)) || matches!(next, "MIN" | "MIN." | "MINIMUM");
            let before = i.checked_sub(1).and_then(|j| words.get(j)).is_some_and(|x| DEPTH_BEFORE.contains(x));
            if ft_in || after || before {
                c.min_depth_in = Some(c.min_depth_in.map_or(v, |d: f64| d.max(v)));
            } else {
                sizes.push((i, v));
            }
        }
        // The size: the first one that is a trade size for its trade.
        let most = if c.trade == "plumbing" { 48.0 } else { 6.0 };
        if let Some((i, v)) = sizes.into_iter().find(|(_, v)| *v >= 0.375 && *v <= most) {
            c.size_in = Some(v);
            // `TWO (2) 5"`, `(2) 4"`, `TWO 4"`.
            let back = |k: usize| i.checked_sub(k).and_then(|j| words.get(j)).copied();
            if c.count.is_none() {
                c.count =
                    back(1).and_then(count_word).or_else(|| back(2).and_then(count_word).filter(|_| back(1).is_some_and(|w| w.starts_with('('))));
            }
        }
        if c.count.is_none() {
            // `TWO (2) CONDUITS` with the size elsewhere.
            c.count = words.windows(2).find_map(|p| match p {
                [a, b] if b.starts_with('(') && b.ends_with(')') => count_word(a).filter(|n| count_word(b) == Some(*n)),
                _ => None,
            });
        }
        c.work = WORK_VERBS.iter().any(|v| up.contains(&format!(" {v}"))) || c.size_in.is_some() || c.count.is_some() || c.min_depth_in.is_some();
        c
    }
}

/// A callout of buried work on a sheet.
#[derive(Clone, Debug)]
pub struct Callout {
    pub sheet: String,
    /// Its keynote's number, when it is a numbered list's note.
    pub number: Option<String>,
    pub text: String,
    /// The level its sheet shows, when one.
    pub level: Option<String>,
    pub calls: Called,
}

fn plain(e: &Entity) -> Option<(String, Vec2, f64, f64)> {
    match &e.kind {
        EntityKind::Text(t) => Some((t.value.clone(), t.insert.xy(), t.height, t.rotation)),
        EntityKind::MText(t) => {
            Some((cadaddy_fonts::plain_mtext(&t.contents).split_whitespace().collect::<Vec<_>>().join(" "), t.insert.xy(), t.height, t.rotation))
        }
        _ => None,
    }
}

/// Every print sheet's callouts of buried work (a sheet printed in several links is read once).
pub fn read_callouts(d: &Drawing, sp: &BuildingSpec) -> (Vec<Callout>, usize) {
    // One sheet per number and place.
    let mut sheets: Vec<(String, [f64; 4], Option<String>)> = Vec::new();
    for l in sp.sheets.iter().take(MAX_SHEETS) {
        let Some(r) = l.rect.map(super::validate_packet::norm_rect) else { continue };
        if !r.iter().all(|v| v.is_finite()) {
            continue;
        }
        match sheets.iter_mut().find(|(n, sr, _)| n.eq_ignore_ascii_case(&l.number) && *sr == r) {
            // Two views of one sheet show two levels: the sheet's own is neither.
            Some(s) => {
                if s.2.as_deref().is_some_and(|x| l.level.as_deref().is_some_and(|y| !x.eq_ignore_ascii_case(y))) {
                    s.2 = None;
                }
            }
            None => sheets.push((l.number.clone(), r, l.level.clone())),
        }
    }
    let mut texts: Vec<Vec<TextItem>> = vec![Vec::new(); sheets.len()];
    let mut read = 0usize;
    for e in d.model.iter().take(MAX_ENTITIES) {
        let Some((value, at, h, rot)) = plain(e) else { continue };
        let value = value.trim().to_string();
        if value.is_empty() || !at.is_finite() || !h.is_finite() || h <= 0.0 {
            continue;
        }
        for (i, (_, r, _)) in sheets.iter().enumerate() {
            if inside(*r, [at.x, at.y])
                && let Some(t) = texts.get_mut(i)
            {
                t.push(TextItem { value: value.clone(), at, height: h, rotation: if rot.is_finite() { rot } else { 0.0 } });
                read += 1;
            }
        }
        if read >= MAX_TEXTS {
            break;
        }
    }
    let mut out: Vec<Callout> = Vec::new();
    let mut legends = 0usize;
    for ((number, _, level), ts) in sheets.iter().zip(&texts) {
        for (n, text) in super::keynotes::buried_notes(ts) {
            let calls = Called::of(&text);
            if !calls.work {
                legends += 1;
                continue;
            }
            if out.len() >= MAX_CALLOUTS || out.iter().any(|c| c.sheet.eq_ignore_ascii_case(number) && c.text == text && c.number == n) {
                continue;
            }
            out.push(Callout { sheet: number.clone(), number: n, text, level: level.clone(), calls });
        }
    }
    (out, legends)
}

// ---------------------------------------------------------------- the model's buried work

/// A run below the floor or in the ground.
#[derive(Clone, Debug)]
pub struct Run {
    pub id: String,
    /// `run` (conduit or pipe below the floor) or `service` (a utility entrance).
    pub kind: &'static str,
    pub trade: &'static str,
    pub system: String,
    pub level: String,
    /// Plan points, drawing units.
    pub points: Vec<[f64; 2]>,
    pub size_in: f64,
    pub conduits: u32,
    /// From the system's name or layer, or the service's; none when nothing says.
    pub material: Option<&'static str>,
    /// Below the floor or grade to its top, inches.
    pub depth_in: f64,
    pub length_ft: f64,
    pub from: String,
    pub to: String,
    pub proposed: bool,
    /// Where it comes from (its note, its sheet, the feeder schedule).
    pub basis: String,
    /// A document gives it (a note built it, a sheet was read for it, the drawings lay out the
    /// service).
    pub from_document: bool,
    /// What it serves (from its system, ends and basis), for matching a callout.
    serves: Option<&'static str>,
}

/// Where buried work comes up.
#[derive(Clone, Debug)]
pub struct Stub {
    pub level: String,
    /// Drawing units on the level's plan.
    pub at: [f64; 2],
    pub runs: Vec<String>,
    pub proposed: bool,
}

/// The model's buried work.
pub struct Buried {
    pub runs: Vec<Run>,
    pub stubs: Vec<Stub>,
}

/// The material family a name gives (`E-UG PVC40` → `PVC Sch 40`).
fn material_in(name: &str) -> Option<&'static str> {
    Called::of(&name.replace(['-', '_'], " ")).material.or_else(|| {
        let c: String = name.to_ascii_uppercase().chars().filter(|c| c.is_ascii_alphanumeric()).collect();
        if c.contains("PVC80") {
            Some("PVC Sch 80")
        } else if c.contains("PVC40") {
            Some("PVC Sch 40")
        } else {
            None
        }
    })
}

/// The same material: a family a callout names (`PVC`) holds its schedules.
fn same_material(called: &str, model: &str) -> bool {
    called.eq_ignore_ascii_case(model) || (called == "PVC" && model.starts_with("PVC")) || (model == "PVC" && called.starts_with("PVC"))
}

fn upf(sp: &BuildingSpec) -> f64 {
    if sp.units_per_foot.is_finite() && sp.units_per_foot > 0.0 { sp.units_per_foot } else { 12.0 }
}

fn finite(p: [f64; 2]) -> bool {
    p[0].is_finite() && p[1].is_finite()
}

fn length(pts: &[[f64; 2]]) -> f64 {
    pts.windows(2)
        .map(|w| match w {
            [a, b] => (b[0] - a[0]).hypot(b[1] - a[1]),
            _ => 0.0,
        })
        .sum()
}

/// The point halfway along a run (its only point, for one).
fn halfway(pts: &[[f64; 2]]) -> [f64; 2] {
    let mut left = length(pts) / 2.0;
    for w in pts.windows(2) {
        if let [a, b] = w {
            let l = (b[0] - a[0]).hypot(b[1] - a[1]);
            if l > 0.0 && left <= l {
                let t = left / l;
                return [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t];
            }
            left -= l;
        }
    }
    pts.first().copied().unwrap_or([0.0, 0.0])
}

fn serves_in(words: &str) -> Option<&'static str> {
    SERVES.iter().find(|(_, ws)| has_any(words, ws)).map(|(k, _)| *k)
}

impl Buried {
    pub fn of(sp: &BuildingSpec) -> Buried {
        let u = upf(sp);
        let sheets: BTreeSet<String> = sp.sheets.iter().map(|l| l.number.to_ascii_uppercase()).collect();
        let gear_at = |level: &str, p: [f64; 2]| {
            sp.mep.equipment.iter().take(MAX_SCAN).find(|e| {
                (e.level.is_empty() || e.level.eq_ignore_ascii_case(level)) && {
                    let dx = (e.min[0].min(e.max[0]) - p[0]).max(p[0] - e.min[0].max(e.max[0])).max(0.0);
                    let dy = (e.min[1].min(e.max[1]) - p[1]).max(p[1] - e.min[1].max(e.max[1])).max(0.0);
                    dx.hypot(dy) <= FEEDS_WITHIN_FT * u
                }
            })
        };
        let gear_words = |tag: &str| {
            sp.mep
                .equipment
                .iter()
                .take(MAX_SCAN)
                .find(|e| !tag.is_empty() && e.tag.eq_ignore_ascii_case(tag))
                .map(|e| format!("{} {} {}", e.kind, e.title, e.systems.join(" ")))
                .unwrap_or_default()
        };
        let mut runs: Vec<Run> = Vec::new();
        let mut risers: Vec<(String, [f64; 2], String, bool)> = Vec::new();
        for (i, r) in sp.mep.runs.iter().enumerate().take(MAX_SCAN) {
            if runs.len() >= MAX_BURIED && risers.len() >= MAX_BURIED {
                break;
            }
            let Some(sys) = sp.mep.systems.iter().find(|s| s.name.eq_ignore_ascii_case(&r.system)) else { continue };
            let trade = match sys.kind {
                MepKind::Conduit => "electrical",
                MepKind::Pipe => "plumbing",
                _ => continue,
            };
            let bottom = r.bottoms_ft.iter().copied().filter(|b| b.is_finite()).fold(f64::INFINITY, f64::min);
            let bottom = if bottom.is_finite() { bottom } else { sys.bottom_ft };
            if !(bottom.is_finite() && bottom < 0.0) {
                continue;
            }
            let points: Vec<[f64; 2]> = r.points.iter().copied().filter(|p| finite(*p)).take(MAX_POINTS).collect();
            let id = if r.id.trim().is_empty() { format!("{}-{}", if trade == "plumbing" { "P" } else { "UG" }, i + 1) } else { r.id.clone() };
            let proposed = r.height_source.eq_ignore_ascii_case("proposed");
            // A riser out of the ground: where it comes up.
            if points.len() == 1 {
                if r.top_ft.is_some_and(|t| t.is_finite() && t >= -1e-6)
                    && risers.len() < MAX_BURIED
                    && let Some(p) = points.first()
                {
                    risers.push((r.level.clone(), *p, id, proposed));
                }
                continue;
            }
            if points.len() < 2 || runs.len() >= MAX_BURIED {
                continue;
            }
            let size = [r.size_in[0], sys.size_in[0]].into_iter().find(|v| v.is_finite() && *v > 0.0).unwrap_or(0.0);
            // A feeder's run says its ends (`feeder 1 A→B`) where no gear is at one.
            let said = r.source_sheet.strip_prefix("feeder ").and_then(|v| v.split_once(' ')).and_then(|(_, ends)| ends.split_once('→'));
            let end = |p: Option<&[f64; 2]>, said: Option<&str>| {
                p.and_then(|p| gear_at(&r.level, *p)).map(|e| e.tag.clone()).or_else(|| said.map(|s| s.trim().to_string())).unwrap_or_default()
            };
            let from = end(points.first(), said.map(|(a, _)| a));
            let to = end(points.last(), said.map(|(_, b)| b.split('→').next().unwrap_or(b)));
            let basis =
                [r.source_sheet.as_str(), r.height_source.as_str()].into_iter().filter(|v| !v.trim().is_empty()).collect::<Vec<_>>().join("; ");
            let words =
                format!(" {} {} {} {} {} {} {} ", sys.name, sys.layer, from, to, gear_words(&from), gear_words(&to), basis).to_ascii_uppercase();
            runs.push(Run {
                id,
                kind: "run",
                trade,
                system: r.system.clone(),
                level: r.level.clone(),
                length_ft: length(&points) / u,
                points,
                size_in: size,
                conduits: 1,
                material: material_in(&format!("{} {}", sys.name, sys.layer)),
                depth_in: (-bottom * 12.0 - size).max(0.0),
                from,
                to,
                proposed,
                from_document: r.height_source.trim_start().to_ascii_lowercase().starts_with("note")
                    || sheets.contains(&r.source_sheet.trim().to_ascii_uppercase()),
                serves: serves_in(&words),
                basis,
            });
        }
        // The services the drawings lay out.
        for svc in sp.mep.services.iter().take(MAX_BURIED) {
            let (Some(at), true) = (svc.at, svc.shown) else { continue };
            let points: Vec<[f64; 2]> = if svc.route.len() >= 2 {
                svc.route.iter().take(MAX_POINTS).copied().filter(|p| finite(*p)).collect()
            } else {
                svc.from.into_iter().chain([at]).filter(|p| finite(*p)).collect()
            };
            if points.len() < 2 || runs.len() >= MAX_BURIED {
                continue;
            }
            let kind = svc.kind.trim().to_ascii_lowercase();
            let pipe = matches!(kind.as_str(), "water" | "sewer" | "storm" | "gas");
            let serves = SERVICE_KINDS.iter().find(|(_, ks)| ks.contains(&kind.as_str())).map(|(k, _)| *k);
            let size = if pipe { svc.size_in.unwrap_or(0.0) } else { svc.conduit_in };
            let depth = svc.depth_ft.map(|d| d * 12.0).filter(|d| d.is_finite()).unwrap_or(svc.cover_in);
            let id = if svc.tag.trim().is_empty() { format!("SVC-{}", kind.to_ascii_uppercase()) } else { svc.tag.clone() };
            let words = format!(" {} {} {} ", svc.kind, svc.tag, svc.to).to_ascii_uppercase();
            runs.push(Run {
                id,
                kind: "service",
                trade: if pipe { "plumbing" } else { "electrical" },
                system: svc.kind.clone(),
                level: svc.level.clone(),
                length_ft: length(&points) / u,
                points,
                size_in: if size.is_finite() && size > 0.0 { size } else { 0.0 },
                conduits: svc.conduits.max(1),
                material: material_in(&svc.material),
                depth_in: if depth.is_finite() && depth >= 0.0 { depth } else { 0.0 },
                from: if svc.from.is_some() { "site".into() } else { String::new() },
                to: svc.to.clone(),
                proposed: false,
                basis: format!("{} service", svc.kind),
                from_document: true,
                serves: serves.or_else(|| serves_in(&words)),
            });
        }
        // Stub-ups: the risers out of the ground, else a run's ends at the gear it feeds.
        let near = SAME_STUB_FT * u;
        let mut stubs: Vec<Stub> = Vec::new();
        let add = |level: &str, at: [f64; 2], run: &str, proposed: bool, stubs: &mut Vec<Stub>| {
            let same = stubs.iter().position(|s| s.level.eq_ignore_ascii_case(level) && (s.at[0] - at[0]).hypot(s.at[1] - at[1]) <= near);
            match same.and_then(|i| stubs.get_mut(i)) {
                Some(s) => {
                    if !s.runs.iter().any(|x| x == run) && s.runs.len() < 50 {
                        s.runs.push(run.to_string());
                    }
                    s.proposed &= proposed;
                }
                None => {
                    if stubs.len() < MAX_BURIED {
                        stubs.push(Stub { level: level.to_string(), at, runs: vec![run.to_string()], proposed });
                    }
                }
            }
        };
        for (level, at, id, proposed) in &risers {
            add(level, *at, id, *proposed, &mut stubs);
        }
        for r in runs.iter().filter(|r| r.kind == "run") {
            for p in [r.points.first(), r.points.last()].into_iter().flatten() {
                let risen = risers.iter().any(|(l, q, _, _)| l.eq_ignore_ascii_case(&r.level) && (q[0] - p[0]).hypot(q[1] - p[1]) <= near);
                if !risen && gear_at(&r.level, *p).is_some() {
                    add(&r.level, *p, &r.id, r.proposed, &mut stubs);
                }
            }
        }
        Buried { runs, stubs }
    }
}

// ---------------------------------------------------------------- the underground sheets

/// A row of a sheet's schedule: header → cell.
type Row = BTreeMap<String, String>;

/// An underground sheet: one the set issues (a print whose title names buried work and a plan)
/// or one CADaddy generated (a layout with an underground run schedule).
#[derive(Clone, Debug)]
pub struct UgSheet {
    pub number: String,
    pub title: String,
    pub level: String,
    /// The generated sheet's layout.
    pub layout: Option<String>,
    /// The set's own: where it lies on the drawing and its place on the level's plan.
    rect: Option<[f64; 4]>,
    registration: Option<cadaddy_walk3d::levels::SheetRegistration>,
    runs: Vec<Row>,
    stubs: Vec<Row>,
    /// The run schedule stops short (`… and 12 more`).
    more_runs: bool,
}

impl UgSheet {
    pub fn generated(&self) -> bool {
        self.layout.is_some()
    }
    fn json(&self) -> Value {
        json!({"number": self.number, "title": self.title, "level": self.level, "layout": self.layout, "generated": self.generated(),
               "runs_listed": self.runs.len(), "stubups_listed": self.stubs.len()})
    }
}

/// A title naming buried work and a plan (`UNDERGROUND PLAN`, `UNDER-SLAB ELECTRICAL PLAN`).
fn is_ug_title(t: &str) -> bool {
    let up = t.to_ascii_uppercase();
    super::keynotes::buried(&up) && up.split(|c: char| !c.is_ascii_alphanumeric()).any(|w| w == "PLAN" || w == "PLANS")
}

struct Txt {
    text: String,
    at: [f64; 2],
    h: f64,
}

/// The rows of the table titled `title` among a sheet's texts: the header row under the title,
/// then each row its cells (each under the header it starts at), until the texts stop.
fn table(texts: &[Txt], title: &Txt) -> (Vec<Row>, bool) {
    let below: Vec<&Txt> =
        texts.iter().filter(|t| t.at[1] < title.at[1] - 0.1 * title.h && t.at[0] >= title.at[0] - title.h && t.h < title.h).collect();
    let Some(cell_h) = below.iter().map(|t| t.h).min_by(f64::total_cmp) else { return (Vec::new(), false) };
    let mut rows: Vec<(f64, Vec<&Txt>)> = Vec::new();
    let mut sorted = below;
    sorted.sort_by(|a, b| b.at[1].total_cmp(&a.at[1]));
    for t in sorted {
        if (t.h - cell_h).abs() > 0.3 * cell_h {
            continue;
        }
        match rows.last_mut() {
            Some((y, cells)) if (*y - t.at[1]).abs() <= 0.5 * cell_h => cells.push(t),
            Some((y, _)) if *y - t.at[1] > 4.0 * cell_h => break,
            None if title.at[1] - t.at[1] > 6.0 * cell_h => break,
            _ => {
                if rows.len() >= MAX_ROWS {
                    break;
                }
                rows.push((t.at[1], vec![t]));
            }
        }
    }
    let mut it = rows.into_iter();
    let Some((_, mut header)) = it.next() else { return (Vec::new(), false) };
    header.sort_by(|a, b| a.at[0].total_cmp(&b.at[0]));
    let mut out = Vec::new();
    let mut more = false;
    for (_, cells) in it {
        if cells.iter().any(|c| c.text.starts_with('…') || c.text.to_ascii_lowercase().contains(" more ")) {
            more = true;
            break;
        }
        let mut row = Row::new();
        for c in cells {
            let col = header.iter().rev().find(|h| h.at[0] <= c.at[0] + 0.25 * cell_h).or(header.first());
            if let Some(h) = col {
                row.insert(h.text.trim().to_ascii_uppercase(), c.text.trim().to_string());
            }
        }
        out.push(row);
    }
    (out, more)
}

/// The level a generated sheet is of: the spec level its layout name or title ends with.
fn level_of(sp: &BuildingSpec, names: &[&str]) -> Option<String> {
    let mut best: Option<&str> = None;
    for l in &sp.levels {
        let n = l.name.trim();
        if n.is_empty() {
            continue;
        }
        let up = n.to_ascii_uppercase();
        if names.iter().any(|x| x.trim().to_ascii_uppercase().ends_with(&up)) && best.is_none_or(|b| b.len() < n.len()) {
            best = Some(n);
        }
    }
    best.map(str::to_string)
}

/// The set's underground plans and the generated underground sheets.
pub fn sheets(d: &Drawing, sp: &BuildingSpec) -> Vec<UgSheet> {
    let mut out: Vec<UgSheet> = Vec::new();
    for l in sp.sheets.iter().take(MAX_SHEETS) {
        let title = l.view.as_ref().map_or(l.title.as_str(), |v| v.title.as_str());
        if !(is_ug_title(title) || is_ug_title(&l.title)) {
            continue;
        }
        let Some(level) = l.level.clone() else { continue };
        out.push(UgSheet {
            number: l.number.clone(),
            title: title.to_string(),
            level,
            layout: None,
            rect: l.view.as_ref().map(|v| v.rect).or(l.rect).map(super::validate_packet::norm_rect),
            registration: l.registration.clone(),
            runs: Vec::new(),
            stubs: Vec::new(),
            more_runs: false,
        });
    }
    for lay in d.layouts.iter().take(MAX_SHEETS) {
        let texts: Vec<Txt> = lay
            .entities
            .iter()
            .take(MAX_ENTITIES)
            .filter_map(|e| plain(e).filter(|(t, at, h, _)| !t.trim().is_empty() && at.is_finite() && h.is_finite() && *h > 0.0))
            .map(|(text, at, h, _)| Txt { text: text.trim().to_string(), at: [at.x, at.y], h })
            .take(MAX_TEXTS)
            .collect();
        let schedule = |words: &str| {
            texts.iter().find(|t| {
                let up = t.text.to_ascii_uppercase();
                up.contains(words) && (words != "RUN SCHEDULE" || super::keynotes::buried(&up))
            })
        };
        let Some(run_title) = schedule("RUN SCHEDULE") else { continue };
        let title = texts.iter().filter(|t| is_ug_title(&t.text) || super::keynotes::buried(&t.text)).max_by(|a, b| a.h.total_cmp(&b.h));
        let title_text = title.map(|t| t.text.clone()).unwrap_or_else(|| lay.name.clone());
        let Some(level) = level_of(sp, &[&lay.name, &title_text]) else { continue };
        let (runs, more_runs) = table(&texts, run_title);
        let stubs = schedule("STUB-UP SCHEDULE").or_else(|| schedule("STUB UP SCHEDULE")).map(|t| table(&texts, t).0).unwrap_or_default();
        let number = lay.name.split_whitespace().next().unwrap_or(&lay.name).to_string();
        out.push(UgSheet {
            number,
            title: title_text,
            level,
            layout: Some(lay.name.clone()),
            rect: None,
            registration: None,
            runs,
            stubs,
            more_runs,
        });
    }
    out
}

/// `1-3/4"` (count, then size) → (1, 0.75).
fn qty_size(cell: &str) -> Option<(u32, f64)> {
    let (n, s) = cell.trim().split_once('-')?;
    Some((n.trim().parse().ok()?, fraction(s.trim().trim_end_matches('"'))?))
}

fn number_in(cell: &str) -> Option<f64> {
    let t: String = cell.chars().take_while(|c| c.is_ascii_digit() || *c == '.' || *c == '-').collect();
    t.parse().ok().filter(|v: &f64| v.is_finite())
}

/// Whether a generated sheet's run schedule lists a run: its size and count, depth and length,
/// and its ends when the row gives them.
fn listed(row: &Row, r: &Run, size_tol: f64, depth_tol: f64) -> bool {
    let get = |k: &str| row.get(k).map(String::as_str).unwrap_or_default();
    let size_ok = qty_size(get("QTY-SIZE")).is_some_and(|(n, s)| n == r.conduits && (s - r.size_in).abs() <= size_tol.max(0.01));
    let depth_ok = number_in(get("DEPTH")).is_some_and(|v| (v - r.depth_in).abs() <= depth_tol.max(0.5));
    let len_ok = number_in(get("LENGTH")).is_some_and(|v| (v - r.length_ft).abs() <= (r.length_ft * LENGTH_SHARE).max(LENGTH_FT));
    let ends_ok = [("FROM", &r.from), ("TO", &r.to)].iter().all(|(k, v)| get(k).is_empty() || v.is_empty() || get(k).eq_ignore_ascii_case(v));
    size_ok && depth_ok && len_ok && ends_ok
}

/// The underground sheet that shows a run (`more`: a generated sheet whose schedule stops short
/// may list it past the end).
fn shown_on<'a>(ug: &'a [UgSheet], r: &Run, size_tol: f64, depth_tol: f64) -> (Option<&'a UgSheet>, bool) {
    let mut more = false;
    for s in ug.iter().filter(|s| s.level.eq_ignore_ascii_case(&r.level)) {
        if s.generated() {
            if s.runs.iter().any(|row| listed(row, r, size_tol, depth_tol)) {
                return (Some(s), false);
            }
            more |= s.more_runs;
        } else if let Some(rect) = s.rect {
            // The set's plan: the run's middle lies on the sheet, placed by its registration.
            let mid = halfway(&r.points);
            let on = match &s.registration {
                Some(reg) => reg.to_sheet(mid).is_some_and(|p| inside(rect, p)),
                None => inside(rect, mid),
            };
            if on {
                return (Some(s), false);
            }
        }
    }
    (None, more)
}

/// A stub-up schedule's dimension cell: its reference, when it is a slab edge or corner or a
/// grid line (`4'-0" E of slab W`, `on 3`); `-`, empty or a level's extent (`west edge`) is
/// none.
fn dimension(cell: Option<&String>) -> Option<String> {
    let c = cell?.trim();
    if c.is_empty() || c == "-" {
        return None;
    }
    let reference = c.split_once(" of ").map(|(_, r)| r).or_else(|| c.strip_prefix("on ")).unwrap_or(c).trim();
    let low = reference.to_ascii_lowercase();
    (!reference.is_empty() && (low.starts_with("slab") || !low.ends_with("edge"))).then(|| reference.to_string())
}

// ---------------------------------------------------------------- checking

fn tol(r: &Effective, name: &str, default: f64) -> f64 {
    r.rule.tolerance.get(name).copied().filter(|t| t.is_finite() && *t >= 0.0).unwrap_or(default)
}

fn quote(t: &str) -> String {
    let q: String = t.chars().take(QUOTE_CHARS).collect();
    if t.chars().count() > QUOTE_CHARS { format!("\"{q}…\"") } else { format!("\"{q}\"") }
}

fn size_words(v: f64) -> String {
    super::ugsheets::trade_size(v)
}

/// The slab level: the lowest one (the first, when none has an elevation).
fn slab_level(sp: &BuildingSpec) -> Option<String> {
    sp.levels
        .iter()
        .filter(|l| l.elevation_ft.is_finite())
        .min_by(|a, b| a.elevation_ft.total_cmp(&b.elevation_ft))
        .or(sp.levels.first())
        .map(|l| l.name.clone())
}

/// The area a point of a level is in, and whether the scope takes it.
fn area_for<'a>(areas: &'a [Area], whole_levels: bool, level: &str, at: [f64; 2]) -> Option<Option<&'a Area>> {
    let mine: Vec<&Area> = areas.iter().filter(|a| a.level.eq_ignore_ascii_case(level)).collect();
    if mine.is_empty() {
        return None;
    }
    let a = mine.iter().copied().find(|a| inside(a.bbox, at) && (a.outline.len() < 3 || point_in(&a.outline, at)));
    if a.is_some() || whole_levels || mine.iter().any(|x| inside(grow(x.bbox, 0.5), at)) { Some(a) } else { None }
}

fn run_json(r: &Run, ft: [f64; 2]) -> Value {
    json!({"id": r.id, "kind": r.kind, "trade": r.trade, "system": r.system, "level": r.level, "from": r.from, "to": r.to,
           "size_in": r2(r.size_in), "conduits": r.conduits, "material": r.material, "depth_in": r2(r.depth_in),
           "length_ft": r2(r.length_ft), "proposed": r.proposed, "basis": r.basis, "at_ft": pt(ft)})
}

/// The review item that asks to confirm a proposed run: an RFI naming it (its id or basis).
fn review_item<'a>(sp: &'a BuildingSpec, r: &Run) -> Option<&'a cadaddy_walk3d::Rfi> {
    let names: Vec<String> = [r.id.as_str(), r.basis.split(';').next().unwrap_or_default()]
        .iter()
        .map(|n| n.trim().to_ascii_uppercase())
        .filter(|n| n.len() >= 3)
        .collect();
    sp.rfis.iter().take(100_000).find(|q| {
        let t = format!("{} {} {}", q.about, q.issue, q.proposed.as_deref().unwrap_or_default()).to_ascii_uppercase();
        names.iter().any(|n| t.contains(n.as_str()))
    })
}

/// Check the documents' buried work against the model and the underground sheets (G-UNDERGROUND).
pub(crate) fn check(cx: &Ctx, r: &Effective) -> Checked {
    let mut out = Checked::default();
    let sp = cx.sp;
    let u = upf(sp);
    let depth_tol = tol(r, "depth_in", DEFAULT_DEPTH_IN);
    let size_tol = tol(r, "size_in", DEFAULT_SIZE_IN);
    let (callouts, legends) = read_callouts(cx.d, sp);
    if legends > 0 {
        out.unmeasured.insert("a legend's line type or a note names buried work but calls for none".into(), legends as u64);
    }
    let buried = Buried::of(sp);
    let ug = sheets(cx.d, sp);
    let slab = slab_level(sp).unwrap_or_default();
    let ft = |level: &str, p: [f64; 2]| cx.frame.to_ft_on(level, Vec2::new(p[0], p[1]));
    let mid_ft = |run: &Run| ft(&run.level, halfway(&run.points));
    let rank_of = |verdict: &str| match verdict {
        "pass" => 9u8,
        "unsure" | "proposed" => 5,
        _ => match r.rule.severity.as_str() {
            "high" => 0,
            "medium" => 1,
            _ => 2,
        },
    };
    #[allow(clippy::too_many_arguments)]
    let push = |out: &mut Checked,
                key: &str,
                verdict: &'static str,
                handle: Option<String>,
                kind: &str,
                level: &str,
                at: Option<[f64; 2]>,
                issue: String,
                expected: String,
                found: String,
                measured: Value| {
        let area = match at {
            Some(p) => match area_for(cx.areas, cx.whole_levels, level, p) {
                Some(a) => a,
                None => return,
            },
            // Work no place on the plan holds (a note): the whole level's.
            None if cx.whole_levels && cx.areas.iter().any(|a| a.level.eq_ignore_ascii_case(level)) => None,
            None => return,
        };
        let id = format!("PS-{}", super::validate_run::hash8(&format!("{RULE}|{key}|{verdict}")));
        out.findings.push((
            rank_of(verdict),
            verdict,
            json!({
                "id": id,
                "rule": RULE,
                "handle": handle,
                "kind": kind,
                "verdict": verdict,
                "issue": issue,
                "expected": expected,
                "found": found,
                "measured": measured,
                "evidence": [],
                "confidence": if matches!(verdict, "unsure" | "proposed") { 0.5 } else { 1.0 },
                "severity": if verdict == "proposed" { "medium" } else { r.rule.severity.as_str() },
                "area": area.map(|a| a.id.clone()),
                "level": level,
                "at": at.map(pt),
                "source": "prescreen",
            }),
        ));
    };
    // The runs each callout's note built (`note: E-101 keynote 4 …`), claimed first.
    let cites = |c: &Callout, run: &Run| {
        let b = run.basis.to_ascii_uppercase();
        let by_number =
            c.number.as_ref().is_some_and(|n| b.contains(&format!("{} KEYNOTE {} ", c.sheet.to_ascii_uppercase(), n.to_ascii_uppercase())));
        let head: String = c.text.chars().take(60).collect::<String>().to_ascii_uppercase();
        by_number || (head.len() >= 30 && b.contains(head.as_str()))
    };
    let mut answered: BTreeSet<String> = BTreeSet::new();
    for c in &callouts {
        let level = c.level.clone().unwrap_or_else(|| slab.clone());
        let of_trade: Vec<&Run> = buried
            .runs
            .iter()
            .filter(|run| c.calls.trade.is_empty() || run.trade == c.calls.trade)
            .filter(|run| c.level.as_deref().is_none_or(|l| l.eq_ignore_ascii_case(&run.level)))
            .collect();
        let cited: Vec<&Run> = of_trade.iter().copied().filter(|run| cites(c, run)).collect();
        let (matched, how): (Vec<&Run>, &str) = if !cited.is_empty() {
            (cited, "its note built it")
        } else if let Some(sv) = c.calls.serves {
            (of_trade.iter().copied().filter(|run| run.serves == Some(sv)).collect(), "it serves what the note names")
        } else if let Some(size) = c.calls.size_in {
            (of_trade.iter().copied().filter(|run| run.serves.is_none() && (run.size_in - size).abs() <= size_tol).collect(), "its size")
        } else {
            (Vec::new(), "")
        };
        let called: Vec<&Run> = matched.iter().copied().filter(|run| !run.proposed).collect();
        let proposed: Vec<&Run> = matched.iter().copied().filter(|run| run.proposed).collect();
        let cite = match &c.number {
            Some(n) => format!("keynote {n} on {} {}", c.sheet, quote(&c.text)),
            None => format!("the note on {} {}", c.sheet, quote(&c.text)),
        };
        let mut want: Vec<String> = Vec::new();
        if let Some(n) = c.calls.count {
            want.push(format!("{n}"));
        }
        if let Some(s) = c.calls.size_in {
            want.push(size_words(s));
        }
        if let Some(m) = c.calls.material {
            want.push(m.to_string());
        }
        want.push(if c.calls.trade == "plumbing" { "pipe".into() } else { "conduit".into() });
        if let Some(dp) = c.calls.min_depth_in {
            want.push(format!("at least {}\" deep", r2(dp)));
        }
        let expected = format!("{} below grade or the slab, in the model and on an underground sheet", want.join(" "));
        let calls = json!({"trade": c.calls.trade, "count": c.calls.count, "size_in": c.calls.size_in, "material": c.calls.material,
                           "min_depth_in": c.calls.min_depth_in, "serves": c.calls.serves});
        let callout = json!({"sheet": c.sheet, "number": c.number, "text": c.text, "calls": calls});
        let key = format!("callout|{}|{}|{}", c.sheet, c.number.clone().unwrap_or_default(), c.text);
        if called.is_empty() {
            let found = if proposed.is_empty() {
                "no buried run in the model answers it".to_string()
            } else {
                format!(
                    "only CADaddy's proposal ({}), which no document calls: not counted as called",
                    proposed.iter().map(|p| p.id.as_str()).take(5).collect::<Vec<_>>().join(", ")
                )
            };
            let at = proposed.first().map(|p| mid_ft(p));
            push(
                &mut out,
                &key,
                "missing",
                None,
                "buried_run",
                proposed.first().map_or(level.as_str(), |p| p.level.as_str()),
                at,
                format!("{cite} calls for buried work the model doesn't have"),
                expected,
                found,
                json!({"callout": callout, "proposed": proposed.iter().map(|p| p.id.clone()).collect::<Vec<_>>()}),
            );
            continue;
        }
        for run in &called {
            answered.insert(run.id.clone());
        }
        // The runs side by side: as many as the count, each checked.
        let have: u32 = called.iter().map(|run| run.conduits).fold(0u32, u32::saturating_add);
        let mut fails: Vec<String> = Vec::new();
        let mut unsure: Vec<String> = Vec::new();
        if let Some(n) = c.calls.count.filter(|n| have < *n) {
            fails.push(format!("{have} of the {n} runs it calls for"));
        }
        if let Some(size) = c.calls.size_in {
            for run in called.iter().filter(|run| run.size_in > 0.0 && (run.size_in - size).abs() > size_tol) {
                fails.push(format!("{} is {} (not {})", run.id, size_words(run.size_in), size_words(size)));
            }
            for run in called.iter().filter(|run| run.size_in <= 0.0) {
                unsure.push(format!("{}'s size isn't given", run.id));
            }
        }
        if let Some(m) = c.calls.material {
            for run in &called {
                match run.material {
                    Some(x) if !same_material(m, x) => fails.push(format!("{} is {x} (not {m})", run.id)),
                    Some(_) => {}
                    None => unsure.push(format!("the model doesn't say {}'s material", run.id)),
                }
            }
        }
        if let Some(dp) = c.calls.min_depth_in {
            for run in called.iter().filter(|run| run.depth_in + depth_tol < dp) {
                fails.push(format!("{} is {}\" deep (at least {}\" called)", run.id, r2(run.depth_in), r2(dp)));
            }
        }
        let mut on_sheets: Vec<String> = Vec::new();
        let mut off: Vec<String> = Vec::new();
        for run in &called {
            match shown_on(&ug, run, size_tol, depth_tol) {
                (Some(s), _) => {
                    if !on_sheets.contains(&s.number) {
                        on_sheets.push(s.number.clone());
                    }
                }
                (None, true) => unsure.push(format!("{}: the underground sheet's run schedule stops short", run.id)),
                (None, false) => off.push(run.id.clone()),
            }
        }
        let verdict = if !fails.is_empty() {
            "fail"
        } else if !off.is_empty() {
            "missing"
        } else if !unsure.is_empty() {
            "unsure"
        } else {
            "pass"
        };
        let mut problems = fails.clone();
        if !off.is_empty() {
            problems.push(format!(
                "no underground sheet of {} shows {}",
                called.first().map_or(level.as_str(), |x| x.level.as_str()),
                off.join(", ")
            ));
        }
        problems.extend(unsure.iter().cloned());
        let found = format!(
            "{} ({how}){}",
            called
                .iter()
                .take(6)
                .map(|run| format!(
                    "{} {}-{} {} at {}\"",
                    run.id,
                    run.conduits,
                    size_words(run.size_in),
                    run.material.unwrap_or("material not given"),
                    r2(run.depth_in)
                ))
                .collect::<Vec<_>>()
                .join("; "),
            if on_sheets.is_empty() { String::new() } else { format!(", on {}", on_sheets.join(", ")) }
        );
        let issue = if verdict == "pass" {
            format!("{cite}: the model and {} have it", on_sheets.join(", "))
        } else {
            format!("{cite}: {}", problems.join("; "))
        };
        let first = called.first().copied();
        push(
            &mut out,
            &key,
            verdict,
            first.map(|x| x.id.clone()),
            "buried_run",
            first.map_or(level.as_str(), |x| x.level.as_str()),
            first.map(&mid_ft),
            issue,
            expected,
            found,
            json!({"callout": callout, "matched_by": how, "runs": called.iter().map(|x| run_json(x, mid_ft(x))).collect::<Vec<_>>(), "sheets": on_sheets}),
        );
    }
    // Each proposal (its runs side by side are one) and each run nothing accounts for.
    let mut proposals: BTreeMap<(String, String), Vec<&Run>> = BTreeMap::new();
    for run in &buried.runs {
        if run.proposed {
            proposals.entry((run.level.to_ascii_lowercase(), run.basis.clone())).or_default().push(run);
        } else if !answered.contains(&run.id) && !run.from_document {
            let at = mid_ft(run);
            push(
                &mut out,
                &format!("extra|{}", run.id),
                "extra",
                Some(run.id.clone()),
                "buried_run",
                &run.level,
                Some(at),
                format!("{} is below the slab, but no document calls for it and nothing proposes it", run.id),
                "buried work a note, a drawing or a proposal accounts for".into(),
                format!(
                    "{}-{} {} at {}\" ({})",
                    run.conduits,
                    size_words(run.size_in),
                    run.system,
                    r2(run.depth_in),
                    if run.basis.is_empty() { "no basis" } else { &run.basis }
                ),
                json!({"run": run_json(run, at)}),
            );
        }
    }
    for ((_, basis), runs) in &proposals {
        let Some(first) = runs.first() else { continue };
        let item = runs.iter().find_map(|run| review_item(sp, run));
        let at = mid_ft(first);
        let ids: Vec<String> = runs.iter().map(|x| x.id.clone()).collect();
        let shown = runs.iter().filter_map(|x| shown_on(&ug, x, size_tol, depth_tol).0.map(|s| s.number.clone())).collect::<BTreeSet<_>>();
        push(
            &mut out,
            &format!("proposed|{}|{basis}", first.level),
            "proposed",
            Some(first.id.clone()),
            "buried_run",
            &first.level,
            Some(at),
            match item {
                Some(q) => format!(
                    "{} is CADaddy's proposal ({basis}), not called by the documents; {} asks the engineer to confirm it",
                    ids.join(", "),
                    q.id
                ),
                None => format!(
                    "{} is CADaddy's proposal ({basis}), not called by the documents, and no review item asks the engineer to confirm it",
                    ids.join(", ")
                ),
            },
            "a run the documents call for, or a proposal the engineer confirms before the pour".into(),
            format!(
                "{} run(s) {}-{} at {}\"{}",
                runs.len(),
                first.conduits,
                size_words(first.size_in),
                r2(first.depth_in),
                if shown.is_empty() { String::new() } else { format!(", dashed on {}", shown.into_iter().collect::<Vec<_>>().join(", ")) }
            ),
            json!({"runs": runs.iter().map(|x| run_json(x, mid_ft(x))).collect::<Vec<_>>(), "review_item": item.map(|q| json!({"id": q.id, "about": q.about, "issue": q.issue}))}),
        );
    }
    // Each stub-up: two dimensions from a slab edge or corner or a grid line.
    let g = super::ugsheets::grids(sp);
    let (ox, oy) = (g.x.first().map_or(0.0, |x| x.1), g.y.first().map_or(0.0, |y| y.1));
    for st in &buried.stubs {
        let at = ft(&st.level, st.at);
        let here: Vec<&UgSheet> = ug.iter().filter(|s| s.level.eq_ignore_ascii_case(&st.level)).collect();
        let coord = [(st.at[0] - ox) / u, (st.at[1] - oy) / u];
        let row = here.iter().filter(|s| s.generated()).find_map(|s| {
            s.stubs
                .iter()
                .find(|row| {
                    let c: Vec<f64> =
                        row.get("COORD (FT)").map(|v| v.split(',').filter_map(|x| x.trim().parse::<f64>().ok()).collect()).unwrap_or_default();
                    matches!(c.as_slice(), [x, y] if (x - coord[0]).abs() <= COORD_FT && (y - coord[1]).abs() <= COORD_FT)
                })
                .map(|row| (*s, row))
        });
        let key = format!("stub|{}|{:.1},{:.1}", st.level, at[0], at[1]);
        let what = json!({"level": st.level, "at_ft": pt(at), "runs": st.runs, "proposed": st.proposed});
        let label = st.runs.first().map_or("a stub-up".to_string(), |x| format!("the stub-up of {x}"));
        match row {
            Some((s, row)) => {
                let x = dimension(row.get("FROM GRID X"));
                let y = dimension(row.get("FROM GRID Y"));
                let n = usize::from(x.is_some()) + usize::from(y.is_some());
                let tag = row.get("STUB").cloned().unwrap_or_default();
                let mut m = what.clone();
                m["sheet"] = json!(s.number);
                m["stub"] = json!(tag);
                m["x"] = json!({"cell": row.get("FROM GRID X"), "from": x});
                m["y"] = json!({"cell": row.get("FROM GRID Y"), "from": y});
                let verdict = if n == 2 { "pass" } else { "fail" };
                let issue = match n {
                    2 => format!("{label} ({tag} on {}) is dimensioned both ways", s.number),
                    1 => format!(
                        "{label} ({tag} on {}) has one dimension: nothing measures it {} from a slab edge or a grid line",
                        s.number,
                        if x.is_none() { "across (east-west)" } else { "along (north-south)" }
                    ),
                    _ => format!("{label} ({tag} on {}) has no dimension from a slab edge or a grid line", s.number),
                };
                push(
                    &mut out,
                    &key,
                    verdict,
                    st.runs.first().cloned(),
                    "stub_up",
                    &st.level,
                    Some(at),
                    issue,
                    "two perpendicular dimensions to a slab corner or edge, or a grid line".into(),
                    format!("{} | {}", row.get("FROM GRID X").map_or("-", String::as_str), row.get("FROM GRID Y").map_or("-", String::as_str)),
                    m,
                );
            }
            None if here.iter().any(|s| !s.generated()) => {
                *out.unmeasured.entry("stub-up dimensions on the set's own underground print are read on its crop".into()).or_default() += 1;
                let s = here.iter().find(|s| !s.generated()).map_or(String::new(), |s| s.number.clone());
                push(
                    &mut out,
                    &key,
                    "unsure",
                    st.runs.first().cloned(),
                    "stub_up",
                    &st.level,
                    Some(at),
                    format!("{label}: its dimensions are on {s}, a print: read them on the crop"),
                    "two perpendicular dimensions to a slab corner or edge, or a grid line".into(),
                    "not measured".into(),
                    what,
                );
            }
            None => {
                let issue = match here.iter().find(|s| s.generated()) {
                    Some(s) => format!("{label} isn't on {}'s stub-up schedule: nothing dimensions it", s.number),
                    None => format!("no underground sheet of {} shows {label}: nothing dimensions it", st.level),
                };
                push(
                    &mut out,
                    &key,
                    "fail",
                    st.runs.first().cloned(),
                    "stub_up",
                    &st.level,
                    Some(at),
                    issue,
                    "two perpendicular dimensions to a slab corner or edge, or a grid line".into(),
                    "none".into(),
                    what,
                );
            }
        }
    }
    out.markers = callouts.len();
    out
}

// ---------------------------------------------------------------- packets

/// A level's underground work for its packets: the callouts, the underground sheets and the
/// buried runs and stub-ups (feet); and the generated sheets' layouts, to draw. None when the
/// level has no buried work, no callout and no underground sheet.
pub(crate) fn for_packet(d: &Drawing, sp: &BuildingSpec, frame: &super::validate::Frame, level: &str) -> Option<(Value, Vec<(String, String)>)> {
    let slab = slab_level(sp).unwrap_or_default();
    let ug: Vec<UgSheet> = sheets(d, sp).into_iter().filter(|s| s.level.eq_ignore_ascii_case(level)).collect();
    let buried = Buried::of(sp);
    let runs: Vec<&Run> = buried.runs.iter().filter(|r| r.level.eq_ignore_ascii_case(level)).collect();
    let stubs: Vec<&Stub> = buried.stubs.iter().filter(|s| s.level.eq_ignore_ascii_case(level)).collect();
    let callouts: Vec<Callout> = if slab.eq_ignore_ascii_case(level) || !runs.is_empty() || !ug.is_empty() {
        read_callouts(d, sp)
            .0
            .into_iter()
            .filter(|c| c.level.as_deref().map_or(slab.eq_ignore_ascii_case(level), |l| l.eq_ignore_ascii_case(level)))
            .collect()
    } else {
        Vec::new()
    };
    if ug.is_empty() && runs.is_empty() && callouts.is_empty() {
        return None;
    }
    let ft = |p: [f64; 2]| frame.to_ft_on(level, Vec2::new(p[0], p[1]));
    let layouts: Vec<(String, String)> = ug.iter().filter_map(|s| Some((s.number.clone(), s.layout.clone()?))).collect();
    Some((
        json!({
            "about": "the work the documents call for below grade or under the slab (callouts), the underground sheets (the set's own and the ones CADaddy generated: their pictures are the `underground_sheet` prints), and the model's buried runs and stub-ups; G-UNDERGROUND in `prescreen` matched each callout to a run",
            "callouts": callouts.iter().take(60).map(|c| json!({"sheet": c.sheet, "number": c.number, "text": c.text,
                "calls": {"trade": c.calls.trade, "count": c.calls.count, "size_in": c.calls.size_in, "material": c.calls.material, "min_depth_in": c.calls.min_depth_in, "serves": c.calls.serves}})).collect::<Vec<_>>(),
            "sheets": ug.iter().map(UgSheet::json).collect::<Vec<_>>(),
            "runs": runs.iter().take(200).map(|r| run_json(r, ft(halfway(&r.points)))).collect::<Vec<_>>(),
            "stubups": stubs.iter().take(200).map(|s| json!({"at_ft": pt(ft(s.at)), "runs": s.runs, "proposed": s.proposed})).collect::<Vec<_>>(),
        }),
        layouts,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_callouts_count_size_material_and_depth_are_read() {
        let c = Called::of("TWO (2) 5\" SCH. 40 PVC CONDUIT WITH STEEL ELLS AND RISERS, MIN. 48\" BELOW GRADE WITH PULLWIRE FOR PRIMARY CONDUCTORS.");
        assert_eq!(c.trade, "electrical");
        assert_eq!(c.count, Some(2));
        assert_eq!(c.size_in, Some(5.0));
        assert_eq!(c.material, Some("PVC Sch 40"));
        assert_eq!(c.min_depth_in, Some(48.0));
        assert_eq!(c.serves, Some("power_primary"));
        assert!(c.work);
        // Count-dash-size, feet and inches of cover, a fraction.
        let c = Called::of("PROVIDE 3-1-1/4\" RGS CONDUITS UNDERGROUND TO THE POLE LIGHT, 2'-0\" COVER");
        assert_eq!((c.count, c.size_in, c.material, c.min_depth_in), (Some(3), Some(1.25), Some("RGS"), Some(24.0)));
        assert_eq!(c.serves, Some("site_lighting"));
        let c = Called::of("PROVIDE UNDER SLAB CONDUIT FOR FLOOR BOX RECEPTACLE.");
        assert_eq!((c.count, c.size_in, c.min_depth_in, c.serves), (None, None, None, Some("floor_box")));
        assert!(c.work);
        let c = Called::of("4\" SANITARY PIPING BELOW SLAB, CAST IRON");
        assert_eq!((c.trade, c.size_in, c.material), ("plumbing", Some(4.0), Some("cast iron")));
        // `48 IN` is a value in inches.
        let c = Called::of("PROVIDE TWO (2) 4\" SCH. 40 PVC CONDUITS 48 IN BELOW GRADE FOR THE TELEPHONE SERVICE ENTRANCE.");
        assert_eq!((c.count, c.size_in, c.min_depth_in, c.serves), (Some(2), Some(4.0), Some(48.0), Some("telecom")));
        // A legend's line type: buried, but no work.
        let c = Called::of("COLD WATER PIPING (BELOW SLAB)");
        assert!(!c.work, "{c:?}");
    }

    #[test]
    fn a_stub_up_schedules_dimensions_name_their_reference() {
        let cell = |s: &str| Some(s.to_string());
        assert_eq!(dimension(cell("4'-0\" E of slab W").as_ref()), Some("slab W".into()));
        assert_eq!(dimension(cell("on 3").as_ref()), Some("3".into()));
        assert_eq!(dimension(cell("12'-6\" N of B").as_ref()), Some("B".into()));
        // The level's extent is no slab corner or grid line.
        assert_eq!(dimension(cell("3'-0\" N of south edge").as_ref()), None);
        assert_eq!(dimension(cell("-").as_ref()), None);
        assert_eq!(dimension(None), None);
        assert_eq!(qty_size("1-3/4\""), Some((1, 0.75)));
        assert_eq!(qty_size("2-1-1/4\""), Some((2, 1.25)));
    }
}
