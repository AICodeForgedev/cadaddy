//! G-KEYNOTE (Q6147): work only a plan keynote calls for — a box of a size, a device, conduit of a
//! size and route, a sleeve — checked against the model at the keynote's marker.
//!
//! Reading ([`Keynotes::read`]):
//! - each sheet's keynote lists: a `… KEYNOTES` (`KEYED NOTES`, `KEY NOTES`) title and the
//!   numbered notes under it (`3. PROVIDE …`, `10.ROUTE …`, a bare number with its words
//!   beside it), each note's lines read on until the next number;
//! - the sheet's keynote markers: a number of the sheet's list (the list of the sheet's own
//!   trade when it has several) about as tall as the list's words, inside a closed shape
//!   (hexagon, circle, square, diamond: linework on all four sides of it), carried onto its
//!   level's plan by the sheet's registration (its view's, for a sheet printing several
//!   levels; every level a typical-floor view covers);
//! - what each note calls for ([`Calls::of`]): a box of a size (`8"X8"X6" JUNCTION BOX`; a BDA
//!   antenna or riser box), conduit of a size with its pull tape and route (`1" CONDUIT WITH
//!   PULL TAPE ABOVE THE CORRIDOR CEILING`; a riser `BETWEEN THE RISER BOXES`; below the slab),
//!   a sleeve, a device the note provides (`PROVIDE TWO SMOKE DETECTORS`), and the fire rating
//!   to keep (`KEEP THE 2-HOUR RATING OF FLOORS AND WALLS`).
//!
//! Checking ([`check`]), each call at each marker in scope, within the rule's `route_ft` of the
//! marker on its level:
//! - a box: one of that size (each side within `size_in`) is a `pass`; boxes of other sizes only,
//!   a `fail`; boxes whose size nothing says, `unsure`; none, `missing`;
//! - a device or sleeve: as many of its kind as the note says (`missing` when fewer);
//! - conduit (the spec's MEP runs of a conduit system, and runs drawn as conduit): one of that
//!   size there, else a `fail` (other sizes only) or `missing`; then its route: above the ceiling
//!   (its bottom against the reflected ceiling plan's height there, else the level's ceiling), in
//!   a corridor, below the slab — a route that can't be measured is `unsure`;
//! - riser conduit: a vertical run of that size at the marker that goes up to the next level with
//!   a riser marker by it (at least to that floor); the highest one is reached from below;
//! - every rated wall the matched conduit crosses (its wall type's rating, the note's) and every
//!   floor a riser passes (the note's rating) is listed with the rating its firestopping keeps.
//!
//! Packets carry each sheet's keynote list, the markers in and by the area, and crops of both
//! ([`for_packet`]).

use std::collections::{BTreeMap, BTreeSet, HashMap};

use cadaddy_doc::{Drawing, Entity, EntityKind};
use cadaddy_geom::Vec2;
use cadaddy_walk3d::levels::SheetLink;
use cadaddy_walk3d::validate::{Model, Prints};
use cadaddy_walk3d::{BuildingSpec, MepKind};
use serde_json::{Value, json};

use super::validate::Frame;
use super::validate_packet::{Area, grow, inside, norm_rect, point_in, pt, r2, seg_dist};
use super::validate_rules::Effective;

pub const RULE: &str = "G-KEYNOTE";
/// The verdicts G-KEYNOTE gives.
pub const VERDICTS: [&str; 4] = ["pass", "fail", "missing", "unsure"];
const MAX_ENTITIES: usize = super::validate::MAX_ENTITIES;
/// Texts read off the sheets, at most.
const MAX_TEXTS: usize = 2_000_000;
/// Keynote lists, notes per list and characters per note, at most.
const MAX_LISTS: usize = 2_000;
const MAX_NOTES: usize = 99;
const MAX_NOTE_CHARS: usize = 1_200;
/// Markers read, at most.
const MAX_MARKERS: usize = 20_000;
/// Linework kept round one marker, at most.
const MAX_SEGS_PER_MARKER: usize = 400;
/// Wall crossings listed per conduit, at most.
const MAX_CROSSINGS: usize = 50;
/// How far from its marker a called object may be, feet (the rule's `route_ft`).
pub const DEFAULT_REACH_FT: f64 = 10.0;
/// How far a size may be off, inches (the rule's `size_in`).
const DEFAULT_SIZE_IN: f64 = 0.1;
/// How far a riser may stop short of the floor it goes up to, feet.
const RISER_SLACK_FT: f64 = 0.5;
/// The longest note text quoted in a finding.
const QUOTE_CHARS: usize = 300;

// ---------------------------------------------------------------- what a note calls for

/// A box a note calls for.
#[derive(Clone, Debug, PartialEq)]
pub struct BoxCall {
    /// Its kind: `bda_antenna_box`, `bda_riser_box`, `floor_box`, `pull_box`, `junction_box`.
    pub kind: &'static str,
    /// Its three sides, inches, as written (none: no size given).
    pub dims_in: Option<[f64; 3]>,
}

/// Conduit a note calls for.
#[derive(Clone, Debug, PartialEq)]
pub struct ConduitCall {
    /// Trade size, inches.
    pub size_in: f64,
    /// How many (`TWO (2) 4"`), 1 when not said.
    pub count: u32,
    pub pull_tape: bool,
    /// Runs above the ceiling (`ABOVE CEILING`, `ABOVE HARD LID`).
    pub above_ceiling: bool,
    /// The ceiling it runs above is a hard lid.
    pub hard_lid: bool,
    /// Runs in (or is stubbed into) the corridor.
    pub corridor: bool,
    /// Below the slab or grade.
    pub below: bool,
    /// A riser between floors.
    pub riser: bool,
    /// The sentence it was read from.
    pub words: String,
}

/// A device a note provides.
#[derive(Clone, Debug, PartialEq)]
pub struct DeviceCall {
    pub kind: &'static str,
    pub count: u32,
}

/// The fire rating a note says to keep.
#[derive(Clone, Debug, PartialEq)]
pub struct Rating {
    pub hours: f64,
    pub floors: bool,
    pub walls: bool,
}

/// What one note calls for.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Calls {
    pub boxes: Vec<BoxCall>,
    pub conduits: Vec<ConduitCall>,
    /// Sleeves, with their size when given.
    pub sleeves: Vec<Option<f64>>,
    pub devices: Vec<DeviceCall>,
    pub rating: Option<Rating>,
}

/// Device words a note may provide, and the kind each is.
const DEVICES: [(&str, &str); 24] = [
    ("EXIT SIGN", "exit_sign"),
    ("EMERGENCY LIGHT", "emergency_light"),
    ("SMOKE DETECTOR", "smoke_detector"),
    ("HEAT DETECTOR", "heat_detector"),
    ("CARBON MONOXIDE DETECTOR", "co_detector"),
    ("DUCT DETECTOR", "duct_detector"),
    ("HORN/STROBE", "horn_strobe"),
    ("HORN STROBE", "horn_strobe"),
    ("STROBE", "strobe"),
    ("PULL STATION", "pull_station"),
    ("OCCUPANCY SENSOR", "occupancy_sensor"),
    ("VACANCY SENSOR", "occupancy_sensor"),
    ("THERMOSTAT", "thermostat"),
    ("DISCONNECT SWITCH", "disconnect"),
    ("DISCONNECT", "disconnect"),
    ("GFCI RECEPTACLE", "receptacle"),
    ("RECEPTACLE", "receptacle"),
    ("DATA OUTLET", "data_outlet"),
    ("TELEPHONE OUTLET", "data_outlet"),
    ("CARD READER", "card_reader"),
    ("PUSH BUTTON", "push_button"),
    ("CAMERA", "camera"),
    ("SPEAKER", "speaker"),
    ("MOTION SENSOR", "occupancy_sensor"),
];

/// Words that start a note about something else than an object at the marker.
const NOT_AN_OBJECT: [&str; 9] = ["CONNECT", "CIRCUIT", "POWER", "ROUTE", "EXTEND", "REFER", "SEE ", "COORDINATE", "EXISTING"];

const NUMBER_WORDS: [(&str, u32); 10] =
    [("ONE", 1), ("TWO", 2), ("THREE", 3), ("FOUR", 4), ("FIVE", 5), ("SIX", 6), ("SEVEN", 7), ("EIGHT", 8), ("NINE", 9), ("TEN", 10)];

/// A number written as words or digits (`TWO`, `(2)`, `2`).
fn count_word(w: &str) -> Option<u32> {
    let t = w.trim_matches(|c: char| c == '(' || c == ')' || c == ',');
    NUMBER_WORDS.iter().find(|(n, _)| *n == t).map(|(_, v)| *v).or_else(|| t.parse::<u32>().ok().filter(|n| (1..=50).contains(n)))
}

/// A note's sentences: split at a period that ends a word (not `SCH.`, `NO.`, `TYP.`, `3.5`).
fn sentences(t: &str) -> Vec<String> {
    const KEEP: [&str; 9] = ["SCH", "NO", "TYP", "MIN", "MAX", "APPROX", "EQ", "DIA", "EA"];
    let chars: Vec<char> = t.chars().collect();
    let mut out = Vec::new();
    let mut cur = String::new();
    for (i, c) in chars.iter().enumerate() {
        if *c == '.' {
            let next = chars.get(i + 1).copied();
            let prev = i.checked_sub(1).and_then(|j| chars.get(j)).copied();
            let decimal = prev.is_some_and(|p| p.is_ascii_digit()) && next.is_some_and(|n| n.is_ascii_digit());
            let word: String = cur.chars().rev().take_while(|c| c.is_ascii_alphabetic()).collect::<Vec<_>>().into_iter().rev().collect();
            if !decimal && !KEEP.contains(&word.as_str()) && next.is_none_or(|n| n == ' ') {
                if !cur.trim().is_empty() {
                    out.push(cur.trim().to_string());
                }
                cur.clear();
                continue;
            }
        }
        cur.push(*c);
    }
    if !cur.trim().is_empty() {
        out.push(cur.trim().to_string());
    }
    out
}

/// Box sides written `8"X8"X6"`, `12 X 12 X 6`, `4"x4"x2-1/8"` → inches.
fn dims3(t: &str) -> Option<[f64; 3]> {
    let s: String = t.to_ascii_uppercase().chars().map(|c| if c == '×' { 'X' } else { c }).filter(|c| *c != '"' && *c != ' ').collect();
    // Each `X` between two numbers.
    let parts: Vec<&str> = s.split('X').collect();
    for w in parts.windows(3) {
        let [a, b, c] = w else { continue };
        let tail = |p: &str| -> Option<f64> {
            let num: String = p
                .chars()
                .rev()
                .take_while(|c| c.is_ascii_digit() || *c == '.' || *c == '/' || *c == '-')
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
                .collect();
            trade_size(&num)
        };
        let head = |p: &str| -> Option<f64> {
            let num: String = p.chars().take_while(|c| c.is_ascii_digit() || *c == '.' || *c == '/' || *c == '-').collect();
            trade_size(&num)
        };
        let mid = b.chars().all(|c| c.is_ascii_digit() || c == '.' || c == '/' || c == '-');
        if let (Some(x), true, Some(y), Some(z)) = (tail(a), mid, trade_size(b), head(c)) {
            return Some([x, y, z]);
        }
    }
    None
}

/// A trade size: `1`, `3/4`, `1-1/4`, `1 1/4`, `1.25` → inches.
pub fn trade_size(s: &str) -> Option<f64> {
    let t = s.trim().trim_end_matches('"').trim();
    let frac = |f: &str| -> Option<f64> {
        let (n, d) = f.split_once('/')?;
        let (n, d): (f64, f64) = (n.trim().parse().ok()?, d.trim().parse().ok()?);
        (d > 0.0).then(|| n / d)
    };
    let v = if let Some((w, f)) = t.split_once(['-', ' ']).filter(|(_, f)| f.contains('/')) {
        w.trim().parse::<f64>().ok()? + frac(f)?
    } else if t.contains('/') {
        frac(t)?
    } else {
        t.parse::<f64>().ok()?
    };
    (v.is_finite() && v > 0.0 && v <= 120.0).then_some(v)
}

/// The size written just before byte `end` of `s` (a `"` there): `1`, `3/4`, `1-1/4`, `1 1/4`.
fn size_before(s: &str, end: usize) -> Option<f64> {
    let head = s.get(..end)?;
    let num: String =
        head.chars().rev().take_while(|c| c.is_ascii_digit() || *c == '.' || *c == '/' || *c == '-').collect::<Vec<_>>().into_iter().rev().collect();
    if num.is_empty() {
        return None;
    }
    // `1 1/4"`: a whole number before a fraction.
    if num.contains('/') && !num.contains('-') {
        let before = head.get(..head.len().saturating_sub(num.len()))?.trim_end();
        let whole: String = before.chars().rev().take_while(char::is_ascii_digit).collect::<Vec<_>>().into_iter().rev().collect();
        if !whole.is_empty() && before.len() < head.len().saturating_sub(num.len()) {
            return trade_size(&format!("{whole}-{num}"));
        }
    }
    trade_size(&num)
}

fn has_any(s: &str, words: &[&str]) -> bool {
    let padded = format!(" {s} ");
    words.iter().any(|w| padded.contains(w))
}

impl Calls {
    /// What a note (upper case) calls for.
    pub fn of(text: &str) -> Calls {
        let t = text.to_ascii_uppercase();
        let mut c = Calls::default();
        let ss = sentences(&t);
        // Boxes: a size and a box anywhere in the note (`… JUNCTION BOX SHALL BE 12"X12"X6".`).
        let box_word = has_any(&t, &[" BOX", "J-BOX", "JBOX"]);
        if box_word {
            let kind = if t.contains("BDA") && t.contains("ANTENNA") {
                "bda_antenna_box"
            } else if t.contains("BDA") && t.contains("RISER") {
                "bda_riser_box"
            } else if t.contains("FLOOR BOX") {
                "floor_box"
            } else if t.contains("PULL BOX") {
                "pull_box"
            } else {
                "junction_box"
            };
            let dims = dims3(&t);
            let provided = ss.iter().any(|s| has_any(s, &[" PROVIDE ", " INSTALL ", " FURNISH "]) && has_any(s, &[" BOX", "J-BOX", "JBOX"]));
            if dims.is_some() || provided {
                c.boxes.push(BoxCall { kind, dims_in: dims });
            }
        }
        for s in &ss {
            let padded = format!(" {s} ");
            // Conduit: a size, then within a few words a conduit word.
            for (i, _) in s.match_indices('"') {
                let Some(size) = size_before(s, i) else { continue };
                let after: Vec<&str> = s.get(i + 1..).unwrap_or_default().split_whitespace().take(4).collect();
                let conduit = after.iter().any(|w| {
                    let w = w.trim_matches(|c: char| !c.is_ascii_alphanumeric());
                    matches!(w, "CONDUIT" | "CONDUITS" | "EMT" | "PVC" | "RGS" | "IMC" | "GRC" | "RMC" | "FMC" | "C" | "RACEWAY" | "RACEWAYS")
                });
                let sleeve = after.iter().any(|w| w.starts_with("SLEEVE"));
                // Not a box's side (`8"X8"X6"`).
                if s.get(i + 1..).is_some_and(|r| r.trim_start().starts_with('X')) {
                    continue;
                }
                if sleeve {
                    c.sleeves.push(Some(size));
                    continue;
                }
                if !conduit {
                    continue;
                }
                let before: Vec<&str> = s.get(..i).unwrap_or_default().split_whitespace().rev().skip(1).take(2).collect();
                let count = before.iter().find_map(|w| count_word(w)).unwrap_or(1);
                c.conduits.push(ConduitCall {
                    size_in: size,
                    count,
                    pull_tape: has_any(&padded, &["PULL TAPE", "PULL STRING", "PULLSTRING", "PULLWIRE", "PULL WIRE", "PULL ROPE", "PULL CORD"]),
                    above_ceiling: padded.contains(" ABOVE ") && has_any(&padded, &["CEILING", "HARD LID", " LID "]),
                    hard_lid: padded.contains("HARD LID"),
                    corridor: has_any(&padded, &[" CORRIDOR", " CORR "]),
                    below: has_any(&padded, &["BELOW SLAB", "UNDER SLAB", "UNDERSLAB", "BELOW GRADE", "UNDERGROUND", "UNDER GROUND", "BELOW FLOOR"]),
                    riser: padded.contains("RISER") && (padded.contains("BETWEEN") || padded.contains(" UP ") || padded.contains("FLOORS")),
                    words: s.chars().take(QUOTE_CHARS).collect(),
                });
            }
            if padded.contains("SLEEVE") && !c.sleeves.iter().any(Option::is_some) && has_any(&padded, &[" PROVIDE ", " INSTALL "]) {
                c.sleeves.push(None);
            }
        }
        // A device the note provides, or names first (`CEILING OCCUPANCY SENSOR, …`).
        for (k, s) in ss.iter().enumerate() {
            let padded = format!(" {s} ");
            let verb = [" PROVIDE ", " INSTALL ", " FURNISH ", " MOUNT ", " LOCATE "].iter().filter_map(|v| padded.find(v)).min();
            let first = k == 0 && !NOT_AN_OBJECT.iter().any(|w| padded.trim_start().starts_with(w));
            if padded.contains(" BY OTHERS") || (verb.is_none() && !first) {
                continue;
            }
            let mut seen: BTreeSet<&str> = BTreeSet::new();
            for (word, kind) in DEVICES {
                let Some(at) = padded.find(&format!(" {word}")) else { continue };
                // `PROVIDE POWER TO THE DAMPER`: what it is connected to, not provided.
                let from = verb.unwrap_or(0);
                if at < from || padded.get(from..at).is_some_and(|x| x.contains(" TO ") || x.contains(" FOR ")) {
                    continue;
                }
                // The longer phrase wins (`DISCONNECT SWITCH` over `DISCONNECT`).
                if !seen.insert(kind) || c.devices.iter().any(|d| d.kind == kind) {
                    continue;
                }
                let count = padded.get(..at).and_then(|h| h.split_whitespace().last()).and_then(count_word).unwrap_or(1);
                c.devices.push(DeviceCall { kind, count });
            }
        }
        // The rating: `2-HOUR`, `2 HR`.
        for s in &ss {
            let padded = format!(" {s} ");
            if let Some(h) = hours(&padded) {
                let floors = padded.contains("FLOOR");
                let walls = padded.contains("WALL") || !floors;
                c.rating = Some(Rating { hours: h, floors, walls });
                break;
            }
        }
        c
    }

    pub fn is_empty(&self) -> bool {
        self.boxes.is_empty() && self.conduits.is_empty() && self.sleeves.is_empty() && self.devices.is_empty()
    }

    pub fn to_json(&self) -> Value {
        let mut o = serde_json::Map::new();
        if !self.boxes.is_empty() {
            o.insert("boxes".into(), json!(self.boxes.iter().map(|b| json!({"kind": b.kind, "size_in": b.dims_in})).collect::<Vec<_>>()));
        }
        if !self.conduits.is_empty() {
            o.insert(
                "conduit".into(),
                json!(
                    self.conduits
                        .iter()
                        .map(|c| json!({"size_in": c.size_in, "count": c.count, "pull_tape": c.pull_tape, "route": route_words(c), "riser": c.riser, "words": c.words}))
                        .collect::<Vec<_>>()
                ),
            );
        }
        if !self.sleeves.is_empty() {
            o.insert("sleeves".into(), json!(self.sleeves.iter().map(|s| json!({"size_in": s})).collect::<Vec<_>>()));
        }
        if !self.devices.is_empty() {
            o.insert("devices".into(), json!(self.devices.iter().map(|d| json!({"kind": d.kind, "count": d.count})).collect::<Vec<_>>()));
        }
        if let Some(r) = &self.rating {
            o.insert("fire_rating".into(), json!({"hours": r.hours, "floors": r.floors, "walls": r.walls}));
        }
        Value::Object(o)
    }
}

/// A rating in hours written in `s`: `2-HOUR`, `2 HOUR`, `2HR`, `1 HR`.
pub fn hours(s: &str) -> Option<f64> {
    let u = s.to_ascii_uppercase();
    for key in ["HOUR", "HR"] {
        for (i, _) in u.match_indices(key) {
            // `HR` inside a word (`THREE`, `CHROME`) is not one.
            if u.get(i + key.len()..).and_then(|r| r.chars().next()).is_some_and(|c| c.is_ascii_alphabetic() && c != 'S') {
                continue;
            }
            let head = u.get(..i).unwrap_or_default().trim_end().trim_end_matches('-').trim_end();
            let num: String = head.chars().rev().take_while(|c| c.is_ascii_digit() || *c == '.').collect::<Vec<_>>().into_iter().rev().collect();
            if let Some(h) = num.parse::<f64>().ok().filter(|h| h.is_finite() && *h > 0.0 && *h <= 4.0) {
                return Some(h);
            }
        }
    }
    None
}

fn route_words(c: &ConduitCall) -> String {
    let mut w = Vec::new();
    if c.riser {
        w.push("riser between the floors' boxes".to_string());
    }
    if c.above_ceiling {
        w.push(if c.hard_lid { "above the hard-lid ceiling" } else { "above the ceiling" }.to_string());
    }
    if c.corridor {
        w.push("in the corridor".into());
    }
    if c.below {
        w.push("below the slab".into());
    }
    w.join(", ")
}

// ---------------------------------------------------------------- reading the sheets

/// One text of a sheet (a line of an MText is one), upper case.
#[derive(Clone, Debug)]
struct Txt {
    text: String,
    at: [f64; 2],
    h: f64,
    handle: cadaddy_doc::Handle,
}

fn clean(s: &str) -> String {
    s.chars().take(MAX_NOTE_CHARS).collect::<String>().to_ascii_uppercase().split_whitespace().collect::<Vec<_>>().join(" ")
}

fn sheet_texts(d: &Drawing, rects: &[[f64; 4]]) -> Vec<Txt> {
    let mut out = Vec::new();
    for e in d.model.iter().take(MAX_ENTITIES) {
        if out.len() >= MAX_TEXTS {
            break;
        }
        let (lines, at, h): (Vec<String>, Vec2, f64) = match &e.kind {
            EntityKind::Text(t) => (vec![t.value.clone()], t.insert.xy(), t.height),
            EntityKind::MText(t) => {
                (cadaddy_fonts::plain_mtext(&t.contents).lines().take(MAX_NOTES).map(str::to_string).collect(), t.insert.xy(), t.height)
            }
            _ => continue,
        };
        let h = h.abs();
        if !(at.x.is_finite() && at.y.is_finite() && h.is_finite() && h > 0.0 && rects.iter().any(|r| inside(*r, [at.x, at.y]))) {
            continue;
        }
        for (i, l) in lines.iter().enumerate() {
            let text = clean(l);
            if !text.is_empty() {
                out.push(Txt { text, at: [at.x, at.y - i as f64 * h * 1.6], h, handle: e.handle });
            }
        }
    }
    out
}

/// A keynote list title.
fn is_title(t: &str) -> bool {
    t.chars().count() <= 80
        && ["KEYNOTES", "KEYED NOTES", "KEY NOTES", "KEYED PLAN NOTES", "KEYED SHEET NOTES"].iter().any(|k| t.contains(k))
        && !t.starts_with("SEE ")
        && !t.starts_with("REFER")
}

/// `3. TEXT`, `10.TEXT`, `3) TEXT` → (number, the rest); `3 TEXT` only when `3` is `next`.
fn entry_start(t: &str, next: Option<u32>) -> Option<(String, String)> {
    let digits: String = t.chars().take_while(char::is_ascii_digit).collect();
    if digits.is_empty() || digits.len() > 3 {
        return None;
    }
    let rest = t.get(digits.len()..)?;
    let n: u32 = digits.parse().ok().filter(|n| *n > 0)?;
    let body = if let Some(x) = rest.strip_prefix('.').or_else(|| rest.strip_prefix(')')).or_else(|| rest.trim_start().strip_prefix('.')) {
        x.trim_start()
    } else if rest.starts_with(' ') && next.is_none_or(|x| x == n) {
        rest.trim_start()
    } else {
        return None;
    };
    body.chars().next().filter(|c| c.is_ascii_alphabetic())?;
    Some((n.to_string(), body.to_string()))
}

/// One note of a keynote list.
#[derive(Clone, Debug)]
pub struct Note {
    pub number: String,
    pub text: String,
    pub calls: Calls,
}

/// A sheet's keynote list.
#[derive(Clone, Debug)]
pub struct List {
    pub sheet: String,
    pub title: String,
    /// Where it is on the sheet (model space): title and notes.
    pub rect: [f64; 4],
    /// Its words' height.
    pub body_h: f64,
    pub notes: Vec<Note>,
}

impl List {
    pub fn note(&self, number: &str) -> Option<&Note> {
        self.notes.iter().find(|n| n.number == number)
    }
    fn to_json(&self) -> Value {
        json!({
            "sheet": self.sheet,
            "title": self.title,
            "notes": self.notes.iter().map(|n| json!({"number": n.number, "text": n.text, "calls": n.calls.to_json()})).collect::<Vec<_>>(),
        })
    }
}

/// The notes under a title, read down its column.
fn read_list(sheet: &str, title: &Txt, texts: &[&Txt]) -> Option<List> {
    let th = title.h;
    // The first note: a number just under the title, at its left.
    let mut col: Vec<&Txt> = texts.iter().copied().filter(|t| t.at[1] < title.at[1] - 0.2 * th && t.at[0] >= title.at[0] - 3.0 * th).collect();
    col.sort_by(|a, b| b.at[1].total_cmp(&a.at[1]).then(a.at[0].total_cmp(&b.at[0])));
    let first = col.iter().find(|t| {
        t.at[1] >= title.at[1] - 10.0 * th
            && t.at[0] <= title.at[0] + 4.0 * th
            && (entry_start(&t.text, Some(1)).is_some() || t.text.chars().all(|c| c.is_ascii_digit()))
    })?;
    let bh = first.h;
    let x0 = first.at[0];
    let mut notes: Vec<(String, String)> = Vec::new();
    let mut used: BTreeSet<usize> = BTreeSet::new();
    let mut last_y = first.at[1] + bh * 1.5;
    let mut lo = [title.at[0], title.at[1] + th];
    let mut hi = lo;
    let mut grow_to = |p: [f64; 2], w: f64| {
        lo = [lo[0].min(p[0]), lo[1].min(p[1])];
        hi = [hi[0].max(p[0] + w), hi[1].max(p[1] + bh)];
    };
    for (i, t) in col.iter().enumerate() {
        if used.contains(&i) || t.at[1] > first.at[1] + 0.3 * bh {
            continue;
        }
        // In the note column (its left margin) only.
        if t.at[0] < x0 - 1.0 * bh || t.at[0] > x0 + 4.0 * bh {
            continue;
        }
        if last_y - t.at[1] > 4.5 * bh || t.h > 1.4 * bh || is_title(&t.text) || notes.len() > MAX_NOTES {
            break;
        }
        if t.h < 0.6 * bh {
            continue;
        }
        let next = notes.last().and_then(|(n, _)| n.parse::<u32>().ok()).map_or(1, |n| n + 1);
        let start = if t.text.chars().all(|c| c.is_ascii_digit()) && t.text.len() <= 3 {
            // A bare number (in a hexagon) with its words beside it.
            let body = col
                .iter()
                .enumerate()
                .filter(|(j, b)| {
                    *j != i && !used.contains(j) && (b.at[1] - t.at[1]).abs() <= 0.6 * bh && b.at[0] > t.at[0] && b.at[0] <= t.at[0] + 10.0 * bh
                })
                .min_by(|a, b| a.1.at[0].total_cmp(&b.1.at[0]));
            match body {
                Some((j, b)) => {
                    used.insert(j);
                    grow_to(b.at, 0.6 * b.h * b.text.chars().count() as f64);
                    Some((t.text.trim_start_matches('0').to_string(), b.text.clone()))
                }
                None => None,
            }
        } else {
            entry_start(&t.text, Some(next))
        };
        grow_to(t.at, 0.6 * t.h * t.text.chars().count() as f64);
        last_y = t.at[1];
        match start {
            Some((n, body)) if !n.is_empty() => notes.push((n, body)),
            _ => {
                if let Some((_, text)) = notes.last_mut()
                    && text.chars().count() < MAX_NOTE_CHARS
                {
                    text.push(' ');
                    text.push_str(&t.text);
                }
            }
        }
    }
    if notes.is_empty() {
        return None;
    }
    let mut seen = BTreeSet::new();
    let notes: Vec<Note> = notes
        .into_iter()
        .filter(|(n, _)| seen.insert(n.clone()))
        .take(MAX_NOTES)
        .map(|(number, text)| {
            let text: String = text.chars().take(MAX_NOTE_CHARS).collect();
            Note { calls: Calls::of(&text), number, text }
        })
        .collect();
    Some(List { sheet: sheet.to_string(), title: title.text.clone(), rect: [lo[0] - bh, lo[1] - bh, hi[0] + bh, hi[1] + bh], body_h: bh, notes })
}

/// A keynote marker, on its level's plan.
#[derive(Clone, Debug)]
pub struct Marker {
    pub sheet: String,
    /// Its list (index into [`Keynotes::lists`]).
    pub list: usize,
    pub number: String,
    /// Where it is on the sheet (model space) and how tall its number is.
    pub sheet_at: [f64; 2],
    pub h: f64,
    pub level: String,
    /// Building feet on its level's plan.
    pub at: [f64; 2],
    /// The view of the sheet it is in, when the sheet prints several.
    pub view: Option<String>,
}

/// The keynote lists and markers of the set.
#[derive(Clone, Debug, Default)]
pub struct Keynotes {
    pub lists: Vec<List>,
    pub markers: Vec<Marker>,
    /// Markers not carried onto a plan, by why.
    pub unplaced: BTreeMap<String, u64>,
}

/// A segment of linework.
type Seg = ([f64; 2], [f64; 2]);

/// The pieces of an entity's linework (curves as chords).
fn segments(e: &Entity, out: &mut Vec<Seg>) {
    let p2 = |v: cadaddy_geom::Vec3| [v.x, v.y];
    match &e.kind {
        EntityKind::Line(l) => out.push((p2(l.a), p2(l.b))),
        EntityKind::LwPolyline(pl) => {
            let v: Vec<[f64; 2]> = pl.vertices.iter().take(100_000).map(|x| [x.p.x, x.p.y]).collect();
            for w in v.windows(2) {
                if let [a, b] = w {
                    out.push((*a, *b));
                }
            }
            if pl.closed
                && let (Some(a), Some(b)) = (v.last(), v.first())
            {
                out.push((*a, *b));
            }
        }
        EntityKind::Polyline3d(pl) => {
            let v: Vec<[f64; 2]> = pl.points.iter().take(100_000).map(|x| [x.x, x.y]).collect();
            for w in v.windows(2) {
                if let [a, b] = w {
                    out.push((*a, *b));
                }
            }
            if pl.closed
                && let (Some(a), Some(b)) = (v.last(), v.first())
            {
                out.push((*a, *b));
            }
        }
        EntityKind::Circle(c) => arc_chords([c.center.x, c.center.y], c.radius, 0.0, std::f64::consts::TAU, out),
        EntityKind::Arc(a) => {
            let sweep = (a.end - a.start).rem_euclid(std::f64::consts::TAU);
            arc_chords([a.center.x, a.center.y], a.radius, a.start, if sweep <= 1e-9 { std::f64::consts::TAU } else { sweep }, out);
        }
        _ => {}
    }
}

fn arc_chords(c: [f64; 2], r: f64, start: f64, sweep: f64, out: &mut Vec<Seg>) {
    if !(r.is_finite() && r > 0.0 && start.is_finite() && sweep.is_finite()) {
        return;
    }
    let n = 16;
    let at = |i: usize| {
        let t = start + sweep * i as f64 / n as f64;
        [c[0] + r * t.cos(), c[1] + r * t.sin()]
    };
    for i in 0..n {
        out.push((at(i), at(i + 1)));
    }
}

/// Does the ray from `c` along `dir` meet segment `s` within `reach`?
fn ray_hits(c: [f64; 2], dir: [f64; 2], s: &Seg, reach: f64) -> bool {
    let (a, b) = s;
    let e = [b[0] - a[0], b[1] - a[1]];
    let den = dir[0] * e[1] - dir[1] * e[0];
    if den.abs() < 1e-12 {
        return false;
    }
    let w = [a[0] - c[0], a[1] - c[1]];
    let t = (w[0] * e[1] - w[1] * e[0]) / den;
    let u = (w[0] * dir[1] - w[1] * dir[0]) / den;
    t > 0.0 && t <= reach && (-1e-9..=1.0 + 1e-9).contains(&u)
}

/// Is `c` closed in by linework on all four sides within `reach`?
fn enclosed(c: [f64; 2], reach: f64, segs: &[Seg]) -> bool {
    [[1.0, 0.0], [-1.0, 0.0], [0.0, 1.0], [0.0, -1.0]].iter().all(|d| segs.iter().any(|s| ray_hits(c, *d, s, reach)))
}

/// The sheet's own trade's word in its keynote list titles.
fn trade_word(number: &str, discipline: Option<&str>) -> &'static str {
    let d = discipline.map(str::to_string).unwrap_or_else(|| cadaddy_mepfix::sheet_facts::discipline_of(number)).to_ascii_uppercase();
    match d.as_str() {
        "E" => "ELECTRICAL",
        "M" | "H" => "MECHANICAL",
        "P" => "PLUMBING",
        "FA" | "FP" | "F" => "FIRE",
        "T" | "LV" => "LOW VOLTAGE",
        "S" => "STRUCTURAL",
        _ => "ARCHITECTURAL",
    }
}

impl Keynotes {
    /// Every sheet's keynote lists and the markers on its plans.
    pub(crate) fn read(d: &Drawing, sp: &BuildingSpec, frame: &Frame) -> Keynotes {
        let mut out = Keynotes::default();
        // The sheets, once each (a sheet printing several levels has a link per view).
        let mut sheets: Vec<(String, [f64; 4], Option<String>)> = Vec::new();
        for l in &sp.sheets {
            let Some(r) = l.rect.map(norm_rect).filter(|r| r.iter().all(|v| v.is_finite())) else { continue };
            if !sheets.iter().any(|(n, sr, _)| n.eq_ignore_ascii_case(&l.number) && *sr == r) {
                sheets.push((l.number.clone(), r, l.discipline.clone()));
            }
        }
        sheets.sort_by(|a, b| a.0.cmp(&b.0).then(a.1[0].total_cmp(&b.1[0])));
        let rects: Vec<[f64; 4]> = sheets.iter().map(|s| s.1).collect();
        if rects.is_empty() {
            return out;
        }
        let texts = sheet_texts(d, &rects);
        let mut by_sheet: Vec<Vec<&Txt>> = vec![Vec::new(); sheets.len()];
        for t in &texts {
            if let Some(i) = sheets.iter().position(|s| inside(s.1, t.at))
                && let Some(v) = by_sheet.get_mut(i)
            {
                v.push(t);
            }
        }
        // Lists, and the sheet's own (its trade's, else its first).
        let mut own: Vec<Option<usize>> = vec![None; sheets.len()];
        for (si, (number, _, disc)) in sheets.iter().enumerate() {
            let Some(ts) = by_sheet.get(si) else { continue };
            let mut titles: Vec<&&Txt> = ts.iter().filter(|t| is_title(&t.text)).collect();
            titles.sort_by(|a, b| b.at[1].total_cmp(&a.at[1]).then(a.at[0].total_cmp(&b.at[0])));
            let first = out.lists.len();
            for t in titles {
                if out.lists.len() >= MAX_LISTS {
                    break;
                }
                if let Some(l) = read_list(number, t, ts) {
                    out.lists.push(l);
                }
            }
            let word = trade_word(number, disc.as_deref());
            let mine: Vec<usize> = (first..out.lists.len()).collect();
            own[si] = mine.iter().copied().find(|i| out.lists.get(*i).is_some_and(|l| l.title.contains(word))).or_else(|| mine.first().copied());
        }
        // Marker candidates: a number of the sheet's list, about as tall as its words, off the
        // list, at its text's middle.
        struct Cand {
            sheet: usize,
            list: usize,
            number: String,
            c: [f64; 2],
            h: f64,
            segs: Vec<Seg>,
            /// A circle round it, or a mask under it.
            tagged: bool,
        }
        let mut cands: Vec<Cand> = Vec::new();
        for (si, ts) in by_sheet.iter().enumerate() {
            let Some(li) = own.get(si).copied().flatten() else { continue };
            let Some(list) = out.lists.get(li) else { continue };
            let lists_here: Vec<[f64; 4]> = out.lists.iter().filter(|l| sheets.get(si).is_some_and(|s| s.0 == l.sheet)).map(|l| l.rect).collect();
            for t in ts {
                if cands.len() >= MAX_MARKERS {
                    break;
                }
                if t.text.is_empty() || t.text.len() > 3 || !t.text.chars().all(|c| c.is_ascii_digit()) {
                    continue;
                }
                let number = t.text.trim_start_matches('0').to_string();
                if list.note(&number).is_none() || t.h < 0.6 * list.body_h || t.h > 1.7 * list.body_h || lists_here.iter().any(|r| inside(*r, t.at)) {
                    continue;
                }
                let b = d.model.get(t.handle).map(|e| cadaddy_doc::entity_bounds(d, e, 0));
                let c = match b.filter(|b| !b.is_empty() && b.center().x.is_finite() && b.center().y.is_finite()) {
                    Some(b) => [b.center().x, b.center().y],
                    None => [t.at[0] + 0.3 * t.h * t.text.len() as f64, t.at[1] + 0.5 * t.h],
                };
                cands.push(Cand { sheet: si, list: li, number, c, h: t.h, segs: Vec::new(), tagged: false });
            }
        }
        if cands.is_empty() {
            return out;
        }
        // The linework round each candidate: one pass, through a grid of the candidates.
        let reach_of = |h: f64| 3.2 * h;
        let cell = cands.iter().map(|c| reach_of(c.h)).fold(0.0, f64::max).max(1e-6);
        let key = |x: f64, y: f64| ((x / cell).floor() as i64, (y / cell).floor() as i64);
        let mut grid: HashMap<(i64, i64), Vec<usize>> = HashMap::new();
        for (i, c) in cands.iter().enumerate() {
            grid.entry(key(c.c[0], c.c[1])).or_default().push(i);
        }
        let mut segs: Vec<Seg> = Vec::new();
        for e in d.model.iter().take(MAX_ENTITIES) {
            // A mask under the number (a tag blanking the linework behind it) marks it as a tag.
            if let EntityKind::Wipeout(w) = &e.kind {
                let (lo, hi) = w.boundary.iter().take(1_000).fold(([f64::INFINITY; 2], [f64::NEG_INFINITY; 2]), |(lo, hi), p| {
                    ([lo[0].min(p.x), lo[1].min(p.y)], [hi[0].max(p.x), hi[1].max(p.y)])
                });
                if !(lo.iter().chain(hi.iter()).all(|v| v.is_finite())) {
                    continue;
                }
                let k = key((lo[0] + hi[0]) / 2.0, (lo[1] + hi[1]) / 2.0);
                for dx in -1..=1 {
                    for dy in -1..=1 {
                        for i in grid.get(&(k.0 + dx, k.1 + dy)).into_iter().flatten() {
                            if let Some(c) = cands.get_mut(*i)
                                && inside([lo[0], lo[1], hi[0], hi[1]], c.c)
                                && (hi[0] - lo[0]).max(hi[1] - lo[1]) <= 8.0 * c.h
                            {
                                c.tagged = true;
                            }
                        }
                    }
                }
                continue;
            }
            if let EntityKind::Circle(ci) = &e.kind {
                let k = key(ci.center.x, ci.center.y);
                for dx in -1..=1 {
                    for dy in -1..=1 {
                        for i in grid.get(&(k.0 + dx, k.1 + dy)).into_iter().flatten() {
                            if let Some(c) = cands.get_mut(*i) {
                                let off = (ci.center.x - c.c[0]).hypot(ci.center.y - c.c[1]);
                                if ci.radius <= reach_of(c.h) && off < ci.radius * 0.9 {
                                    c.tagged = true;
                                }
                            }
                        }
                    }
                }
                continue;
            }
            segs.clear();
            segments(e, &mut segs);
            for s in &segs {
                let len = (s.1[0] - s.0[0]).hypot(s.1[1] - s.0[1]);
                if !len.is_finite() || len > 4.0 * cell {
                    continue;
                }
                let (a, b) = (key(s.0[0].min(s.1[0]), s.0[1].min(s.1[1])), key(s.0[0].max(s.1[0]), s.0[1].max(s.1[1])));
                for gx in a.0 - 1..=b.0 + 1 {
                    for gy in a.1 - 1..=b.1 + 1 {
                        for i in grid.get(&(gx, gy)).into_iter().flatten() {
                            if let Some(c) = cands.get_mut(*i)
                                && c.segs.len() < MAX_SEGS_PER_MARKER
                                && seg_dist(c.c, s.0, s.1).0 <= reach_of(c.h)
                            {
                                c.segs.push(*s);
                            }
                        }
                    }
                }
            }
        }
        // Each marker onto its level's plan.
        for c in cands {
            if !(c.tagged || enclosed(c.c, reach_of(c.h), &c.segs)) {
                continue;
            }
            let Some((number_s, _, _)) = sheets.get(c.sheet) else { continue };
            let links: Vec<&SheetLink> = sp.sheets.iter().filter(|l| l.number.eq_ignore_ascii_case(number_s)).collect();
            let in_view = links.iter().copied().find(|l| l.view.as_ref().is_some_and(|v| inside(norm_rect(v.rect), c.c)));
            let link = in_view.or_else(|| links.iter().copied().find(|l| l.view.is_none()));
            let Some(link) = link else {
                *out.unplaced.entry("in no view of its sheet that shows a level".into()).or_default() += 1;
                continue;
            };
            let Some(level) = link.level.clone().filter(|l| !l.trim().is_empty()) else {
                *out.unplaced.entry("its sheet shows no level".into()).or_default() += 1;
                continue;
            };
            // A view whose registration was fitted on another level's view of the sheet (it
            // names that level's plan) would put the marker on the wrong plan.
            let ref_level = link.registration.as_ref().and_then(|r| {
                sp.sheets.iter().find(|l| l.number.eq_ignore_ascii_case(&r.reference) && l.view.is_none()).and_then(|l| l.level.clone())
            });
            if ref_level.as_deref().is_some_and(|rl| !rl.eq_ignore_ascii_case(&level)) {
                *out.unplaced.entry("its registration places it on another level's plan (fitted on that level's view)".into()).or_default() += 1;
                continue;
            }
            let project = match &link.registration {
                Some(r) => r.to_project(c.c),
                // A sheet that is its level's plan itself.
                None if sp.level(&level).and_then(|l| l.sheet.as_deref()).is_some_and(|sh| sh.eq_ignore_ascii_case(number_s))
                    && frame.to_ft(Vec2::new(c.c[0], c.c[1])).0.eq_ignore_ascii_case(&level) =>
                {
                    c.c
                }
                None => {
                    *out.unplaced.entry("its sheet is not placed on its level's plan yet (bim.identity registers it)".into()).or_default() += 1;
                    continue;
                }
            };
            if !project.iter().all(|v| v.is_finite()) {
                continue;
            }
            let at = frame.to_ft_on(&level, Vec2::new(project[0], project[1]));
            let mut levels = vec![level];
            for l in &link.levels {
                if !levels.iter().any(|x| x.eq_ignore_ascii_case(l)) {
                    levels.push(l.clone());
                }
            }
            for level in levels {
                out.markers.push(Marker {
                    sheet: number_s.clone(),
                    list: c.list,
                    number: c.number.clone(),
                    sheet_at: [r2(c.c[0]), r2(c.c[1])],
                    h: c.h,
                    level,
                    at: [r2(at[0]), r2(at[1])],
                    view: link.view.as_ref().map(|v| v.title.clone()),
                });
            }
        }
        out.markers.sort_by(|a, b| {
            (a.level.as_str(), a.sheet.as_str(), a.number.as_str())
                .cmp(&(b.level.as_str(), b.sheet.as_str(), b.number.as_str()))
                .then(a.at[0].total_cmp(&b.at[0]))
                .then(a.at[1].total_cmp(&b.at[1]))
        });
        out
    }

    pub fn note_of(&self, m: &Marker) -> Option<&Note> {
        self.lists.get(m.list)?.note(&m.number)
    }
}

// ---------------------------------------------------------------- the model at a marker

/// A model element the prescreen looks at (kind resolved).
pub(crate) struct Thing<'a> {
    pub handle: &'a str,
    pub kind: &'a str,
    pub name: Option<&'a str>,
    pub mark: Option<&'a str>,
    pub level: &'a str,
    pub at: [f64; 2],
}

/// A piece of conduit: a spec MEP run of a conduit system or a run drawn as conduit.
#[derive(Clone, Debug)]
struct Piece {
    id: String,
    level: String,
    path: Vec<[f64; 2]>,
    size_in: Option<f64>,
    /// Its bottom, feet above its level's floor (none: not known).
    bottom_ft: Option<f64>,
    /// A riser's top, feet above its level's floor.
    top_ft: Option<f64>,
    pull_tape: Option<bool>,
    system: String,
    from: &'static str,
}

impl Piece {
    fn riser(&self) -> bool {
        self.top_ft.is_some()
    }
    fn dist(&self, p: [f64; 2]) -> f64 {
        match self.path.as_slice() {
            [] => f64::INFINITY,
            [a] => (a[0] - p[0]).hypot(a[1] - p[1]),
            more => more.windows(2).map(|w| if let [a, b] = w { seg_dist(p, *a, *b).0 } else { f64::INFINITY }).fold(f64::INFINITY, f64::min),
        }
    }
    fn to_json(&self) -> Value {
        json!({"id": self.id, "from": self.from, "system": self.system, "size_in": self.size_in, "bottom_ft": self.bottom_ft.map(r2), "top_ft": self.top_ft.map(r2), "path_ft": self.path.iter().take(20).map(|p| pt(*p)).collect::<Vec<_>>()})
    }
}

fn pieces(d: &Drawing, sp: &BuildingSpec, model: &Model, frame: &Frame) -> Vec<Piece> {
    let mut out = Vec::new();
    let conduit_systems: BTreeSet<String> =
        sp.mep.systems.iter().filter(|s| s.kind == MepKind::Conduit).map(|s| s.name.to_ascii_lowercase()).collect();
    for (i, r) in sp.mep.runs.iter().enumerate().take(200_000) {
        if !conduit_systems.contains(&r.system.to_ascii_lowercase()) {
            continue;
        }
        let path: Vec<[f64; 2]> = r.points.iter().take(10_000).map(|p| frame.to_ft_on(&r.level, Vec2::new(p[0], p[1]))).collect();
        if path.is_empty() || !path.iter().flatten().all(|v| v.is_finite()) {
            continue;
        }
        let bottom = r.bottoms_ft.iter().copied().filter(|b| b.is_finite()).fold(None, |m: Option<f64>, b| Some(m.map_or(b, |m| m.min(b))));
        out.push(Piece {
            id: if r.id.is_empty() { format!("mep.runs[{i}]") } else { r.id.clone() },
            level: r.level.clone(),
            path,
            size_in: r.size_in.first().copied().filter(|s| s.is_finite() && *s > 0.0),
            bottom_ft: bottom,
            top_ft: r.top_ft.filter(|t| t.is_finite()),
            pull_tape: None,
            system: r.system.clone(),
            from: "mep_run",
        });
    }
    let upf = frame.upf();
    for r in model.runs.iter() {
        let k = r.kind.to_ascii_lowercase();
        if !["conduit", "raceway", "emt"].iter().any(|w| k.contains(w)) {
            continue;
        }
        let size = ["size", "tradeSize", "conduitSize", "size_in"].iter().find_map(|f| r.fields.get(*f)).and_then(|s| {
            let t = s.to_ascii_uppercase();
            let num: String = t.chars().take_while(|c| c.is_ascii_digit() || matches!(c, '.' | '/' | '-' | ' ')).collect();
            trade_size(num.trim())
        });
        let pull = r
            .fields
            .iter()
            .find(|(k, _)| k.to_ascii_lowercase().contains("pull"))
            .map(|(_, v)| !matches!(v.trim().to_ascii_lowercase().as_str(), "no" | "false" | "0" | ""));
        // A drawn run's height: its own z (a flat 2D line at 0 says nothing).
        let floor = frame.floor_z(&r.level).unwrap_or(0.0);
        let ent = cadaddy_doc::Handle::parse_hex(&r.id).and_then(|h| d.model.get(h));
        let (bottom, top, path) = match ent.map(|e| &e.kind) {
            Some(EntityKind::Line(l)) if l.a.z != 0.0 || l.b.z != 0.0 => {
                let (z0, z1) = ((l.a.z.min(l.b.z) - floor) / upf, (l.a.z.max(l.b.z) - floor) / upf);
                let flat = (l.a.x - l.b.x).hypot(l.a.y - l.b.y) / upf;
                if flat < 0.1 && z1 - z0 > 1.0 {
                    (Some(z0), Some(z1), r.path.iter().take(1).copied().collect())
                } else {
                    (Some(z0), None, r.path.clone())
                }
            }
            Some(EntityKind::LwPolyline(pl)) if pl.elevation != 0.0 => (Some((pl.elevation - floor) / upf), None, r.path.clone()),
            _ => (None, None, r.path.clone()),
        };
        out.push(Piece {
            id: r.id.clone(),
            level: r.level.clone(),
            path,
            size_in: size,
            bottom_ft: bottom.filter(|b| b.is_finite()),
            top_ft: top.filter(|t| t.is_finite()),
            pull_tape: pull,
            system: r.fields.get("system").cloned().unwrap_or_default(),
            from: "drawn",
        });
    }
    out
}

/// A box's three sides from what is known of it: its record's size, else its built shape.
fn box_size(d: &Drawing, upf: f64, handle: &str, fields_size: Option<&str>) -> Option<([f64; 3], &'static str)> {
    if let Some(s) = fields_size.and_then(dims3) {
        return Some((s, "type"));
    }
    let e = cadaddy_doc::Handle::parse_hex(handle).and_then(|h| d.model.get(h))?;
    let store = cadaddy_doc::props::read(&e.common.xdata);
    for k in ["size", "box_size", "dimensions"] {
        if let Some(s) = store.get(k).map(|p| p.text()).and_then(|t| dims3(&t)) {
            return Some((s, "record"));
        }
    }
    if super::wallgen::made_as(e).is_some()
        && let EntityKind::Mesh(m) = &e.kind
        && !m.vertices.is_empty()
    {
        let (lo, hi) = m.vertices.iter().take(100_000).fold(([f64::INFINITY; 3], [f64::NEG_INFINITY; 3]), |(lo, hi), v| {
            ([lo[0].min(v.x), lo[1].min(v.y), lo[2].min(v.z)], [hi[0].max(v.x), hi[1].max(v.y), hi[2].max(v.z)])
        });
        let s = [(hi[0] - lo[0]) / upf * 12.0, (hi[1] - lo[1]) / upf * 12.0, (hi[2] - lo[2]) / upf * 12.0];
        if s.iter().all(|v| v.is_finite() && *v > 0.0) {
            return Some((s.map(r2), "shape"));
        }
    }
    None
}

fn same_size(a: [f64; 3], b: [f64; 3], tol: f64) -> bool {
    let mut x = a;
    let mut y = b;
    x.sort_by(f64::total_cmp);
    y.sort_by(f64::total_cmp);
    x.iter().zip(y).all(|(p, q)| (p - q).abs() <= tol)
}

fn size_words(s: [f64; 3]) -> String {
    format!("{}x{}x{}", inch_words(s[0]), inch_words(s[1]), inch_words(s[2]))
}

/// `a` or `an` before a size or a word.
fn a_or_an(next: &str) -> &'static str {
    let t = next.trim_start();
    if t.starts_with('8') || t.starts_with("11") || t.starts_with("18") || t.starts_with(['a', 'e', 'i', 'o', 'u']) { "an" } else { "a" }
}

fn inch_words(v: f64) -> String {
    let whole = v.trunc();
    let frac = v - whole;
    let eighths = (frac * 8.0).round() as i64;
    let w = whole as i64;
    let f = match eighths {
        0 | 8 => String::new(),
        2 => "1/4".into(),
        4 => "1/2".into(),
        6 => "3/4".into(),
        n => format!("{n}/8"),
    };
    let w = if eighths == 8 { w + 1 } else { w };
    match (w, f.is_empty()) {
        (0, false) => format!("{f}\""),
        (_, false) => format!("{w}-{f}\""),
        _ => format!("{w}\""),
    }
}

/// A wall type's fire rating, hours: its assembly's, else its name's (`2 HR CORRIDOR`).
fn wall_hours(sp: &BuildingSpec, wall_type: Option<&str>) -> Option<f64> {
    let t = wall_type?.trim();
    sp.wall_types
        .iter()
        .find(|w| w.name.eq_ignore_ascii_case(t))
        .and_then(|w| w.assembly.as_ref())
        .and_then(|a| a.fire_rating.as_deref().and_then(hours).or_else(|| hours(&a.description)))
        .or_else(|| hours(t))
}

/// Where two segments cross.
fn crossing(a: [f64; 2], b: [f64; 2], c: [f64; 2], e: [f64; 2]) -> Option<[f64; 2]> {
    let r = [b[0] - a[0], b[1] - a[1]];
    let s = [e[0] - c[0], e[1] - c[1]];
    let den = r[0] * s[1] - r[1] * s[0];
    if den.abs() < 1e-12 {
        return None;
    }
    let w = [c[0] - a[0], c[1] - a[1]];
    let t = (w[0] * s[1] - w[1] * s[0]) / den;
    let u = (w[0] * r[1] - w[1] * r[0]) / den;
    ((0.0..=1.0).contains(&t) && (0.0..=1.0).contains(&u)).then(|| [a[0] + r[0] * t, a[1] + r[1] * t])
}

/// What G-KEYNOTE found.
#[derive(Default)]
pub(crate) struct Checked {
    /// (rank, verdict, finding).
    pub findings: Vec<(u8, &'static str, Value)>,
    pub unmeasured: BTreeMap<String, u64>,
    /// Markers checked.
    pub markers: usize,
}

pub(crate) struct Ctx<'a> {
    pub d: &'a Drawing,
    pub sp: &'a BuildingSpec,
    pub model: &'a Model,
    /// What the documents say (the schedule rows name what a type mark is).
    pub prints: Option<&'a Prints>,
    pub frame: &'a Frame,
    pub areas: &'a [Area],
    pub whole_levels: bool,
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

fn tol(r: &Effective, name: &str, default: f64) -> f64 {
    r.rule.tolerance.get(name).copied().filter(|t| t.is_finite() && *t >= 0.0).unwrap_or(default)
}

fn quote(t: &str) -> String {
    let q: String = t.chars().take(QUOTE_CHARS).collect();
    if t.chars().count() > QUOTE_CHARS { format!("\"{q}…\"") } else { format!("\"{q}\"") }
}

/// Check every keynote marker in scope against the model (G-KEYNOTE).
pub(crate) fn check(cx: &Ctx, r: &Effective, things: &[Thing], fields_size: &HashMap<String, String>) -> Checked {
    let mut out = Checked::default();
    let kn = Keynotes::read(cx.d, cx.sp, cx.frame);
    for (why, n) in &kn.unplaced {
        *out.unmeasured.entry(format!("keynote marker not placed: {why}")).or_default() += n;
    }
    let reach = tol(r, "route_ft", DEFAULT_REACH_FT);
    let size_tol = tol(r, "size_in", DEFAULT_SIZE_IN);
    let upf = cx.frame.upf();
    let all_pieces = pieces(cx.d, cx.sp, cx.model, cx.frame);
    let elev = |l: &str| cx.sp.level(l).map(|x| x.elevation_ft);
    // Ceilings (feet on each level's plan) and corridors.
    let mut ceilings: HashMap<String, Vec<(Vec<[f64; 2]>, f64, Option<String>)>> = HashMap::new();
    for c in cx.sp.rcp.ceilings.iter().filter(|c| c.outline.len() >= 3 && c.height_ft.is_finite() && c.height_ft > 0.0) {
        let o: Vec<[f64; 2]> = c.outline.iter().take(10_000).map(|p| cx.frame.to_ft_on(&c.level, Vec2::new(p[0], p[1]))).collect();
        ceilings.entry(c.level.to_ascii_lowercase()).or_default().push((o, c.height_ft, c.ceiling_type.clone()));
    }
    let needs_areas = kn.notes_with(|c| c.conduits.iter().any(|x| x.corridor));
    let corridors: Vec<Area> = if needs_areas {
        super::validate_packet::model_areas(cx.d, cx.frame).into_iter().filter(|a| a.kind == "corridor").collect()
    } else {
        Vec::new()
    };
    let rank_of = |verdict: &str| match verdict {
        "pass" => 9u8,
        "unsure" => 5,
        _ => match r.rule.severity.as_str() {
            "high" => 0,
            "medium" => 1,
            _ => 2,
        },
    };
    // What each type mark is, as its schedule row and BIM type say (`XA` → `EXIT SIGN …`).
    let mut mark_text: HashMap<String, String> = HashMap::new();
    for row in cx.prints.map_or(&[][..], |p| p.schedule.as_slice()).iter().take(100_000) {
        let t = mark_text.entry(row.mark.trim().to_ascii_uppercase()).or_default();
        if t.len() < 2_000 {
            t.push(' ');
            t.push_str(&row.kind.clone().unwrap_or_default().to_ascii_uppercase().replace('_', " "));
            for v in row.fields.values().take(50) {
                t.push(' ');
                t.push_str(&v.to_ascii_uppercase());
            }
        }
    }
    for (mark, fields) in cx.model.types.iter().take(100_000) {
        let t = mark_text.entry(mark.trim().to_ascii_uppercase()).or_default();
        if t.len() < 2_000 {
            for v in fields.values().take(50) {
                t.push(' ');
                t.push_str(&v.to_ascii_uppercase());
            }
        }
    }
    // The riser markers, for where each riser goes.
    let riser_markers: Vec<&Marker> = kn.markers.iter().filter(|m| kn.note_of(m).is_some_and(|n| n.calls.conduits.iter().any(|c| c.riser))).collect();
    for m in &kn.markers {
        let Some(note) = kn.note_of(m) else { continue };
        if note.calls.is_empty() {
            continue;
        }
        let Some(area) = area_for(cx.areas, cx.whole_levels, &m.level, m.at) else { continue };
        out.markers += 1;
        let keynote = json!({"sheet": m.sheet, "number": m.number, "text": note.text, "view": m.view});
        let marker = json!({"sheet": m.sheet, "sheet_at": pt(m.sheet_at), "level": m.level, "at_ft": pt(m.at)});
        let cite = format!("keynote {} on {} {}", m.number, m.sheet, quote(&note.text));
        let push = |out: &mut Checked,
                    call: &str,
                    verdict: &'static str,
                    handle: Option<String>,
                    kind: &str,
                    issue: String,
                    expected: String,
                    found: String,
                    measured: Value| {
            let id = format!(
                "PS-{}",
                super::validate_run::hash8(&format!("{RULE}|{}|{}|{}|{:.1},{:.1}|{call}|{verdict}", m.sheet, m.number, m.level, m.at[0], m.at[1]))
            );
            let mut measured = measured;
            measured["keynote"] = keynote.clone();
            measured["marker"] = marker.clone();
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
                    "confidence": if verdict == "unsure" { 0.5 } else { 1.0 },
                    "severity": r.rule.severity,
                    "area": area.map(|a| a.id.clone()),
                    "level": m.level,
                    "at": pt(m.at),
                    "source": "prescreen",
                }),
            ));
        };
        let near: Vec<(&Thing, f64)> = things
            .iter()
            .filter(|t| t.level.eq_ignore_ascii_case(&m.level))
            .map(|t| (t, (t.at[0] - m.at[0]).hypot(t.at[1] - m.at[1])))
            .filter(|(_, dd)| *dd <= reach)
            .collect();
        let words_of = |t: &Thing, w: &str| -> bool {
            let w = w.replace('_', " ");
            let pad = |s: &str| format!(" {} ", s.to_ascii_lowercase().replace(['_', '-', '/'], " "));
            pad(t.kind).contains(&format!(" {w} ")) || t.name.is_some_and(|n| pad(n).contains(&format!(" {w} ")))
        };
        // A device kind, or a type mark whose schedule row or type names it (`EXIT SIGN`).
        let is_device = |t: &Thing, kind: &str| -> bool {
            words_of(t, kind)
                || t.kind.eq_ignore_ascii_case(kind)
                || t.mark.and_then(|m| mark_text.get(&m.trim().to_ascii_uppercase())).is_some_and(|text| {
                    let text = format!(" {} ", text.replace(['_', '-', '/', ',', '.'], " "));
                    DEVICES.iter().filter(|(_, k)| *k == kind).any(|(w, _)| text.contains(&format!(" {} ", w.replace('/', " "))))
                })
        };
        // Boxes.
        for (bi, b) in note.calls.boxes.iter().enumerate() {
            let call = format!("box{bi}");
            let label = match b.dims_in {
                Some(s) => format!("{} {}", size_words(s), b.kind.replace('_', " ")),
                None => b.kind.replace('_', " "),
            };
            let label = format!("{} {label}", a_or_an(&label));
            // The called kind, or a plain junction or pull box (any box the job didn't type further).
            let generic = |t: &Thing| {
                let k = t.kind.to_ascii_lowercase();
                matches!(k.as_str(), "box" | "jbox" | "j_box" | "junction_box" | "pull_box" | "junction box" | "pull box")
                    || t.mark.is_some_and(|mk| matches!(mk.to_ascii_uppercase().as_str(), "J" | "JB" | "JBOX" | "J-BOX"))
                    || t.name.is_some_and(|n| n.to_ascii_uppercase().contains("JUNCTION BOX"))
            };
            // Another size fails only for the called kind, or when the call is a plain box.
            let plain_call = matches!(b.kind, "junction_box" | "pull_box");
            let judged = |t: &Thing| plain_call || t.kind.eq_ignore_ascii_case(b.kind);
            let mut boxes: Vec<(&Thing, f64, Option<([f64; 3], &'static str)>)> = near
                .iter()
                .filter(|(t, _)| t.kind.eq_ignore_ascii_case(b.kind) || generic(t))
                .map(|(t, dd)| (*t, *dd, box_size(cx.d, upf, t.handle, fields_size.get(&t.handle.to_ascii_uppercase()).map(String::as_str))))
                .collect();
            boxes.sort_by(|a, b| a.1.total_cmp(&b.1).then(a.0.handle.cmp(b.0.handle)));
            let expected = format!("{label} within {reach} ft of the marker ({cite})");
            let size_ok = |s: &Option<([f64; 3], &'static str)>| match (b.dims_in, s) {
                (None, _) => Some(true),
                (Some(want), Some((got, _))) => Some(same_size(want, *got, size_tol)),
                (Some(_), None) => None,
            };
            let measured_of = |t: &Thing, dd: f64, s: &Option<([f64; 3], &'static str)>| json!({"call": {"box": b.kind, "size_in": b.dims_in}, "element": t.handle, "element_kind": t.kind, "distance_ft": r2(dd), "size_in": s.map(|x| x.0), "size_from": s.map(|x| x.1)});
            if let Some((t, dd, s)) = boxes.iter().find(|(_, _, s)| size_ok(s) == Some(true)) {
                let found = format!("{} {} ft from the marker{}", t.kind, r2(*dd), s.map(|x| format!(", {}", size_words(x.0))).unwrap_or_default());
                push(
                    &mut out,
                    &call,
                    "pass",
                    Some(t.handle.to_string()),
                    b.kind,
                    format!("{cite} calls for {label}: {found}"),
                    expected,
                    found,
                    measured_of(t, *dd, s),
                );
            } else if let Some((t, dd, s)) = boxes.iter().find(|(t, _, s)| judged(t) && size_ok(s) == Some(false)) {
                let got = s.map(|x| size_words(x.0)).unwrap_or_default();
                let found = format!("{} {got} {} {} ft from the marker", a_or_an(&got), t.kind, r2(*dd));
                push(
                    &mut out,
                    &call,
                    "fail",
                    Some(t.handle.to_string()),
                    b.kind,
                    format!("{cite} calls for {label}; the box there is {got}"),
                    expected,
                    found,
                    measured_of(t, *dd, s),
                );
            } else if let Some((t, dd, s)) = boxes.iter().find(|(_, _, s)| s.is_none()) {
                *out.unmeasured.entry(format!("{RULE}: a box at a keynote marker whose size nothing says")).or_default() += 1;
                let found = format!("{} {} ft from the marker, its size not known", t.kind, r2(*dd));
                push(
                    &mut out,
                    &call,
                    "unsure",
                    Some(t.handle.to_string()),
                    b.kind,
                    format!("{cite} calls for {label}; the box there has no size in its record or shape"),
                    expected,
                    found,
                    measured_of(t, *dd, s),
                );
            } else {
                // Plain boxes of other sizes there (a device's box) are not the called one.
                let others: Vec<Value> = boxes
                    .iter()
                    .take(10)
                    .map(|(t, dd, s)| json!({"element": t.handle, "kind": t.kind, "distance_ft": r2(*dd), "size_in": s.map(|x| x.0)}))
                    .collect();
                let found = if others.is_empty() {
                    format!("no box within {reach} ft of the marker")
                } else {
                    format!("no {} within {reach} ft of the marker ({} box(es) of other sizes there)", b.kind.replace('_', " "), others.len())
                };
                push(
                    &mut out,
                    &call,
                    "missing",
                    None,
                    b.kind,
                    format!("{cite} calls for {label}: {found}"),
                    expected,
                    found,
                    json!({"call": {"box": b.kind, "size_in": b.dims_in}, "searched_ft": reach, "other_boxes": others}),
                );
            }
        }
        // Devices and sleeves.
        let mut counted: Vec<(String, &'static str, u32, Option<f64>)> =
            note.calls.devices.iter().map(|dv| (format!("device:{}", dv.kind), dv.kind, dv.count, None)).collect();
        counted.extend(note.calls.sleeves.iter().enumerate().map(|(i, s)| (format!("sleeve{i}"), "sleeve", 1, *s)));
        for (call, kind, count, size) in counted {
            let mut hits: Vec<(&Thing, f64)> = near.iter().filter(|(t, _)| is_device(t, kind)).copied().collect();
            hits.sort_by(|a, b| a.1.total_cmp(&b.1).then(a.0.handle.cmp(b.0.handle)));
            let label = match size {
                Some(s) => format!("{} {} {}", count, inch_words(s), kind.replace('_', " ")),
                None => format!("{} {}", count, kind.replace('_', " ")),
            };
            let expected = format!("{label} within {reach} ft of the marker ({cite})");
            let mut measured = json!({"call": {"kind": kind, "count": count, "size_in": size}, "found": hits.iter().map(|(t, dd)| json!({"element": t.handle, "kind": t.kind, "distance_ft": r2(*dd)})).collect::<Vec<_>>(), "searched_ft": reach});
            if (hits.len() as u32) < count {
                // What is there instead (a type nothing names as this kind may be it).
                let mut others: Vec<&(&Thing, f64)> = near.iter().filter(|(t, _)| !is_device(t, kind)).collect();
                others.sort_by(|a, b| a.1.total_cmp(&b.1).then(a.0.handle.cmp(b.0.handle)));
                measured["nearest_others"] = json!(
                    others
                        .iter()
                        .take(8)
                        .map(|(t, dd)| json!({"element": t.handle, "kind": t.kind, "mark": t.mark, "distance_ft": r2(*dd)}))
                        .collect::<Vec<_>>()
                );
            }
            let found = format!("{} {} within {reach} ft", hits.len(), kind.replace('_', " "));
            let first = hits.first().map(|(t, _)| t.handle.to_string());
            if hits.len() as u32 >= count {
                push(&mut out, &call, "pass", first, kind, format!("{cite} calls for {label}: {found}"), expected, found, measured);
            } else {
                push(&mut out, &call, "missing", first, kind, format!("{cite} calls for {label}: {found}"), expected, found, measured);
            }
        }
        // Conduit.
        let lv = m.level.to_ascii_lowercase();
        let level_ceiling = cx.sp.level(&m.level).map(|l| l.ceiling_ft).filter(|c| c.is_finite() && *c > 0.0);
        let ceiling_at = |p: [f64; 2]| -> Option<(f64, Option<String>, &'static str)> {
            ceilings
                .get(&lv)
                .and_then(|cs| cs.iter().find(|(o, _, _)| point_in(o, p)))
                .map(|(_, h, ty)| (*h, ty.clone(), "reflected ceiling plan"))
                .or_else(|| level_ceiling.map(|h| (h, None, "the level's ceiling")))
        };
        for (ci, c) in note.calls.conduits.iter().enumerate() {
            let call = format!("conduit{ci}");
            let label = format!(
                "{}{} conduit{}",
                if c.count > 1 { format!("{} × ", c.count) } else { String::new() },
                inch_words(c.size_in),
                if c.pull_tape { " with pull tape" } else { "" }
            );
            let route = route_words(c);
            let expected = if route.is_empty() { format!("{label} at the marker ({cite})") } else { format!("{label}, {route} ({cite})") };
            // A riser of a level below that comes up through this floor is this marker's too.
            let reaches_here = |p: &Piece| -> bool {
                match (elev(&p.level), elev(&m.level)) {
                    (Some(e0), Some(e1)) if e1 > e0 => p.top_ft.is_some_and(|t| t + RISER_SLACK_FT >= e1 - e0),
                    _ => false,
                }
            };
            let here: Vec<(&Piece, f64)> = all_pieces
                .iter()
                .filter(|p| p.riser() == c.riser && (p.level.eq_ignore_ascii_case(&m.level) || (c.riser && reaches_here(p))))
                .map(|p| (p, p.dist(m.at)))
                .filter(|(_, dd)| *dd <= reach)
                .collect();
            // A note naming its system (BDA, fire alarm, telecom) wants that system's conduit:
            // another system's conduit there (a branch circuit's) is not it.
            let system = note_system(&note.text);
            let of_system = |p: &Piece| system.is_none_or(|w| p.system.to_ascii_uppercase().contains(w));
            let (here, other_systems): (Vec<(&Piece, f64)>, Vec<(&Piece, f64)>) = here.into_iter().partition(|(p, _)| of_system(p));
            let fits = |p: &Piece| p.size_in.is_some_and(|s| (s - c.size_in).abs() <= size_tol.max(0.01));
            let mut ok: Vec<&(&Piece, f64)> = here.iter().filter(|(p, _)| fits(p)).collect();
            ok.sort_by(|a, b| a.1.total_cmp(&b.1).then(a.0.id.cmp(&b.0.id)));
            let sizes: BTreeSet<String> = here.iter().map(|(p, _)| p.size_in.map_or("size not known".into(), inch_words)).collect();
            let Some((p, dd)) = ok.first().map(|x| (x.0, x.1)) else {
                if here.is_empty() {
                    let what = format!("{}{}conduit", system.map(|w| format!("{w} ")).unwrap_or_default(), if c.riser { "riser " } else { "" });
                    let found = if other_systems.is_empty() {
                        format!("no {what} within {reach} ft of the marker")
                    } else {
                        format!("no {what} within {reach} ft of the marker (only other systems' conduit there)")
                    };
                    let others: Vec<Value> =
                        other_systems.iter().take(10).map(|(p, dd)| json!({"piece": p.to_json(), "distance_ft": r2(*dd)})).collect();
                    push(
                        &mut out,
                        &call,
                        "missing",
                        None,
                        "conduit",
                        format!("{cite} calls for {label}: {found}"),
                        expected,
                        found,
                        json!({"call": c_json(c), "searched_ft": reach, "system": system, "other_conduit": others}),
                    );
                } else {
                    let found = format!("conduit of {} there", sizes.iter().cloned().collect::<Vec<_>>().join(", "));
                    let best = here.iter().min_by(|a, b| a.1.total_cmp(&b.1)).map(|(p, _)| p.id.clone());
                    push(
                        &mut out,
                        &call,
                        "fail",
                        best,
                        "conduit",
                        format!("{cite} calls for {label}; {found}"),
                        expected,
                        found,
                        json!({"call": c_json(c), "pieces": here.iter().take(10).map(|(p, dd)| json!({"piece": p.to_json(), "distance_ft": r2(*dd)})).collect::<Vec<_>>()}),
                    );
                }
                continue;
            };
            let mut measured = json!({"call": c_json(c), "piece": p.to_json(), "distance_ft": r2(dd), "pull_tape": p.pull_tape});
            let mut problems: Vec<String> = Vec::new();
            let mut unknown: Vec<String> = Vec::new();
            let mut penetrations: Vec<Value> = Vec::new();
            if c.riser && !p.level.eq_ignore_ascii_case(&m.level) {
                // A riser from below: its floors are listed at its own level's marker.
                measured["comes_from"] = json!({"level": p.level, "top_ft": p.top_ft.map(r2)});
            }
            if c.riser && p.level.eq_ignore_ascii_case(&m.level) {
                // Up to the next level with a riser marker by this one.
                let here_elev = elev(&m.level);
                let up = riser_markers
                    .iter()
                    .filter(|o| !o.level.eq_ignore_ascii_case(&m.level) && (o.at[0] - m.at[0]).hypot(o.at[1] - m.at[1]) <= 3.0 * reach)
                    .filter_map(|o| Some((o, elev(&o.level)? - here_elev?)))
                    .filter(|(_, rise)| *rise > 0.0)
                    .min_by(|a, b| a.1.total_cmp(&b.1));
                let top = p.top_ft.unwrap_or(0.0);
                let bottom = p.bottom_ft.unwrap_or(0.0);
                match up {
                    Some((o, rise)) => {
                        measured["goes_to"] = json!({"level": o.level, "rise_ft": r2(rise), "marker_at_ft": pt(o.at), "top_ft": r2(top)});
                        if top + RISER_SLACK_FT < rise {
                            problems.push(format!("the riser stops {} ft up; the riser box on {} is {} ft up", r2(top), o.level, r2(rise)));
                        }
                    }
                    None => measured["goes_to"] = json!(null),
                }
                // The floors it passes.
                if let Some(e0) = here_elev {
                    for l in &cx.sp.levels {
                        let rise = l.elevation_ft - e0;
                        if rise > bottom + 1e-6 && rise <= top + 1e-6 {
                            let need = note.calls.rating.as_ref().filter(|x| x.floors).map(|x| x.hours);
                            penetrations.push(json!({"assembly": "floor", "level": l.name, "at_ft": pt(m.at), "needs_hr": need, "from": if need.is_some() { "keynote" } else { "not known: the floor assembly's rating" }, "firestop": firestop(need)}));
                        }
                    }
                }
            } else if !c.riser {
                // Its route.
                let samples: Vec<[f64; 2]> = p
                    .path
                    .windows(2)
                    .flat_map(|w| {
                        let (a, b) = match w {
                            [a, b] => (*a, *b),
                            _ => ([0.0; 2], [0.0; 2]),
                        };
                        let n = ((a[0] - b[0]).hypot(a[1] - b[1]).ceil() as usize).clamp(1, 500);
                        (0..=n).map(move |i| {
                            let t = i as f64 / n as f64;
                            [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t]
                        })
                    })
                    .take(20_000)
                    .collect();
                if c.above_ceiling {
                    let near_marker: Vec<[f64; 2]> = samples.iter().copied().filter(|s| (s[0] - m.at[0]).hypot(s[1] - m.at[1]) <= reach).collect();
                    // The highest ceiling it passes over there (the reflected ceiling plan's on a tie).
                    let ceil = near_marker
                        .iter()
                        .filter_map(|s| ceiling_at(*s))
                        .max_by(|a, b| a.0.total_cmp(&b.0).then((a.2 != "the level's ceiling").cmp(&(b.2 != "the level's ceiling"))));
                    match (p.bottom_ft, &ceil) {
                        (Some(b), Some((h, ty, from))) => {
                            measured["ceiling"] = json!({"height_ft": r2(*h), "type": ty, "from": from, "conduit_bottom_ft": r2(b)});
                            if b + 0.01 < *h {
                                problems.push(format!("it runs at {} ft, under the {} ft ceiling ({from})", r2(b), r2(*h)));
                            }
                        }
                        (None, _) => unknown.push("its height is not known".into()),
                        (_, None) => unknown.push("the ceiling height there is not known".into()),
                    }
                }
                if c.below {
                    match p.bottom_ft {
                        Some(b) if b < 0.0 => {}
                        Some(b) => problems.push(format!("it runs {} ft above the floor, not below the slab", r2(b))),
                        None => unknown.push("its height is not known".into()),
                    }
                }
                if c.corridor {
                    let lv_corr: Vec<&Area> = corridors.iter().filter(|a| a.level.eq_ignore_ascii_case(&m.level)).collect();
                    if lv_corr.is_empty() {
                        unknown.push("the level has no corridor room to measure against".into());
                    } else {
                        let inside_n = samples.iter().filter(|s| lv_corr.iter().any(|a| point_in(&a.outline, **s))).count();
                        measured["in_corridor_share"] = json!(r2(inside_n as f64 / samples.len().max(1) as f64));
                        if inside_n == 0 {
                            problems.push("it does not run in the corridor".into());
                        }
                    }
                }
                // The rated walls it crosses.
                let note_walls = note.calls.rating.as_ref().filter(|x| x.walls).map(|x| x.hours);
                for w in cx.model.walls.iter().filter(|w| w.level.eq_ignore_ascii_case(&m.level)) {
                    if penetrations.len() >= MAX_CROSSINGS {
                        break;
                    }
                    let Some(x) = p.path.windows(2).find_map(|s| match s {
                        [a, b] => crossing(*a, *b, w.a, w.b),
                        _ => None,
                    }) else {
                        continue;
                    };
                    let wall_hr = wall_hours(cx.sp, w.wall_type.as_deref());
                    let need = match (wall_hr, note_walls) {
                        (Some(a), Some(b)) => Some(a.max(b)),
                        (a, b) => a.or(b),
                    };
                    if need.is_none() {
                        continue;
                    }
                    penetrations.push(json!({"assembly": "wall", "wall": w.id, "wall_type": w.wall_type, "wall_rating_hr": wall_hr, "needs_hr": need, "from": match (wall_hr, note_walls) { (Some(_), Some(_)) => "wall type and keynote", (Some(_), None) => "wall type", _ => "keynote" }, "at_ft": pt([r2(x[0]), r2(x[1])]), "firestop": firestop(need)}));
                }
            }
            measured["penetrations"] = json!(penetrations);
            let found = format!("{} conduit ({}) {} ft from the marker", p.size_in.map_or("?".into(), inch_words), p.id, r2(dd));
            let (verdict, issue) = if !problems.is_empty() {
                ("fail", format!("{cite} calls for {label}, {route}: {}", problems.join("; ")))
            } else if !unknown.is_empty() {
                *out.unmeasured.entry(format!("{RULE}: keynote conduit route not measured")).or_default() += 1;
                ("unsure", format!("{cite} calls for {label}, {route}: the size fits; the route is not measured ({})", unknown.join("; ")))
            } else {
                ("pass", format!("{cite} calls for {label}: {found}"))
            };
            measured["route_problems"] = json!(problems);
            measured["route_not_measured"] = json!(unknown);
            push(&mut out, &call, verdict, Some(p.id.clone()), "conduit", issue, expected, found, measured);
        }
    }
    out
}

/// The system a note names, as a run's system name holds it (`BDA`, `FIRE`, `TEL`, `DATA`).
fn note_system(text: &str) -> Option<&'static str> {
    let t = format!(" {} ", text.to_ascii_uppercase());
    [(" BDA ", "BDA"), ("FIRE ALARM", "FIRE"), (" TELCO", "TEL"), ("TELECOM", "TEL"), (" DATA ", "DATA")]
        .iter()
        .find(|(k, _)| t.contains(k))
        .map(|(_, w)| *w)
}

fn c_json(c: &ConduitCall) -> Value {
    json!({"size_in": c.size_in, "count": c.count, "pull_tape": c.pull_tape, "riser": c.riser, "route": route_words(c), "words": c.words})
}

fn firestop(hours: Option<f64>) -> String {
    match hours {
        Some(h) => format!("a listed firestop system keeping the {h}-hour rating"),
        None => "a listed firestop system for the assembly's rating".into(),
    }
}

impl Keynotes {
    fn notes_with(&self, f: impl Fn(&Calls) -> bool) -> bool {
        self.lists.iter().any(|l| l.notes.iter().any(|n| f(&n.calls)))
    }
}

// ---------------------------------------------------------------- packets

/// A crop a packet asks for: a keynote list, or the print round a marker.
#[derive(Clone, Debug)]
pub(crate) struct CropAsk {
    pub sheet: String,
    pub kind: &'static str,
    pub rect: [f64; 4],
    pub shows: String,
}

/// Markers cropped per packet, at most; lists carried.
const MAX_MARKER_CROPS: usize = 8;
const MAX_PACKET_LISTS: usize = 6;

/// What a packet carries of the keynotes: the lists of the sheets with markers in or by the area
/// (and of `sheets`, the plans cropped for it), the markers within `margin` feet of it, and the
/// crops to cut.
pub(crate) fn for_packet(kn: &Keynotes, area: &Area, margin: f64, sheets: &BTreeSet<String>) -> (Value, Vec<CropAsk>) {
    let reach = grow(area.bbox, margin);
    let here: Vec<&Marker> = kn.markers.iter().filter(|m| m.level.eq_ignore_ascii_case(&area.level) && inside(reach, m.at)).collect();
    let mut lists: BTreeSet<usize> = here.iter().map(|m| m.list).collect();
    for (i, l) in kn.lists.iter().enumerate() {
        if sheets.contains(&l.sheet.to_ascii_uppercase()) && lists.len() < MAX_PACKET_LISTS {
            lists.insert(i);
        }
    }
    let mut asks: Vec<CropAsk> = Vec::new();
    let markers: Vec<Value> = here
        .iter()
        .enumerate()
        .map(|(k, m)| {
            let note = kn.note_of(m);
            let shows = format!("keynote {} marker on {} at ({}, {})", m.number, m.sheet, r2(m.sheet_at[0]), r2(m.sheet_at[1]));
            let cropped = k < MAX_MARKER_CROPS;
            if cropped {
                let half = 15.0 * m.h;
                asks.push(CropAsk {
                    sheet: m.sheet.clone(),
                    kind: "keynote_marker",
                    rect: [m.sheet_at[0] - half, m.sheet_at[1] - half, m.sheet_at[0] + half, m.sheet_at[1] + half],
                    shows: shows.clone(),
                });
            }
            json!({
                "sheet": m.sheet,
                "number": m.number,
                "view": m.view,
                "level": m.level,
                "at_ft": pt(m.at),
                "sheet_at": pt(m.sheet_at),
                "in_area": point_in(&area.outline, m.at) || (area.outline.len() < 3 && inside(area.bbox, m.at)),
                "text": note.map(|n| n.text.clone()),
                "calls": note.map(|n| n.calls.to_json()),
                "crop_shows": if cropped { json!(shows) } else { Value::Null },
            })
        })
        .collect();
    // The lists of the sheets that mark the area after the markers (a packet's cap keeps the
    // markers first); the other plans' lists are in the JSON only.
    let marking: BTreeSet<usize> = here.iter().map(|m| m.list).collect();
    for i in marking.iter().take(MAX_PACKET_LISTS) {
        if let Some(l) = kn.lists.get(*i) {
            asks.push(CropAsk { sheet: l.sheet.clone(), kind: "keynotes", rect: l.rect, shows: format!("{} {}", l.sheet, l.title) });
        }
    }
    let v = json!({
        "about": "the keynote lists of the sheets that mark this area (number → note, and what each note calls for) and each keynote marker in or by it, carried onto the plan; a marker's crop is the print round it (`crop_shows` names it in the manifest). G-KEYNOTE in `prescreen` checked each call against the model at its marker",
        "lists": lists.iter().filter_map(|i| kn.lists.get(*i)).map(List::to_json).collect::<Vec<_>>(),
        "markers": markers,
    });
    (v, asks)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_bda_antenna_note_calls_for_its_box_conduit_and_rating() {
        let c = Calls::of(
            "ANTENNA SPOT FOR THE BDA SYSTEM. PROVIDE AN 8\"X8\"X6\" JUNCTION BOX HERE. RUN 1\" CONDUIT WITH PULL TAPE ABOVE THE HARD LID CEILING OF THE CORRIDOR. KEEP THE 2-HOUR RATING OF THE WALL.",
        );
        assert_eq!(c.boxes, vec![BoxCall { kind: "bda_antenna_box", dims_in: Some([8.0, 8.0, 6.0]) }]);
        assert_eq!(c.conduits.len(), 1, "{c:?}");
        let k = &c.conduits[0];
        assert_eq!(k.size_in, 1.0);
        assert!(k.pull_tape && k.above_ceiling && k.hard_lid && k.corridor && !k.riser && !k.below, "{k:?}");
        assert_eq!(c.rating, Some(Rating { hours: 2.0, floors: false, walls: true }));
        assert!(c.devices.is_empty(), "{c:?}");
    }

    #[test]
    fn a_riser_note_calls_for_a_riser_and_a_stub() {
        let c = Calls::of(
            "RISER BOX FOR THE BDA SYSTEM, 12\"X12\"X6\". RUN 2\" CONDUIT BETWEEN THE RISER BOXES OF EACH FLOOR. STUB 1\" CONDUIT INTO THE CORRIDOR ABOVE THE CEILING. KEEP THE 2-HOUR RATING OF FLOORS AND WALLS.",
        );
        assert_eq!(c.boxes, vec![BoxCall { kind: "bda_riser_box", dims_in: Some([12.0, 12.0, 6.0]) }]);
        assert_eq!(c.conduits.len(), 2, "{c:?}");
        assert!(c.conduits[0].riser && c.conduits[0].size_in == 2.0, "{c:?}");
        assert!(!c.conduits[1].riser && c.conduits[1].size_in == 1.0 && c.conduits[1].corridor && c.conduits[1].above_ceiling, "{c:?}");
        assert_eq!(c.rating, Some(Rating { hours: 2.0, floors: true, walls: true }));
    }

    #[test]
    fn notes_that_call_for_nothing_or_a_device() {
        for t in [
            "BRANCH CIRCUITS OF THE SUITE COME FROM ITS OWN PANEL. SEE THE ENLARGED PLANS.",
            "CONNECT TO THE UNIT'S BUILT-IN DISCONNECT. THE FAN RUNS ALL DAY.",
            "PROVIDE POWER TO THE DAMPER. CONNECT TO ITS OWN CIRCUIT.",
            "CIRCUIT TO THE BUILT-IN DISCONNECT SWITCH.",
        ] {
            assert!(Calls::of(t).is_empty(), "{t}: {:?}", Calls::of(t));
        }
        let c = Calls::of("PROVIDE ONE EXIT SIGN OVER THE DOOR, SEE THE FIXTURE SCHEDULE.");
        assert_eq!(c.devices, vec![DeviceCall { kind: "exit_sign", count: 1 }]);
        let c = Calls::of("CEILING OCCUPANCY SENSOR, AUTOMATIC ON AND OFF.");
        assert_eq!(c.devices, vec![DeviceCall { kind: "occupancy_sensor", count: 1 }]);
        let c = Calls::of("TWO (2) 4\" CONDUITS FOR THE PHONE SERVICE TO THE DATA ROOM. PROVIDE 3\" SLEEVE THROUGH WALL.");
        assert_eq!(c.conduits.len(), 1);
        assert_eq!((c.conduits[0].size_in, c.conduits[0].count), (4.0, 2));
        assert_eq!(c.sleeves, vec![Some(3.0)]);
        let c = Calls::of("TWO (2) 5\" SCH. 40 PVC CONDUITS, 36\" MIN. BELOW GRADE, WITH PULLWIRE.");
        assert_eq!(c.conduits.len(), 1, "{c:?}");
        assert!(c.conduits[0].size_in == 5.0 && c.conduits[0].count == 2 && c.conduits[0].pull_tape && c.conduits[0].below, "{c:?}");
    }

    #[test]
    fn sizes_read() {
        assert_eq!(trade_size("3/4"), Some(0.75));
        assert_eq!(trade_size("1-1/4\""), Some(1.25));
        assert_eq!(trade_size("1 1/2"), Some(1.5));
        assert_eq!(trade_size("2.5"), Some(2.5));
        assert_eq!(trade_size("x"), None);
        assert_eq!(dims3("4\"X4\"X2-1/8\" BOX"), Some([4.0, 4.0, 2.125]));
        assert_eq!(dims3("12 x 12 x 6"), Some([12.0, 12.0, 6.0]));
        assert_eq!(dims3("PROVIDE 1\" CONDUIT"), None);
        assert_eq!(hours("MAINTAIN 2-HOUR FIRE RATING"), Some(2.0));
        assert_eq!(hours("1 HR RATED CORRIDOR"), Some(1.0));
        assert_eq!(hours("THREE WAY CHROME"), None);
        let c = Calls::of("PROVIDE 1 1/4\" EMT HOMERUN.");
        assert_eq!(c.conduits.first().map(|c| c.size_in), Some(1.25), "{c:?}");
        assert_eq!(inch_words(0.75), "3/4\"");
        assert_eq!(inch_words(1.25), "1-1/4\"");
        assert_eq!(inch_words(2.0), "2\"");
    }

    #[test]
    fn list_entries_read() {
        assert_eq!(entry_start("3. PROVIDE A BOX", Some(3)), Some(("3".into(), "PROVIDE A BOX".into())));
        assert_eq!(entry_start("10.ROUTE TO ROOF", Some(4)), Some(("10".into(), "ROUTE TO ROOF".into())));
        assert_eq!(entry_start("4 CONNECT TO", Some(4)), Some(("4".into(), "CONNECT TO".into())));
        // A carried-on line starting with a number is not a note.
        assert_eq!(entry_start("40 PVC CONDUIT", Some(4)), None);
        assert_eq!(entry_start("12\"X12\"X6\". SEE", Some(8)), None);
        assert!(is_title("ELECTRICAL KEYNOTES: ( , , )") && !is_title("SEE KEYNOTES ON E-1"));
    }

    #[test]
    fn a_number_in_a_hexagon_is_enclosed_a_bare_one_is_not() {
        let hex: Vec<[f64; 2]> = (0..6).map(|i| f64::from(i) * std::f64::consts::PI / 3.0).map(|t| [10.0 * t.cos(), 10.0 * t.sin()]).collect();
        let segs: Vec<Seg> = (0..6).map(|i| (hex[i], hex[(i + 1) % 6])).collect();
        assert!(enclosed([0.0, 0.0], 15.0, &segs));
        // A leader on one side only.
        assert!(!enclosed([0.0, 0.0], 15.0, &[([5.0, -5.0], [5.0, 5.0])]));
    }
}
