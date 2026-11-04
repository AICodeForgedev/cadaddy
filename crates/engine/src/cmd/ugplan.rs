//! The underground plan's work (Q6148): what the documents call for below the slab or in the
//! ground, by trade, where it goes, and what CADaddy proposes there.
//!
//! - [`callouts`]: a sheet's notes, keynotes and legend line types calling for buried work (the
//!   words of `ugplan.json`: below grade, under the slab, and work out to the site, which is in
//!   the ground by nature), each with its trade, what it is and what its words give (count, size,
//!   material, least cover, steel elbows, the room it goes to).
//! - [`place`]: the called work CADaddy can locate from the model: a service from its utility pad
//!   to the service gear, telecom service conduits to their room, an EV charger's feed out of its
//!   panel; each end inside the building on its gear or room, the site end by rule outside the
//!   slab edge nearest it (the site plan isn't registered to the building).
//! - [`stack_bases`]: the drainage stacks of the slab level carried below the floor to the
//!   building drain.
//! - [`slab_drains`]: the slab level's horizontal drainage, read off its plans with no height of
//!   its own, under its floor at the building drain's invert (not in its ceiling).
//! - [`weigh`]: each feed whose two ends are on the slab level, under the slab against overhead,
//!   with its reasons from the documents and the rules, and its alternative.
//!
//! Every value comes from the documents or from `ugplan.json` (rules data, each with its source).

use std::collections::BTreeMap;

use cadaddy_mepfix::sheet_facts::TextItem;
use cadaddy_walk3d::{BuildingSpec, MepEquipment, MepRun};
use serde::Deserialize;
use serde_json::{Value, json};

/// The system of electrical work below the slab or in the ground.
pub(crate) const E_UG: &str = "E-UG";
/// The systems of plumbing below the slab: `P-UG-<its service>` (`P-UG-SAN`).
pub(crate) const P_UG: &str = "P-UG";

const RULES: &str = include_str!("ugplan.json");

/// Callouts one sheet gives, at most (hostile input).
const MAX_CALLOUTS: usize = 200;
/// Runs, feeders and circuits looked at, at most.
const MAX_ITEMS: usize = 5_000;
/// A spec's runs scanned, at most: the cap on what is worked on ([`MAX_ITEMS`]) counts the
/// runs that qualify (a large job's drains and stacks come after thousands of other runs).
const MAX_SCAN: usize = 200_000;
/// Characters of a callout kept.
const MAX_TEXT: usize = 600;

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub(crate) struct Kind {
    pub what: String,
    pub trade: String,
    pub words: Vec<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub(crate) struct Legend {
    pub below_words: Vec<String>,
    pub max_chars: usize,
    pub abbr_reach_h: f64,
    pub source: String,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub(crate) struct Depths {
    pub conduit_under_slab_ft: f64,
    pub conduit_source: String,
    pub service_cover_in: f64,
    pub service_source: String,
    pub drain_invert_ft: f64,
    pub drain_source: String,
    /// Why the slab level's untagged horizontal drains run under its floor ([`slab_drains`]).
    pub slab_drain_source: String,
    pub water_ft: f64,
    pub water_source: String,
    pub sump_discharge_in: f64,
    pub sump_source: String,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub(crate) struct Site {
    pub stub_ft: f64,
    pub source: String,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub(crate) struct Stubup {
    pub conduit_top_ft: f64,
    pub telecom_top_ft: f64,
    pub source: String,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub(crate) struct ProposalRules {
    pub parallel_min: u32,
    pub big_conduit_in: f64,
    pub under_cheaper: f64,
    pub branch_min_amps: f64,
    /// A row's load whose name has one of these is an elevator's: its hoistway.
    pub hoistway_words: Vec<String>,
    pub code: Vec<String>,
    pub source: String,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub(crate) struct Sleeve {
    pub oversize_in: f64,
    pub item: String,
    pub source: String,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub(crate) struct BearingPlane {
    pub angle_deg: f64,
    pub source: String,
}

/// `ugplan.json`.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub(crate) struct Rules {
    pub site_words: Vec<String>,
    /// A note asks for work when it has one of these (a schedule row or a legend's name doesn't).
    pub action_words: Vec<String>,
    /// Rooms telecom service conduits go to when the note names none.
    pub telecom_rooms: Vec<String>,
    pub site_source: String,
    pub trade_words: BTreeMap<String, Vec<String>>,
    pub kinds: Vec<Kind>,
    pub legend: Legend,
    pub depths: Depths,
    pub site: Site,
    pub stubup: Stubup,
    pub proposal: ProposalRules,
    pub sleeve: Sleeve,
    pub bearing_plane: BearingPlane,
}

pub(crate) fn rules() -> &'static Rules {
    static R: std::sync::OnceLock<Rules> = std::sync::OnceLock::new();
    R.get_or_init(|| serde_json::from_str(RULES).unwrap_or_default())
}

fn has_any(up: &str, words: &[String]) -> bool {
    words.iter().any(|w| !w.is_empty() && up.contains(w.as_str()))
}

// ---------------- detect ----------------

/// Buried work one note, keynote or legend line type calls for.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Callout {
    /// `E` or `P`.
    pub trade: String,
    pub sheet: String,
    /// `note`, `keynote` or `legend`.
    pub from: &'static str,
    /// Its number in its list (a keynote's), or a legend line type's abbreviation.
    pub mark: Option<String>,
    pub text: String,
    /// What it is ([`Kind::what`]), `other` when no kind's words are in it.
    pub what: String,
    /// In the ground by its own words (`BELOW GRADE`), not only for going to the site.
    pub said_buried: bool,
    pub count: u32,
    pub size_in: Option<f64>,
    pub material: String,
    /// The least cover the note asks for, inches.
    pub cover_in: Option<f64>,
    /// Steel elbows and risers.
    pub steel_ells: bool,
    /// The room it goes to (`… TO DATA ROOM`).
    pub room: Option<String>,
}

impl Callout {
    pub(crate) fn json(&self) -> Value {
        json!({
            "trade": self.trade, "sheet": self.sheet, "from": self.from, "mark": self.mark, "text": self.text, "what": self.what,
            "said_buried": self.said_buried, "count": self.count, "size_in": self.size_in, "material": self.material,
            "cover_in": self.cover_in, "steel_ells": self.steel_ells, "room": self.room,
        })
    }
    /// How it is cited: `E-101 keynote 4`, `P-001 legend CWUS`, `E-501 note`.
    pub(crate) fn cite(&self) -> String {
        match (&self.mark, self.from) {
            (Some(m), "legend") => format!("{} legend {m}", self.sheet),
            (Some(m), _) => format!("{} keynote {m}", self.sheet),
            (None, f) => format!("{} {f}", self.sheet),
        }
    }
}

/// A count in words or digits: `TWO`, `(2)`, `2`.
fn count_word(w: &str) -> Option<u32> {
    let w = w.trim_matches(|c: char| !c.is_ascii_alphanumeric());
    let n = match w {
        "ONE" => 1,
        "TWO" => 2,
        "THREE" => 3,
        "FOUR" => 4,
        "FIVE" => 5,
        "SIX" => 6,
        "SEVEN" => 7,
        "EIGHT" => 8,
        "NINE" => 9,
        "TEN" => 10,
        _ => w.parse::<u32>().ok()?,
    };
    (1..=50).contains(&n).then_some(n)
}

/// A size word in inches: `5"`, `1-1/2"`, `24"`.
fn inches(w: &str) -> Option<f64> {
    let t = w.trim_start_matches('(').trim_end_matches([',', '.', ';', ')']);
    let t = t.strip_suffix('"')?;
    // `1-1/2`, `3/4`, `48`, `4.5`.
    let frac = |v: &str| -> Option<f64> {
        match v.split_once('/') {
            Some((a, b)) => {
                let (a, b) = (a.parse::<f64>().ok()?, b.parse::<f64>().ok()?);
                (b > 0.0).then(|| a / b)
            }
            None => v.parse::<f64>().ok(),
        }
    };
    let v = match t.split_once('-') {
        Some((whole, f)) if f.contains('/') => whole.parse::<f64>().ok()? + frac(f)?,
        _ => frac(t)?,
    };
    (v.is_finite() && v > 0.0 && v <= 120.0).then_some(v)
}

/// What a callout's words give.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Parsed {
    pub count: Option<u32>,
    pub size_in: Option<f64>,
    pub material: String,
    pub cover_in: Option<f64>,
    pub steel_ells: bool,
    pub room: Option<String>,
}

/// Read a callout's words: `TWO (2) 5" SCH. 40 PVC CONDUIT WITH STEEL ELLS AND RISERS, MIN. 48"
/// BELOW GRADE` → 2, 5", PVC Sch 40, 48" cover, steel elbows.
pub(crate) fn parse(text: &str) -> Parsed {
    let up = text.to_ascii_uppercase().replace(['\u{201d}', '\u{2033}'], "\"");
    let words: Vec<&str> = up.split_whitespace().take(400).collect();
    // `(2)` first (the digits a count in words repeats), else a count word before the size.
    let mut p = Parsed {
        count: words.iter().find(|w| w.starts_with('(') && w.trim_end_matches([',', '.']).ends_with(')')).and_then(|w| count_word(w)),
        ..Parsed::default()
    };
    for (i, w) in words.iter().enumerate() {
        let Some(v) = inches(w) else { continue };
        let next: Vec<&str> = words.iter().skip(i + 1).take(4).copied().collect();
        let first = next.first().copied().unwrap_or_default();
        if first.starts_with("BELOW") || first.starts_with("COVER") || first.starts_with("DEEP") {
            // `48" BELOW GRADE`: a depth; the least one asked for is kept.
            p.cover_in = Some(p.cover_in.map_or(v, |c: f64| c.max(v)));
            continue;
        }
        if p.size_in.is_none()
            && next
                .iter()
                .any(|n| ["CONDUIT", "PVC", "SCH", "RGS", "RMC", "PIPE", "EMT", "CONT"].iter().any(|k| n.trim_start_matches('(').starts_with(k)))
        {
            p.size_in = Some(v);
            if p.count.is_none() {
                p.count = i.checked_sub(1).and_then(|j| words.get(j)).and_then(|w| count_word(w));
            }
        }
    }
    // `MINIMUM OF 24" BELOW`: the depth's number comes before BELOW, read above.
    let sch = |n: &str| up.contains(&format!("SCH. {n}")) || up.contains(&format!("SCH {n}")) || up.contains(&format!("SCHEDULE {n}"));
    // PVC first: `PVC CONDUIT WITH RGS ELBOWS` is PVC with steel elbows.
    p.material = if sch("80") {
        "PVC Sch 80".into()
    } else if sch("40") || up.contains("PVC") {
        "PVC Sch 40".into()
    } else if up.contains("RGS") || up.contains("RIGID GALV") || up.contains("RMC") {
        "RGS".into()
    } else if up.contains("TYPE 'K'") || up.contains("TYPE K") {
        "Copper Type K".into()
    } else {
        String::new()
    };
    p.steel_ells = ["STEEL ELL", "STEEL ELBOW", "RGS ELBOW", "RGS ELL", "RIGID ELBOW"].iter().any(|k| up.contains(k));
    // `… TO DATA ROOM …`: the word(s) between TO and ROOM.
    if let Some(i) = words.iter().position(|w| w.trim_end_matches([',', '.']) == "ROOM").filter(|i| *i > 0) {
        let to = words.iter().take(i).rposition(|w| *w == "TO");
        if let Some(j) = to.filter(|j| i - j <= 3) {
            let name = words
                .get(j + 1..i)
                .map(|v| v.iter().filter(|w| !matches!(**w, "THE" | "A" | "AN")).copied().collect::<Vec<_>>().join(" "))
                .unwrap_or_default();
            if !name.is_empty() {
                p.room = Some(name);
            }
        }
    }
    p
}

/// The kind a callout's words say, and its trade.
fn kind_of(up: &str, r: &Rules) -> Option<(&'static str, String)> {
    r.kinds.iter().find(|k| has_any(up, &k.words)).map(|k| (leak_what(&k.what), k.trade.clone()))
}

/// The kinds' names as `'static` (the table is static data).
fn leak_what(what: &str) -> &'static str {
    match what {
        "service_primary" => "service_primary",
        "service_secondary" => "service_secondary",
        "telecom" => "telecom",
        "floor_box" => "floor_box",
        "ev_charger" => "ev_charger",
        "site_power" => "site_power",
        "water" => "water",
        "sanitary" => "sanitary",
        "vent" => "vent",
        "storm" => "storm",
        _ => "other",
    }
}

/// A sheet number's trade: its discipline letters (`E-101` electrical, `P-201` plumbing).
pub(crate) fn sheet_trade(sheet: &str) -> Option<&'static str> {
    let t = sheet.trim();
    let disc: String = t.chars().take_while(|c| c.is_ascii_alphabetic()).collect();
    // Letters then a number (`E-101`, `P2.01`, `E101`).
    if !t.get(disc.len()..).unwrap_or_default().trim_start_matches(['-', '.', ' ']).starts_with(|c: char| c.is_ascii_digit()) {
        return None;
    }
    match disc.to_ascii_uppercase().as_str() {
        "E" | "ES" | "ED" | "EP" | "EL" | "T" | "TN" => Some("E"),
        "P" | "PL" => Some("P"),
        _ => None,
    }
}

/// The trade of a callout: its kind's, else its words', else its sheet's.
fn trade_of(up: &str, sheet: &str, kind: Option<&str>, r: &Rules) -> Option<String> {
    if let Some(k) = kind {
        return Some(k.to_string());
    }
    let by_words: Vec<&String> = r.trade_words.iter().filter(|(_, w)| has_any(up, w)).map(|(t, _)| t).collect();
    match by_words.as_slice() {
        [one] => Some((*one).clone()),
        _ => sheet_trade(sheet).map(str::to_string).or_else(|| by_words.first().map(|t| (*t).clone())),
    }
}

/// An abbreviation as a legend draws it with its line: 2 to 6 capitals (and digits), not a
/// number or a sheet's grid label.
fn abbreviation(v: &str) -> bool {
    let t = v.trim();
    (2..=6).contains(&t.len()) && t.chars().all(|c| c.is_ascii_uppercase() || c.is_ascii_digit()) && t.chars().any(|c| c.is_ascii_uppercase())
}

/// The callouts one sheet gives: notes and keynotes calling for buried work (by their words, or
/// for going out to the site), and its legend's line types for work below the slab.
pub(crate) fn callouts(sheet: &str, texts: &[TextItem]) -> Vec<Callout> {
    let r = rules();
    let mut out: Vec<Callout> = Vec::new();
    // The legend first: a short description saying below the slab, its abbreviation drawn beside
    // it (`CWUS  COLD WATER PIPING (UNDER SLAB)`).
    let lg = &r.legend;
    for t in texts.iter().take(200_000) {
        if out.len() >= MAX_CALLOUTS {
            break;
        }
        let up = t.value.trim().to_ascii_uppercase();
        if up.len() > lg.max_chars || t.rotation.abs() > 0.01 || !t.height.is_finite() || t.height <= 0.0 || !has_any(&up, &lg.below_words) {
            continue;
        }
        // A note's sentence isn't a legend's line type.
        if up.split_whitespace().count() > 8 || has_any(&up, &r.action_words) || up.contains("SHALL") {
            continue;
        }
        let reach = lg.abbr_reach_h * t.height;
        let near: Vec<(f64, f64, String)> = texts
            .iter()
            .filter(|o| abbreviation(&o.value) && o.rotation.abs() <= 0.01)
            .map(|o| ((o.at - t.at).len(), (o.at.y - t.at.y).abs(), o.value.trim().to_string()))
            .filter(|(d, _, _)| d.is_finite() && *d <= reach)
            .collect();
        // An abbreviation starts as its description does (`CWUS`, `COLD WATER …`); the one on
        // its row first (a legend's rows are its line types); then the nearest.
        let initial = up.trim_start_matches(|c: char| !c.is_ascii_alphanumeric()).chars().next();
        let abbr = near
            .iter()
            .min_by(|a, b| {
                let key = |x: &(f64, f64, String)| (x.2.chars().next() != initial, x.1 > 0.8 * t.height);
                key(a).cmp(&key(b)).then(a.0.total_cmp(&b.0))
            })
            .map(|(_, _, v)| v.clone());
        // A line type is drawn with its abbreviation.
        if abbr.is_none() {
            continue;
        }
        let kind = kind_of(&up, r);
        let Some(trade) = trade_of(&up, sheet, kind.as_ref().map(|k| k.1.as_str()), r) else { continue };
        let c = Callout {
            trade,
            sheet: sheet.to_string(),
            from: "legend",
            mark: abbr,
            text: t.value.trim().to_string(),
            what: kind.map_or("other", |k| k.0).to_string(),
            said_buried: true,
            count: 1,
            ..Callout::default()
        };
        if !out.iter().any(|o| o.text == c.text) {
            out.push(c);
        }
    }
    let legend_texts: Vec<String> = out.iter().map(|c| c.text.to_ascii_uppercase()).collect();
    let site = |t: &str| has_any(&t.to_ascii_uppercase(), &r.site_words);
    for (mark, text) in super::keynotes::notes_matching(texts, |t| super::keynotes::buried(t) || site(t)) {
        if out.len() >= MAX_CALLOUTS {
            break;
        }
        let text: String = text.chars().take(MAX_TEXT).collect();
        let up = text.to_ascii_uppercase();
        if legend_texts.iter().any(|l| up.trim() == l.as_str()) {
            continue;
        }
        let said_buried = super::keynotes::buried(&up);
        let kind = kind_of(&up, r);
        let p = parse(&text);
        // Work out to the site is in the ground only when it names something buried work is, and
        // asks for it (a schedule row or a legend's name, `CAR CHARGER`, asks nothing).
        if !said_buried && (kind.is_none() || !(has_any(&up, &r.action_words) || p.size_in.is_some())) {
            continue;
        }
        // A legend's name or a schedule's heading (a few words, no request, no size) isn't a note.
        if up.split_whitespace().count() < 4 && !has_any(&up, &r.action_words) && p.size_in.is_none() {
            continue;
        }
        let Some(trade) = trade_of(&up, sheet, kind.as_ref().map(|k| k.1.as_str()), r) else { continue };
        let c = Callout {
            trade,
            sheet: sheet.to_string(),
            from: if mark.is_some() { "keynote" } else { "note" },
            mark,
            what: kind.map_or("other", |k| k.0).to_string(),
            said_buried,
            count: p.count.unwrap_or(1),
            size_in: p.size_in,
            material: p.material,
            cover_in: p.cover_in,
            steel_ells: p.steel_ells,
            room: p.room,
            text,
        };
        if !out.iter().any(|o| o.text == c.text) {
            out.push(c);
        }
    }
    out
}

/// Whether a sheet's view title is a plan of buried work for `trade` (its sheet's trade, or a
/// title naming the trade).
pub(crate) fn plan_title_trade(sheet: &str, title: &str) -> Option<&'static str> {
    let up = title.to_ascii_uppercase();
    sheet_trade(sheet).or(if up.contains("ELECTRICAL") || up.contains("POWER") {
        Some("E")
    } else if up.contains("PLUMBING") || up.contains("SANITARY") || up.contains("WATER") {
        Some("P")
    } else {
        None
    })
}

// ---------------- the model's places ----------------

fn upf(sp: &BuildingSpec) -> f64 {
    if sp.units_per_foot.is_finite() && sp.units_per_foot > 0.0 { sp.units_per_foot } else { 12.0 }
}

/// The slab level: the lowest storey (the first, when none has an elevation).
pub(crate) fn slab_level(sp: &BuildingSpec) -> Option<String> {
    sp.levels
        .iter()
        .filter(|l| l.elevation_ft.is_finite())
        .min_by(|a, b| a.elevation_ft.total_cmp(&b.elevation_ft))
        .or_else(|| sp.levels.first())
        .map(|l| l.name.clone())
}

/// A run below the slab: on the slab level and below its floor (a run under an upper floor is in
/// the ceiling of the floor below, not in the ground).
pub(crate) fn below_slab(sp: &BuildingSpec, r: &MepRun) -> bool {
    let Some(slab) = slab_level(sp) else { return false };
    let lowest = r.bottoms_ft.iter().copied().filter(|b| b.is_finite()).fold(f64::INFINITY, f64::min);
    let here = sp.level(&r.level).map_or(f64::NAN, |l| l.elevation_ft);
    let base = sp.level(&slab).map_or(0.0, |l| l.elevation_ft);
    lowest.is_finite() && lowest < -1e-6 && (r.level.eq_ignore_ascii_case(&slab) || (here.is_finite() && here + lowest < base - 1e-6))
}

/// Its system is underground work's (`E-UG`, `P-UG-…`).
pub(crate) fn ug_system(name: &str) -> bool {
    name.eq_ignore_ascii_case(E_UG) || name.to_ascii_uppercase().starts_with(&format!("{P_UG}-")) || name.eq_ignore_ascii_case(P_UG)
}

/// A run a feeder's route made (its source names the feeder, `feeder 7 SB1→P2`), above or
/// below the slab.
pub(crate) fn is_feeder_run(r: &MepRun) -> bool {
    r.system == super::gearbuild::FEEDER_SYSTEM || (r.system == E_UG && r.source_sheet.starts_with("feeder "))
}

/// The slab's outline `[x0, y0, x1, y1]` (its exterior walls' extent), else the level's region.
pub(crate) fn slab_box(sp: &BuildingSpec, level: &str) -> Option<[f64; 4]> {
    let walls: Vec<&[f64; 4]> = sp
        .shells
        .iter()
        .find(|(l, _)| l.eq_ignore_ascii_case(level))
        .map(|(_, b)| b.iter().filter(|b| b.iter().all(|v| v.is_finite())).collect())
        .unwrap_or_default();
    if !walls.is_empty() {
        return Some(walls.iter().fold([f64::INFINITY, f64::INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY], |a, b| {
            [a[0].min(b[0]).min(b[2]), a[1].min(b[1]).min(b[3]), a[2].max(b[0]).max(b[2]), a[3].max(b[1]).max(b[3])]
        }));
    }
    sp.level(level)
        .and_then(|l| l.region)
        .filter(|r| r.iter().all(|v| v.is_finite()))
        .map(|r| [r[0].min(r[2]), r[1].min(r[3]), r[0].max(r[2]), r[1].max(r[3])])
}

/// The point on the slab's edge nearest `p`, and the way out (a unit vector).
pub(crate) fn nearest_edge(b: [f64; 4], p: [f64; 2]) -> ([f64; 2], [f64; 2]) {
    let px = p[0].clamp(b[0], b[2]);
    let py = p[1].clamp(b[1], b[3]);
    let sides = [
        ((px - b[0]).abs(), [b[0], py], [-1.0, 0.0]),
        ((b[2] - px).abs(), [b[2], py], [1.0, 0.0]),
        ((py - b[1]).abs(), [px, b[1]], [0.0, -1.0]),
        ((b[3] - py).abs(), [px, b[3]], [0.0, 1.0]),
    ];
    sides.into_iter().min_by(|a, c| a.0.total_cmp(&c.0)).map_or((p, [0.0, -1.0]), |(_, e, n)| (e, n))
}

fn centre(e: &MepEquipment) -> [f64; 2] {
    [(e.min[0] + e.max[0]) / 2.0, (e.min[1] + e.max[1]) / 2.0]
}

/// A room of the slab level: its name (capitals) and outline (plan, sheet units).
#[derive(Clone, Debug)]
pub(crate) struct Room {
    pub level: String,
    pub name: String,
    pub outline: Vec<[f64; 2]>,
}

fn room_centre(r: &Room) -> Option<[f64; 2]> {
    let n = r.outline.len();
    (n >= 3).then(|| {
        let (sx, sy) = r.outline.iter().fold((0.0, 0.0), |a, p| (a.0 + p[0], a.1 + p[1]));
        [sx / n as f64, sy / n as f64]
    })
}

/// Whether a name has `word` as a whole word.
fn has_word(name: &str, word: &str) -> bool {
    !word.is_empty() && name.split(|c: char| !c.is_ascii_alphanumeric()).any(|w| w.eq_ignore_ascii_case(word))
}

/// The service gear on the slab level: a switchboard or switchgear, else the gear feeders leave
/// and none arrives at.
pub(crate) fn service_gear<'a>(sp: &'a BuildingSpec, slab: &str) -> Option<&'a MepEquipment> {
    let on = |e: &&MepEquipment| e.level.eq_ignore_ascii_case(slab);
    sp.mep
        .equipment
        .iter()
        .filter(on)
        .find(|e| {
            let k = e.kind.to_ascii_lowercase();
            k.contains("switchboard") || k.contains("switchgear") || k.contains("service")
        })
        .or_else(|| {
            sp.mep.equipment.iter().filter(on).find(|e| {
                sp.mep.feeders.iter().any(|f| f.from.eq_ignore_ascii_case(&e.tag))
                    && !sp.mep.feeders.iter().any(|f| f.to.eq_ignore_ascii_case(&e.tag))
            })
        })
}

/// A buried conduit run (`E-UG`): `pts` at `depth_ft` below the floor to its top.
fn ug_run(level: &str, src: &str, pts: Vec<[f64; 2]>, size: f64, bottom: f64, why: &str, size_from: &str) -> MepRun {
    let mut r = super::printcond::run(E_UG, level, src, pts, size, bottom);
    r.height_source = why.to_string();
    r.size_source = size_from.to_string();
    r
}

/// A stub-up: from `bottom` below the floor up to `top`.
fn ug_riser(level: &str, src: &str, at: [f64; 2], size: f64, bottom: f64, top: f64, why: &str, size_from: &str) -> MepRun {
    let mut r = ug_run(level, src, vec![at], size, bottom, why, size_from);
    r.top_ft = Some(top);
    r
}

/// How a called run's height source starts (`called: E-101 note (service_primary)`): the runs
/// [`place`] makes, so a rerun replaces them.
pub(crate) const CALLED_PREFIX: &str = "called: ";
/// How a proposal's runs' source starts (`ug feeder 7 SB1→P2`).
pub(crate) const PROPOSAL_PREFIX: &str = "ug ";

/// Runs this module made (called, proposed, stack bases): a rerun of the job replaces them.
pub(crate) fn made_here(r: &MepRun) -> bool {
    r.height_source.starts_with(CALLED_PREFIX)
        || r.source_sheet.starts_with(PROPOSAL_PREFIX)
        || r.system.to_ascii_uppercase().starts_with(&format!("{P_UG}-"))
}

/// What [`place`] did with one callout.
#[derive(Clone, Debug)]
pub(crate) struct Fate {
    pub callout: usize,
    /// `placed`, `placed_elsewhere` (an earlier step built it: the keynotes' floor box) or
    /// `not_placed`.
    pub status: &'static str,
    pub runs: usize,
    pub how: String,
}

/// The called electrical work CADaddy can locate: each a solid `E-UG` run (and its stub-ups), its
/// source the callout. `already` says whether an earlier step built a callout's work (the
/// keynotes' floor-box conduit).
pub(crate) fn place(sp: &BuildingSpec, rooms: &[Room], called: &[Callout], already: &dyn Fn(&Callout) -> bool) -> (Vec<MepRun>, Vec<Fate>) {
    let r = rules();
    let u = upf(sp);
    let mut runs = Vec::new();
    let mut fates = Vec::new();
    let Some(slab) = slab_level(sp) else { return (runs, fates) };
    let slab_b = slab_box(sp, &slab);
    let gear = service_gear(sp, &slab);
    let stub = r.site.stub_ft.max(1.0) * u;
    let site_point = |p: [f64; 2]| {
        slab_b.map(|b| {
            let (e, n) = nearest_edge(b, p);
            (e, [e[0] + n[0] * stub, e[1] + n[1] * stub])
        })
    };
    for (i, c) in called.iter().enumerate() {
        let fate = |status, n, how: String| Fate { callout: i, status, runs: n, how };
        if c.trade != "E" {
            // A sump's discharge routed under the slab to the outside: the sump where the panel
            // schedule's circuit for it says (an elevator's: in its hoistway).
            let up = c.text.to_ascii_uppercase();
            if c.what == "storm" && up.contains("SUMP") && c.from != "legend" {
                let pr = &r.proposal;
                let pit = sp
                    .mep
                    .panel_schedules
                    .iter()
                    .flat_map(|p| &p.circuits)
                    .map(|ci| ci.description.to_ascii_uppercase())
                    .find(|d| d.contains("SUMP") && has_any(d, &pr.hoistway_words))
                    .and_then(|_| {
                        sp.circulation
                            .iter()
                            .find(|x| x.level.eq_ignore_ascii_case(&slab) && matches!(x.kind, cadaddy_walk3d::CirculationKind::Hoistway))
                            .map(|x| x.at)
                    });
                match (pit.filter(|p| p.iter().all(|v| v.is_finite())), slab_b) {
                    (Some(at), Some(b)) => {
                        let (edge, n) = nearest_edge(b, at);
                        let out = [edge[0] + n[0] * u, edge[1] + n[1] * u];
                        let mut pts = vec![at, [edge[0], at[1]], edge, out];
                        if (edge[0] - at[0]).abs() < 1e-6 || (edge[1] - at[1]).abs() > (edge[0] - at[0]).abs() {
                            pts = vec![at, [at[0], edge[1]], edge, out];
                        }
                        pts.dedup_by(|a, b| (a[0] - b[0]).abs() < 1e-6 && (a[1] - b[1]).abs() < 1e-6);
                        let size = c.size_in.unwrap_or(r.depths.sump_discharge_in);
                        let size_from = if c.size_in.is_some() { "note".to_string() } else { format!("rule: {}", r.depths.sump_source) };
                        let why = format!("{CALLED_PREFIX}{} ({})", c.cite(), c.what);
                        let sys = format!("{P_UG}-ST");
                        let mut run = super::printcond::run(&sys, &slab, &c.sheet, pts, size, -r.depths.drain_invert_ft);
                        run.height_source = why.clone();
                        run.size_source = size_from.clone();
                        let mut day = super::printcond::run(&sys, &slab, &c.sheet, vec![out], size, -r.depths.drain_invert_ft);
                        day.top_ft = Some(0.5);
                        day.height_source = why;
                        day.size_source = size_from;
                        runs.push(run);
                        runs.push(day);
                        fates.push(fate("placed", 2, "from the sump in the elevator's hoistway (its circuit on the panel schedule) under the slab to the nearest exterior wall, up to daylight".into()));
                    }
                    _ => fates.push(fate(
                        "not_placed",
                        0,
                        "no sump the panel schedules place (an elevator's, in its hoistway) on the slab level".into(),
                    )),
                }
                continue;
            }
            // Plumbing below the slab is what the plans draw: the pipes read below it and the
            // stacks' bases. A legend line type or a note alone locates nothing.
            let how = match (c.from, c.what.as_str()) {
                ("legend", _) => {
                    "a line type: placed where the plans' pipes are read below the slab (the PDF import keeps no line pattern, so lines drawn only by it aren't told apart)"
                }
                (_, "other") => "a requirement, not work to place (kept as its note)",
                _ => "no rule places it from a note alone (its tag on the plan isn't read)",
            };
            fates.push(fate("not_placed", 0, how.into()));
            continue;
        }
        if already(c) {
            fates.push(fate("placed_elsewhere", 0, "built by the keynotes step from its tag".into()));
            continue;
        }
        let why = format!("{CALLED_PREFIX}{} ({}{})", c.cite(), c.what, if c.steel_ells { ", steel elbows and risers" } else { "" });
        let (size, size_from) = match c.size_in {
            Some(v) => (v, "note".to_string()),
            None => (super::keynotes::BURIED_MIN_IN, "rule: smallest trade size (the note gives none)".to_string()),
        };
        let cover_ft = c.cover_in.unwrap_or(r.depths.service_cover_in) / 12.0;
        let n_before = runs.len();
        let conduits = c.count.clamp(1, 20);
        let mut add = |pts: Vec<[f64; 2]>, bottom: f64, risers: &[([f64; 2], f64)]| {
            for _ in 0..conduits {
                if pts.len() >= 2 {
                    runs.push(ug_run(&slab, &c.sheet, pts.clone(), size, bottom, &why, &size_from));
                }
                for (at, top) in risers {
                    runs.push(ug_riser(&slab, &c.sheet, *at, size, bottom, *top, &why, &size_from));
                }
            }
        };
        match c.what.as_str() {
            "service_primary" | "service_secondary" => {
                let Some(g) = gear else {
                    fates.push(fate("not_placed", 0, "no service gear on the slab level".into()));
                    continue;
                };
                let Some((edge, pad)) = site_point(centre(g)) else {
                    fates.push(fate("not_placed", 0, "the slab level has no outline".into()));
                    continue;
                };
                let bottom = -(cover_ft + size / 12.0);
                if c.what == "service_primary" {
                    // From the utility's side out past the pad to the pad, coming up in it.
                    let out = [pad[0] + (pad[0] - edge[0]), pad[1] + (pad[1] - edge[1])];
                    add(vec![out, pad], bottom, &[(pad, r.stubup.conduit_top_ft)]);
                    fates.push(fate(
                        "placed",
                        runs.len() - n_before,
                        format!(
                            "utility to the pad transformer; pad by rule {} ft outside the slab edge nearest {} (locate from the site plan)",
                            r.site.stub_ft, g.tag
                        ),
                    ));
                } else {
                    let gc = centre(g);
                    let pts = vec![pad, edge, [gc[0], edge[1]], gc];
                    let mut pts = pts;
                    pts.dedup_by(|a, b| (a[0] - b[0]).abs() < 1e-6 && (a[1] - b[1]).abs() < 1e-6);
                    add(pts, bottom, &[(pad, r.stubup.conduit_top_ft), (gc, r.stubup.conduit_top_ft.max(g.bottom_ft))]);
                    fates.push(fate(
                        "placed",
                        runs.len() - n_before,
                        format!("pad to {} (bottom entry), pad by rule outside the nearest slab edge", g.tag),
                    ));
                }
            }
            "telecom" => {
                let room_word = c.room.clone().unwrap_or_default();
                // The slab level's rooms first, then the levels up (a room upstairs: its riser).
                let elevation = |l: &str| sp.level(l).map_or(f64::INFINITY, |v| v.elevation_ft);
                let mut ordered: Vec<&Room> = rooms.iter().collect();
                ordered.sort_by(|a, b| elevation(&a.level).total_cmp(&elevation(&b.level)));
                let named = |w: &str| ordered.iter().copied().find(|rm| has_word(&rm.name, w));
                // The room the note names, else a telecom room by the rules' names.
                let target = ordered
                    .iter()
                    .copied()
                    .find(|rm| !room_word.is_empty() && room_word.split_whitespace().all(|w| has_word(&rm.name, w)))
                    .or_else(|| if room_word.is_empty() { r.telecom_rooms.iter().find_map(|w| named(w)) } else { None })
                    .and_then(|rm| room_centre(rm).map(|p| (rm.name.clone(), rm.level.clone(), p)));
                let room_word = if room_word.is_empty() { r.telecom_rooms.join("/") } else { room_word };
                let Some((name, room_level, at)) = target else {
                    fates.push(fate("not_placed", 0, format!("no room named {room_word} in the model")));
                    continue;
                };
                // Onto the slab level's plan (each level's plan has its own place in the drawing).
                let corner = |l: &str| sp.level(l).and_then(|v| v.region).map_or([0.0, 0.0], |g| [g[0], g[1]]);
                let (h, m) = (corner(&room_level), corner(&slab));
                let at = [at[0] - h[0] + m[0], at[1] - h[1] + m[1]];
                let rise = elevation(&room_level) - elevation(&slab);
                let rise = if rise.is_finite() { rise.max(0.0) } else { 0.0 };
                let Some((edge, out)) = site_point(at) else {
                    fates.push(fate("not_placed", 0, "the slab level has no outline".into()));
                    continue;
                };
                let bottom = -(cover_ft + size / 12.0);
                let mut pts = vec![out, edge, [at[0], edge[1]], at];
                pts.dedup_by(|a, b| (a[0] - b[0]).abs() < 1e-6 && (a[1] - b[1]).abs() < 1e-6);
                add(pts, bottom, &[(at, rise + r.stubup.telecom_top_ft)]);
                let up = if rise > 0.0 { format!(", rising to it on {room_level}") } else { String::new() };
                fates.push(fate(
                    "placed",
                    runs.len() - n_before,
                    format!("from the street side under the slab to room {name}{up}; site end by rule outside the nearest slab edge"),
                ));
            }
            "ev_charger" | "site_power" => {
                // The panel whose schedule feeds it (a circuit described as it is).
                let words: Vec<&String> = r.kinds.iter().filter(|k| k.what == c.what).flat_map(|k| &k.words).collect();
                let panel = sp
                    .mep
                    .panel_schedules
                    .iter()
                    .find(|p| p.circuits.iter().any(|ci| words.iter().any(|w| ci.description.to_ascii_uppercase().contains(w.as_str()))))
                    .and_then(|p| sp.mep.equipment.iter().find(|e| e.tag.eq_ignore_ascii_case(&p.panel) && e.level.eq_ignore_ascii_case(&slab)));
                let Some(pn) = panel else {
                    fates.push(fate("not_placed", 0, format!("no panel on {slab} has a circuit for it")));
                    continue;
                };
                let pc = centre(pn);
                let Some((edge, out)) = site_point(pc) else {
                    fates.push(fate("not_placed", 0, "the slab level has no outline".into()));
                    continue;
                };
                let bottom = -(r.depths.conduit_under_slab_ft + size / 12.0);
                let mut pts = vec![pc, [pc[0], edge[1]], edge, out];
                if (edge[1] - pc[1]).abs() < 1e-6 || (edge[0] - pc[0]).abs() > (edge[1] - pc[1]).abs() {
                    pts = vec![pc, [edge[0], pc[1]], edge, out];
                }
                pts.dedup_by(|a, b| (a[0] - b[0]).abs() < 1e-6 && (a[1] - b[1]).abs() < 1e-6);
                add(pts, bottom, &[(pc, r.stubup.conduit_top_ft.max(pn.bottom_ft))]);
                fates.push(fate(
                    "placed",
                    runs.len() - n_before,
                    format!("out of panel {} to the site; the site end by rule outside the nearest slab edge (locate from the site plan)", pn.tag),
                ));
            }
            "floor_box" => fates.push(fate("not_placed", 0, "no keynote tag locates the floor box".into())),
            _ => fates.push(fate("not_placed", 0, "no rule places this kind of work".into())),
        }
    }
    (runs, fates)
}

/// The drainage services (sanitary, waste, storm) a stack base or a slab drain belongs to.
const DRAINS: [&str; 6] = ["SAN", "W", "ST", "SS", "SD", "RD"];

/// How a slab drain's height source starts: a run [`slab_drains`] put under the floor.
pub(crate) const SLAB_DRAIN_PREFIX: &str = "below the slab: ";

/// The slab level's horizontal drainage (sanitary, waste, storm) that its plans give no height:
/// on the slab level it runs under the floor to the building drain, not in the ceiling (where a
/// default height or the plenum seating put it). Each such run goes to the drain's invert
/// (`depths.drain_invert_ft`), its height source saying so; a run with a height of its own (a
/// tag, a callout) and the risers keep theirs, and upper levels' drains stay in the ceiling below
/// them. Returns the runs moved and their length (drawing units). A rerun finds them moved.
pub(crate) fn slab_drains(sp: &mut BuildingSpec) -> (usize, f64) {
    let r = rules();
    let Some(slab) = slab_level(sp) else { return (0, 0.0) };
    let invert = -r.depths.drain_invert_ft.abs();
    if !invert.is_finite() {
        return (0, 0.0);
    }
    let mut moved = 0usize;
    let mut length = 0.0;
    for run in sp.mep.runs.iter_mut().take(MAX_SCAN) {
        if moved >= MAX_ITEMS {
            break;
        }
        let sys = run.system.to_ascii_uppercase();
        let untagged = matches!(run.height_source.trim().replace(' ', "-").as_str(), "" | "default" | "estimated-route");
        if !DRAINS.contains(&sys.as_str())
            || !run.level.eq_ignore_ascii_case(&slab)
            || !untagged
            || run.top_ft.is_some()
            || run.points.len() < 2
            || ug_system(&run.system)
        {
            continue;
        }
        run.bottoms_ft.fill(invert);
        if run.bottoms_ft.is_empty() {
            run.bottoms_ft.push(invert);
        }
        run.height_source = format!("{SLAB_DRAIN_PREFIX}{} ({} ft)", r.depths.slab_drain_source, r.depths.drain_invert_ft);
        run.reviewed = false;
        length += run.points.windows(2).map(|w| (w[1][0] - w[0][0]).hypot(w[1][1] - w[0][1])).filter(|v| v.is_finite()).sum::<f64>();
        moved += 1;
    }
    (moved, length)
}

/// The drainage stacks of the slab level carried below its floor into the building drain, as
/// `P-UG-<service>` risers from the drain's invert up to the floor; vents only when a legend
/// says vent piping runs below the slab.
pub(crate) fn stack_bases(sp: &BuildingSpec, vent_below: bool) -> Vec<MepRun> {
    let r = rules();
    let Some(slab) = slab_level(sp) else { return Vec::new() };
    let mut out: Vec<MepRun> = Vec::new();
    for run in sp.mep.runs.iter().take(MAX_SCAN) {
        if out.len() >= MAX_ITEMS {
            break;
        }
        let sys = run.system.to_ascii_uppercase();
        let drainage = DRAINS.contains(&sys.as_str()) || (vent_below && sys == "V");
        if !drainage || !run.level.eq_ignore_ascii_case(&slab) || ug_system(&run.system) {
            continue;
        }
        let (Some(at), Some(_)) = (run.points.first(), run.top_ft) else { continue };
        let bottom = run.bottoms_ft.first().copied().unwrap_or(0.0);
        if run.points.len() != 1 || !bottom.is_finite() || bottom.abs() > 0.25 {
            continue;
        }
        // One base per spot.
        if out.iter().any(|o| o.points.first().is_some_and(|p| (p[0] - at[0]).hypot(p[1] - at[1]) < 0.5 * upf(sp))) {
            continue;
        }
        let mut b = super::printcond::run(&format!("{P_UG}-{sys}"), &slab, &run.source_sheet, vec![*at], run.size_in[0], -r.depths.drain_invert_ft);
        b.top_ft = Some(0.0);
        b.size_source = if run.size_source.is_empty() { "the stack's".into() } else { format!("the stack's ({})", run.size_source) };
        b.height_source = format!("rule: stack base to the building drain ({}); the plans give no invert", r.depths.drain_invert_ft);
        out.push(b);
    }
    out
}

// ---------------- weigh ----------------

/// What the framing above the slab level is (from the joist and truss documents).
#[derive(Clone, Debug, Default)]
pub(crate) struct FramingAbove {
    /// `joist` or `truss`.
    pub kind: String,
    pub item: String,
    pub members: usize,
    pub source: String,
}

/// One feed weighed under the slab against overhead.
#[derive(Clone, Debug)]
pub(crate) struct Proposal {
    /// `feeder 7 SB1→P2`, `feed 84 SB1→LIFT (circuit 4)`.
    pub subject: String,
    pub from: String,
    pub to: String,
    /// Feeders of a typical panel weighed together.
    pub copies: usize,
    pub conduits: u32,
    pub size_in: f64,
    pub under_ft: f64,
    pub over_ft: f64,
    /// `under_slab` or `overhead`.
    pub decision: &'static str,
    pub reasons: Vec<String>,
    pub alternative: String,
    /// Its route in the model already runs under the slab (the gear's entries took it there).
    pub routed_under: bool,
    /// Its route in the model already runs overhead.
    pub routed_over: bool,
    /// Points of a route this proposal adds (none routed it yet): from, to (plan).
    pub ends: Option<([f64; 2], [f64; 2])>,
    pub stub_tops: (f64, f64),
    /// Its route under the slab (plan): from its gear to its (first) load in two legs.
    pub path: Vec<[f64; 2]>,
}

impl Proposal {
    pub(crate) fn json(&self) -> Value {
        json!({
            "subject": self.subject, "from": self.from, "to": self.to, "copies": self.copies, "conduits": self.conduits, "size_in": self.size_in,
            "under_slab_ft": (self.under_ft * 10.0).round() / 10.0, "overhead_ft": (self.over_ft * 10.0).round() / 10.0,
            "decision": self.decision, "reasons": self.reasons, "alternative": self.alternative,
            "routed_under": self.routed_under, "routed_over": self.routed_over, "adds_route": self.ends.is_some(),
        })
    }
}

/// A feed read from a panel schedule's row (`feeder 74: 1 set(s) of 4 4 cu, ground 8, in 1.25"
/// conduit`): sets, conduit size.
fn row_feed(wire: &str) -> Option<(u32, f64, String)> {
    let low = wire.to_ascii_lowercase();
    let rest = low.strip_prefix("feeder ")?;
    let tag = rest.split(':').next().unwrap_or_default().trim().to_string();
    let sets =
        low.split_whitespace().find_map(|w| w.strip_suffix("set(s)").or(Some(w)).and_then(|v| v.parse::<u32>().ok())).unwrap_or(1).clamp(1, 20);
    let size = low
        .split(" in ")
        .nth(1)
        .and_then(|v| v.split_whitespace().next())
        .and_then(|v| v.trim_end_matches('"').parse::<f64>().ok())
        .filter(|v| v.is_finite() && *v > 0.0 && *v <= 12.0)?;
    Some((sets, size, tag))
}

/// Weigh each feed whose two ends are on the slab level: the feeders (a typical panel's copies
/// on the slab together), and the rows of slab-level gear feeding a load that isn't gear (an
/// elevator: its hoistway on the slab).
pub(crate) fn weigh(sp: &BuildingSpec, framing: Option<&FramingAbove>, below_method: Option<&str>) -> Vec<Proposal> {
    let r = rules();
    let pr = &r.proposal;
    let u = upf(sp);
    let mut out = Vec::new();
    let Some(slab) = slab_level(sp) else { return out };
    let ceiling = sp.level(&slab).map(|l| l.ceiling_ft).filter(|c| c.is_finite() && *c > 0.0).unwrap_or(9.0);
    let under_depth = r.depths.conduit_under_slab_ft;
    let on_slab = |e: &&MepEquipment| e.level.eq_ignore_ascii_case(&slab);
    let gear = |tag: &str| sp.mep.equipment.iter().filter(on_slab).find(|e| e.tag.eq_ignore_ascii_case(tag));
    let family = |tag: &str| -> Vec<&MepEquipment> {
        let fam = format!("{}-", tag.to_ascii_uppercase());
        sp.mep.equipment.iter().filter(on_slab).filter(|e| e.tag.eq_ignore_ascii_case(tag) || e.tag.to_ascii_uppercase().starts_with(&fam)).collect()
    };
    let routes = |head: &str| -> (bool, bool) {
        let mine: Vec<&MepRun> = sp.mep.runs.iter().take(200_000).filter(|x| is_feeder_run(x) && x.source_sheet.starts_with(head)).collect();
        let under = mine.iter().any(|x| x.level.eq_ignore_ascii_case(&slab) && x.points.len() >= 2 && x.bottoms_ft.iter().any(|b| *b < 0.0));
        let over = mine.iter().any(|x| x.level.eq_ignore_ascii_case(&slab) && x.points.len() >= 2 && x.bottoms_ft.iter().all(|b| *b >= 0.0));
        (under, over)
    };
    let manhattan = |a: [f64; 2], b: [f64; 2]| ((a[0] - b[0]).abs() + (a[1] - b[1]).abs()) / u;
    // Each level's plan has its own place in the drawing: a point of another level's onto the
    // slab level's plan (the riser under it).
    let corner = |level: &str| sp.level(level).and_then(|l| l.region).map_or([0.0, 0.0], |r| [r[0], r[1]]);
    let onto_slab = |e: &MepEquipment| {
        let (c, h, m) = (centre(e), corner(&e.level), corner(&slab));
        [c[0] - h[0] + m[0], c[1] - h[1] + m[1]]
    };
    // Gear whose feeders leave through its bottom (their entries, from its submittal).
    let bottom_fed = |tag: &str| {
        sp.mep.feeders.iter().take(MAX_ITEMS).any(|f| {
            let side = if f.from_entry.trim().is_empty() { &f.entry } else { &f.from_entry };
            f.from.eq_ignore_ascii_case(tag) && side.trim().eq_ignore_ascii_case("bottom")
        }) || sp.mep.runs.iter().take(200_000).any(|x| {
            x.system == E_UG
                && x.source_sheet.starts_with("feeder ")
                && x.source_sheet.split_whitespace().nth(2).is_some_and(|ends| ends.split('→').next().is_some_and(|a| a.eq_ignore_ascii_case(tag)))
        })
    };
    let mut weigh_one = |subject: String,
                         from: &MepEquipment,
                         to_tag: String,
                         targets: Vec<[f64; 2]>,
                         conduits: u32,
                         size: f64,
                         head: &str,
                         stub_to: f64| {
        if targets.is_empty() {
            return;
        }
        let a = centre(from);
        let flat: f64 = targets.iter().map(|t| manhattan(a, *t)).filter(|v| v.is_finite()).sum();
        let n = targets.len() as f64;
        let under_ft = flat + n * 2.0 * (under_depth + 1.0);
        let over_ft = flat + n * 2.0 * ceiling;
        let (routed_under, routed_over) = routes(head);
        let mut reasons: Vec<String> = Vec::new();
        let mut score = 0i32;
        if routed_under {
            score += 2;
            reasons.push(format!(
                "{} leaves {} through its bottom (the gear's entries, its submittal): its route already runs under the slab",
                subject, from.tag
            ));
        } else if bottom_fed(&from.tag) {
            score += 2;
            reasons.push(format!(
                "{} is bottom-fed: its feeders leave through its bottom (their entries, its submittal), so a feed out of it leaves the same way",
                from.tag
            ));
        }
        if conduits >= pr.parallel_min.max(2) {
            score += 1;
            reasons.push(format!(
                "{conduits} parallel raceways of {}\": a bank overhead needs a rack and crosses every joist bay (rule: {} or more)",
                super::ugsheets::trade_size(size),
                pr.parallel_min
            ));
        }
        if size >= pr.big_conduit_in {
            if let Some(f) = framing {
                score += 1;
                reasons.push(format!(
                    "the floor above the slab level is framed with {} ({} members, {}): a {}\" raceway bores a hole of {}\" or passes truss webs at every bay",
                    if f.item.is_empty() { f.kind.clone() } else { f.item.clone() },
                    f.members,
                    f.source,
                    super::ugsheets::trade_size(size),
                    super::ugsheets::trade_size(size + 0.375)
                ));
            } else {
                reasons.push(format!(
                    "{}\" raceway (rule: {}\" and up weigh the framing above; the joist and truss documents give none here)",
                    super::ugsheets::trade_size(size),
                    pr.big_conduit_in
                ));
            }
        }
        if under_ft < over_ft * pr.under_cheaper {
            score += 1;
            reasons
                .push(format!("{under_ft:.0} ft under the slab against {over_ft:.0} ft overhead (down from and back up to the ceiling at each end)"));
        }
        match below_method {
            Some(m) => reasons.push(format!("the specification allows it below the slab: {m}")),
            None => {
                score -= 1;
                reasons.push("the specification's wiring-methods table lists no method below the slab: the engineer confirms PVC there".into());
            }
        }
        reasons.extend(pr.code.iter().cloned());
        let decision = if score >= 2 { "under_slab" } else { "overhead" };
        let alternative = if decision == "under_slab" {
            format!("overhead: {over_ft:.0} ft of EMT in the ceiling space of {slab}, through the framing")
        } else {
            format!("under the slab: {under_ft:.0} ft of Sch 40 PVC, stubbed up at each end")
        };
        let adds = !routed_under && !routed_over && decision == "under_slab";
        let path = targets.first().map_or_else(Vec::new, |t| {
            let mut v = vec![a, [t[0], a[1]], *t];
            v.dedup_by(|x, y| (x[0] - y[0]).abs() < 1e-6 && (x[1] - y[1]).abs() < 1e-6);
            v
        });
        out.push(Proposal {
            subject,
            from: from.tag.clone(),
            to: to_tag,
            copies: targets.len(),
            conduits,
            size_in: size,
            under_ft,
            over_ft,
            decision,
            reasons,
            alternative,
            routed_under,
            routed_over,
            ends: if adds { targets.first().map(|t| (a, *t)) } else { None },
            stub_tops: (r.stubup.conduit_top_ft.max(from.bottom_ft), stub_to),
            path,
        });
    };
    // The feeders, a typical panel's copies on the slab weighed together.
    let mut seen: Vec<(String, String, String)> = Vec::new();
    for f in sp.mep.feeders.iter().take(MAX_ITEMS) {
        let key = (f.tag.to_ascii_uppercase(), f.from.to_ascii_uppercase(), f.to.to_ascii_uppercase());
        if seen.contains(&key) {
            continue;
        }
        seen.push(key);
        let Some(a) = gear(&f.from) else { continue };
        let copies =
            sp.mep.feeders.iter().filter(|g| g.tag == f.tag && g.from.eq_ignore_ascii_case(&f.from) && g.to.eq_ignore_ascii_case(&f.to)).count();
        let fam = family(&f.to);
        // Gear on another level: the slab leg to the riser under it.
        let up_there: Vec<&MepEquipment> =
            if fam.is_empty() { sp.mep.equipment.iter().filter(|e| e.tag.eq_ignore_ascii_case(&f.to)).take(1).collect() } else { Vec::new() };
        let targets: Vec<[f64; 2]> = if fam.is_empty() {
            up_there.iter().map(|e| onto_slab(e)).collect()
        } else {
            fam.iter().take(copies.max(1)).map(|e| centre(e)).collect()
        };
        let to_bottom = fam.first().map_or(0.5, |e| e.bottom_ft.max(0.5));
        let conduits = f.conduits.max(f.sets).max(1);
        let head = format!("feeder {} {}→", f.tag, f.from);
        let subject = match up_there.first() {
            Some(e) => format!("feeder {} {}→{} (to its riser; {} is on {})", f.tag, f.from, f.to, e.tag, e.level),
            None if targets.len() > 1 => format!("feeders {} {}→{} ({} on {slab})", f.tag, f.from, f.to, targets.len()),
            None => format!("feeder {} {}→{}", f.tag, f.from, f.to),
        };
        weigh_one(subject, a, f.to.clone(), targets, conduits, f.conduit_in, &head, to_bottom);
    }
    // The rows of slab-level gear feeding a load that isn't gear: its place by its name (a
    // hoistway for an elevator, a room so named).
    for p in sp.mep.panel_schedules.iter().take(MAX_ITEMS) {
        let Some(a) = gear(&p.panel) else { continue };
        for c in p.circuits.iter().take(400) {
            let Some((sets, size, tag)) = row_feed(&c.wire) else { continue };
            let amps = c.breaker.trim().trim_end_matches(['A', 'a']).parse::<f64>().unwrap_or(0.0);
            let desc = c.description.to_ascii_uppercase();
            if amps < pr.branch_min_amps || sp.mep.feeders.iter().any(|f| f.tag == tag && f.from.eq_ignore_ascii_case(&p.panel)) {
                continue;
            }
            // Gear the row names is a feeder's (weighed above, or not on the slab).
            if sp.mep.equipment.iter().any(|e| has_word(&desc, &e.tag)) {
                continue;
            }
            let hoist = sp
                .circulation
                .iter()
                .filter(|x| x.level.eq_ignore_ascii_case(&slab) && matches!(x.kind, cadaddy_walk3d::CirculationKind::Hoistway))
                .find(|_| has_any(&desc, &pr.hoistway_words))
                .map(|x| x.at);
            let Some(at) = hoist.filter(|p| p.iter().all(|v| v.is_finite())) else { continue };
            let subject = format!("feed {tag} {}→{} (circuit {})", p.panel, c.description.trim(), c.number);
            weigh_one(
                subject,
                a,
                c.description.trim().to_string(),
                vec![at],
                sets,
                size,
                &format!("feeder {tag} {}→", p.panel),
                r.stubup.conduit_top_ft,
            );
        }
    }
    out
}

/// The runs a proposal adds where nothing routed it yet: under the slab from its gear to its
/// load in two legs, stubbed up at both ends, dashed on the sheet (`proposed`).
pub(crate) fn proposal_runs(sp: &BuildingSpec, p: &Proposal) -> Vec<MepRun> {
    let r = rules();
    let Some(slab) = slab_level(sp) else { return Vec::new() };
    let Some((a, b)) = p.ends else { return Vec::new() };
    let src = format!("{PROPOSAL_PREFIX}{}", p.subject);
    let bottom = -(r.depths.conduit_under_slab_ft + p.size_in / 12.0);
    let mut out = Vec::new();
    for _ in 0..p.conduits.clamp(1, 20) {
        let mut pts = vec![a, [b[0], a[1]], b];
        pts.dedup_by(|x, y| (x[0] - y[0]).abs() < 1e-6 && (x[1] - y[1]).abs() < 1e-6);
        let mut run = ug_run(&slab, &src, pts, p.size_in, bottom, "proposed", "the schedule's row");
        run.height_source = "proposed".into();
        out.push(run);
        for (at, top) in [(a, p.stub_tops.0), (b, p.stub_tops.1)] {
            let mut up = ug_riser(&slab, &src, at, p.size_in, bottom, top, "proposed", "the schedule's row");
            up.height_source = "proposed".into();
            out.push(up);
        }
    }
    out
}

/// The spec's below-slab wiring method, as the job read its wiring-methods table: the first
/// application naming work below the slab or in the ground, with its methods and section.
pub(crate) fn below_method(d: &cadaddy_doc::Drawing) -> Option<String> {
    let m = super::conduitroute::Methods::stored(d);
    let words = ["UNDERGROUND", "BELOW SLAB", "UNDER SLAB", "BELOW GRADE", "IN SLAB", "BELOW GROUND"];
    m.applications.iter().find(|a| words.iter().any(|w| a.application.to_ascii_uppercase().contains(w))).map(|a| {
        format!(
            "{} ({}{}): {}",
            a.application.trim(),
            a.section.trim(),
            if a.item.trim().is_empty() { String::new() } else { format!(" {}", a.item.trim()) },
            a.methods.join(", ")
        )
    })
}

/// The framing over the slab level (the joists or trusses of the floor above it), when the
/// trades' layout has made it.
pub(crate) fn framing_above(d: &cadaddy_doc::Drawing, sp: &BuildingSpec) -> Option<FramingAbove> {
    let slab = slab_level(sp)?;
    let lv = sp.level(&slab)?;
    let floor = lv.elevation_ft * upf(sp);
    let members = super::tradeatoms::framing_members(d, sp);
    let over: Vec<_> = members.iter().filter(|m| m.z0 > floor && matches!(m.kind, "joist" | "truss")).collect();
    let low = over.iter().map(|m| m.z0).fold(f64::INFINITY, f64::min);
    let first: Vec<_> = over.iter().filter(|m| m.z0 <= low + 2.0 * upf(sp)).collect();
    let m = first.first()?;
    Some(FramingAbove { kind: m.kind.to_string(), item: m.item.clone(), members: first.len(), source: m.source.clone() })
}

#[cfg(test)]
mod tests {
    use super::*;
    use cadaddy_geom::Vec2;

    fn t(x: f64, y: f64, v: &str, h: f64) -> TextItem {
        TextItem { value: v.into(), at: Vec2::new(x, y), height: h, rotation: 0.0 }
    }

    #[test]
    fn the_rules_load_with_their_sources() {
        let r = rules();
        assert!(!r.kinds.is_empty() && !r.site_words.is_empty() && !r.legend.below_words.is_empty());
        for s in [
            &r.site_source,
            &r.legend.source,
            &r.depths.conduit_source,
            &r.depths.service_source,
            &r.depths.drain_source,
            &r.depths.slab_drain_source,
            &r.site.source,
            &r.stubup.source,
            &r.proposal.source,
            &r.sleeve.source,
            &r.bearing_plane.source,
        ] {
            assert!(!s.trim().is_empty());
        }
        assert!(r.depths.service_cover_in > 0.0 && r.site.stub_ft > 0.0 && r.bearing_plane.angle_deg > 0.0);
    }

    #[test]
    fn a_callouts_words_give_count_size_material_cover_and_elbows() {
        let p = parse("THREE (3) 4\" SCH. 80 PVC CONDUITS WITH RGS ELBOWS AND RISERS, MIN. 36\" BELOW GRADE TO THE UTILITY'S VAULT.");
        assert_eq!((p.count, p.size_in, p.material.as_str(), p.cover_in, p.steel_ells), (Some(3), Some(4.0), "PVC Sch 80", Some(36.0), true));
        // A count in words alone, the depth worded the other way, another material.
        let p = parse("Provide two 2-1/2\" RGS conduit a minimum of 30\" below finished grade");
        assert_eq!((p.count, p.size_in, p.material.as_str(), p.cover_in), (Some(2), Some(2.5), "RGS", Some(30.0)));
        // A room it goes to; nothing where nothing is said.
        let p = parse("PROVIDE (2) 3\" CONDUITS FROM THE STREET TO THE TELECOM ROOM.");
        assert_eq!((p.count, p.size_in, p.room.as_deref()), (Some(2), Some(3.0), Some("TELECOM")));
        assert_eq!(parse("ROUTE CONDUIT AS REQUIRED"), Parsed::default());
        // Hostile: long and odd text.
        let long = "1\" ".repeat(5000);
        let _ = parse(&long);
        let _ = parse("(99999999999) 0\" -5\" NaN\" BELOW");
    }

    #[test]
    fn callouts_come_from_notes_keynotes_and_legend_line_types_by_trade() {
        let texts = vec![
            // A site plan's callout, line by line (no buried word in its first line).
            t(100.0, 500.0, "FOUR (4) 3\" SCH. 40 PVC CONDUIT", 6.0),
            t(100.0, 492.0, "WITH STEEL ELLS, MIN. 42\" BELOW GRADE", 6.0),
            t(100.0, 484.0, "FOR PRIMARY CONDUCTORS.", 6.0),
            // A keynote list: one buried, one going out to the site, one neither.
            t(600.0, 600.0, "ELECTRICAL KEYNOTES:", 8.0),
            t(600.0, 590.0, "1.", 6.0),
            t(612.0, 590.0, "PROVIDE POWER FOR ELECTRIC VEHICLE CHARGERS IN THE LOT.", 6.0),
            t(600.0, 580.0, "2.", 6.0),
            t(612.0, 580.0, "PROVIDE UNDER SLAB CONDUIT FOR FLOOR BOX.", 6.0),
            t(600.0, 570.0, "3.", 6.0),
            t(612.0, 570.0, "MOUNT AT 48 INCHES.", 6.0),
            // A legend's line type: the abbreviation beside its description.
            t(900.0, 300.0, "SANITARY PIPING (BELOW FLOOR)", 5.0),
            t(880.0, 300.0, "SANBF", 4.0),
            // A legend row that isn't buried, and a long note naming a slab that isn't a line type.
            t(900.0, 280.0, "COLD WATER PIPING", 5.0),
            t(880.0, 280.0, "CW", 4.0),
            // A row whose own abbreviation is farther than another column's.
            t(900.0, 200.0, "COLD WATER PIPING (BELOW SLAB)", 5.0),
            t(885.0, 200.0, "HW", 4.0),
            t(864.0, 198.0, "CWUS", 4.0),
        ];
        let c = callouts("E-101", &texts);
        let prim = c.iter().find(|c| c.what == "service_primary").expect("primary");
        assert_eq!(
            (prim.trade.as_str(), prim.count, prim.size_in, prim.cover_in, prim.steel_ells, prim.from),
            ("E", 4, Some(3.0), Some(42.0), true, "note")
        );
        let ev = c.iter().find(|c| c.what == "ev_charger").expect("ev");
        assert_eq!((ev.from, ev.mark.as_deref(), ev.said_buried), ("keynote", Some("1"), false));
        assert!(c.iter().any(|c| c.what == "floor_box" && c.said_buried));
        assert!(!c.iter().any(|c| c.text.contains("48 INCHES")));
        let lg = c.iter().find(|c| c.from == "legend").expect("legend");
        assert_eq!((lg.trade.as_str(), lg.what.as_str(), lg.mark.as_deref()), ("P", "sanitary", Some("SANBF")));
        assert!(!c.iter().any(|c| c.text == "COLD WATER PIPING"));
        let cw = c.iter().find(|c| c.text.contains("COLD WATER PIPING (BELOW")).expect("cw");
        assert_eq!((cw.mark.as_deref(), cw.what.as_str()), (Some("CWUS"), "water"));
        assert_eq!(lg.cite(), "E-101 legend SANBF");
    }

    fn spec() -> BuildingSpec {
        BuildingSpec::from_json(
            &json!({
                "units_per_foot": 12.0,
                "levels": [{"name": "Level 1", "elevation_ft": 0, "height_ft": 10, "ceiling_ft": 9}, {"name": "Level 2", "elevation_ft": 10, "height_ft": 10}],
                "shells": {"Level 1": [[0, 0, 1200, 0], [1200, 0, 1200, 600], [1200, 600, 0, 600], [0, 600, 0, 0]]},
                "mep": {
                    "equipment": [
                        {"tag": "SB1", "kind": "switchboard", "level": "Level 1", "min": [1100, 300], "max": [1130, 340], "bottom_ft": 0, "height_ft": 7},
                        {"tag": "P2", "kind": "panel", "level": "Level 1", "min": [200, 500], "max": [220, 504], "bottom_ft": 4, "height_ft": 4},
                        {"tag": "Q", "kind": "panel", "level": "Level 1", "min": [100, 100], "max": [120, 104], "bottom_ft": 4, "height_ft": 4},
                        {"tag": "Q-3", "kind": "panel", "level": "Level 2", "min": [100, 100], "max": [120, 104], "bottom_ft": 4, "height_ft": 4}
                    ],
                    "feeders": [
                        {"tag": "F1", "from": "SB1", "to": "P2", "conduit_in": 3, "conduits": 2, "sets": 2, "size": "350", "material": "cu"},
                        {"tag": "F2", "from": "SB1", "to": "Q", "conduit_in": 1, "conduits": 1, "sets": 1, "size": "6", "material": "cu"}
                    ],
                    "panel_schedules": [
                        {"panel": "SB1", "circuits": [{"number": "4", "description": "LIFT", "breaker": "80", "poles": 3, "wire": "feeder 84: 1 set(s) of 4 3 cu, ground 8, in 1.5\" conduit"}]},
                        {"panel": "P2", "circuits": [{"number": "7", "description": "ELECTRIC VEHICLE CHARGER", "breaker": "50", "poles": 2, "wire": "2#6, 1#10 G"}, {"number": "9", "description": "LIFT PIT SUMP PUMP", "breaker": "20", "poles": 1, "wire": "2#12"}]}
                    ],
                    "runs": [
                        {"system": "SAN", "level": "Level 1", "points": [[600, 300]], "size_in": [4, 0], "bottoms_ft": [0], "top_ft": 10, "source_sheet": "P-201"},
                        {"system": "E-BRANCH", "level": "Level 2", "points": [[0, 0], [100, 0]], "size_in": [0.75, 0], "bottoms_ft": [-2]}
                    ]
                },
                "circulation": [{"kind": "hoistway", "level": "Level 1", "name": "ELEV", "at": [300, 100], "rects": [[[290, 90], [310, 110]]]}]
            })
            .to_string(),
        )
        .expect("spec")
    }

    #[test]
    fn called_work_is_placed_from_its_gear_room_or_panel_to_the_site() {
        let sp = spec();
        assert_eq!(slab_level(&sp).as_deref(), Some("Level 1"));
        let rooms = vec![Room {
            level: "Level 1".into(),
            name: "COMM ROOM 105".into(),
            outline: vec![[20.0, 20.0], [80.0, 20.0], [80.0, 80.0], [20.0, 80.0]],
        }];
        let mk = |what: &str, text: &str| Callout {
            trade: "E".into(),
            sheet: "E-100".into(),
            from: "note",
            what: what.into(),
            text: text.into(),
            ..Callout::default()
        };
        let mut prim = mk("service_primary", "TWO (2) 5\" PVC MIN 48\" BELOW GRADE FOR PRIMARY");
        prim.count = 2;
        prim.size_in = Some(5.0);
        prim.cover_in = Some(48.0);
        let mut tel = mk("telecom", "PROVIDE (2) 4\" CONDUITS FROM STREET TELCO TO COMM ROOM");
        tel.count = 2;
        tel.size_in = Some(4.0);
        tel.room = Some("COMM".into());
        let ev = mk("ev_charger", "PROVIDE POWER FOR ELECTRIC VEHICLE CHARGER");
        let lost = mk("telecom", "TO MISSING ROOM");
        let mut lost = lost;
        lost.room = Some("VAULT".into());
        let fb = mk("floor_box", "UNDER SLAB CONDUIT FOR FLOOR BOX");
        let mut sump = mk("storm", "ROUTE SUMP PUMP DISCHARGE UNDERSLAB TO EXTERIOR WALL AND DAYLIGHT");
        sump.trade = "P".into();
        let mut legend = mk("water", "COLD WATER PIPING (BELOW SLAB)");
        legend.trade = "P".into();
        legend.from = "legend";
        let (runs, fates) = place(&sp, &rooms, &[prim, tel, ev, lost, fb, sump, legend], &|c| c.what == "floor_box");
        // The sump's discharge: from the hoistway (its circuit names the lift) to the nearest wall
        // (south, 100 from it) and a foot out, up to daylight.
        let st: Vec<&MepRun> = runs.iter().filter(|r| r.system == "P-UG-ST").collect();
        assert_eq!(st.len(), 2);
        assert_eq!(st[0].points.first(), Some(&[300.0, 100.0]));
        assert_eq!(st[0].points.last(), Some(&[300.0, -12.0]));
        assert_eq!((st[0].size_in[0], st[1].top_ft), (2.0, Some(0.5)));
        assert!(st[0].size_source.starts_with("rule:"));
        let runs: Vec<MepRun> = runs.into_iter().filter(|r| r.system != "P-UG-ST").collect();
        assert!(runs.iter().all(|r| r.system == E_UG && r.level == "Level 1"));
        // The primary: two conduits out past the pad, at 48" cover below the floor to the top
        // (plus the conduit), coming up at the pad outside the east wall (nearest the switchboard).
        let prim_runs: Vec<&MepRun> = runs.iter().filter(|r| r.height_source.contains("service_primary") && r.points.len() >= 2).collect();
        assert_eq!(prim_runs.len(), 2);
        let bottom = prim_runs[0].bottoms_ft[0];
        assert!((bottom + (48.0 + 5.0) / 12.0).abs() < 1e-9, "{bottom}");
        assert!(prim_runs[0].points.iter().all(|p| p[0] > 1200.0), "{:?}", prim_runs[0].points);
        // The telecom conduits: into the room so named, the stub-up a foot above the floor.
        let tel_up: Vec<&MepRun> = runs.iter().filter(|r| r.height_source.contains("telecom") && r.points.len() == 1).collect();
        assert_eq!(tel_up.len(), 2);
        assert_eq!(tel_up[0].points[0], [50.0, 50.0]);
        assert_eq!(tel_up[0].top_ft, Some(1.0));
        // The EV feed out of the panel whose schedule has it, the size assumed and said so.
        let ev_run = runs.iter().find(|r| r.height_source.contains("ev_charger") && r.points.len() >= 2).expect("ev");
        assert_eq!(ev_run.points.first(), Some(&[210.0, 502.0]));
        assert!(ev_run.size_source.contains("smallest"));
        let status: Vec<&str> = fates.iter().map(|f| f.status).collect();
        assert_eq!(status, vec!["placed", "placed", "placed", "not_placed", "placed_elsewhere", "placed", "not_placed"]);
        assert!(fates[6].how.contains("line pattern"));
        assert!(fates[3].how.contains("VAULT"));
        // The note's steel elbows and risers go with its runs.
        let mut steel = mk("service_primary", "PRIMARY");
        steel.steel_ells = true;
        let (sr, _) = place(&sp, &rooms, &[steel], &|_| false);
        assert!(sr.iter().all(|r| r.height_source.contains("steel elbows and risers")));
        // A telecom room upstairs: under the slab to the riser below it, up to it.
        let up =
            vec![Room { level: "Level 2".into(), name: "DATA 201".into(), outline: vec![[20.0, 20.0], [80.0, 20.0], [80.0, 80.0], [20.0, 80.0]] }];
        let mut t2 = mk("telecom", "PROVIDE (1) 4\" CONDUIT FROM STREET TELCO TO DATA ROOM");
        t2.room = Some("DATA".into());
        let (tr, tf) = place(&sp, &up, &[t2], &|_| false);
        assert_eq!(tf[0].status, "placed", "{:?}", tf[0]);
        assert!(tf[0].how.contains("rising to it on Level 2"));
        assert_eq!(tr.iter().find(|r| r.points.len() == 1).and_then(|r| r.top_ft), Some(11.0));
    }

    #[test]
    fn only_runs_under_the_slab_level_are_buried_and_stacks_go_down_to_the_drain() {
        let sp = spec();
        let up = sp.mep.runs.iter().find(|r| r.level == "Level 2").expect("upper");
        // Under Level 2's floor is Level 1's ceiling, not the ground.
        assert!(!below_slab(&sp, up));
        let mut down = up.clone();
        down.level = "Level 1".into();
        assert!(below_slab(&sp, &down));
        let bases = stack_bases(&sp, false);
        assert_eq!(bases.len(), 1);
        assert_eq!((bases[0].system.as_str(), bases[0].top_ft, bases[0].bottoms_ft[0]), ("P-UG-SAN", Some(0.0), -2.0));
        assert!(ug_system(&bases[0].system) && ug_system("E-UG") && !ug_system("E-FEEDER"));
    }

    #[test]
    fn the_slab_levels_untagged_drains_go_under_its_floor_and_the_rest_keep_their_height() {
        let mut sp = spec();
        let run = |system: &str, level: &str, pts: Vec<[f64; 2]>, bottom: f64, source: &str, top: Option<f64>| -> MepRun {
            let mut r = super::super::printcond::run(system, level, "P-101", pts, 4.0, bottom);
            r.height_source = source.into();
            r.top_ft = top;
            r
        };
        let two = vec![[100.0, 100.0], [100.0, 400.0], [500.0, 400.0]];
        sp.mep.runs = vec![
            // Seated in the ceiling by default (no height on the plan): the building drain.
            run("SAN", "Level 1", two.clone(), 8.75, "default", None),
            run("ST", "Level 1", two.clone(), 0.0, "", None),
            // A height the plan gives stays.
            run("SAN", "Level 1", two.clone(), 1.0, "tag", None),
            // Upper floors' drains run in the ceiling below them.
            run("SAN", "Level 2", two.clone(), 8.0, "default", None),
            // Water and vent piping, and a stack (a riser), aren't the building drain.
            run("CW", "Level 1", two.clone(), 8.75, "default", None),
            run("V", "Level 1", two.clone(), 8.75, "default", None),
            run("SAN", "Level 1", vec![[600.0, 300.0]], 0.0, "default", Some(10.0)),
        ];
        let (moved, length) = slab_drains(&mut sp);
        assert_eq!(moved, 2);
        assert!((length - 2.0 * 700.0).abs() < 1e-6, "{length}");
        let invert = -rules().depths.drain_invert_ft;
        for r in &sp.mep.runs[..2] {
            assert!(r.bottoms_ft.iter().all(|b| (b - invert).abs() < 1e-9), "{:?}", r.bottoms_ft);
            assert!(r.height_source.starts_with(SLAB_DRAIN_PREFIX) && r.height_source.contains("invert"), "{}", r.height_source);
            assert!(below_slab(&sp, r));
            assert!(!made_here(r), "a rerun keeps the plan's drains");
        }
        let kept: Vec<f64> = sp.mep.runs[2..].iter().map(|r| r.bottoms_ft[0]).collect();
        assert_eq!(kept, vec![1.0, 8.0, 8.75, 8.75, 0.0]);
        assert!(sp.mep.runs[2..].iter().all(|r| !below_slab(&sp, r)));
        // The stack still gets its base; a rerun finds the drains moved.
        assert_eq!(stack_bases(&sp, false).len(), 1);
        assert_eq!(slab_drains(&mut sp), (0, 0.0));
        // A large job's drains and stacks after thousands of other runs: still found.
        let mut big = spec();
        let filler = run("E-BRANCH", "Level 1", vec![[0.0, 0.0], [10.0, 0.0]], 9.0, "default", None);
        let mut runs = vec![filler; MAX_ITEMS + 1_000];
        runs.push(run("SAN", "Level 1", vec![[100.0, 100.0], [300.0, 100.0]], 8.75, "default", None));
        runs.push(run("SAN", "Level 1", vec![[600.0, 300.0]], 0.0, "default", Some(10.0)));
        big.mep.runs = runs;
        assert_eq!(slab_drains(&mut big).0, 1);
        assert_eq!(stack_bases(&big, false).len(), 1);
        // No slab level, nothing moved.
        let mut none = BuildingSpec::default();
        none.levels.clear();
        none.mep.runs = vec![run("SAN", "Level 1", two, 0.0, "default", None)];
        assert_eq!(slab_drains(&mut none), (0, 0.0));
    }

    #[test]
    fn feeds_on_the_slab_level_are_weighed_with_reasons_and_an_alternative() {
        let sp = spec();
        let fr = FramingAbove { kind: "joist".into(), item: "I-joist 11-7/8\"".into(), members: 40, source: "joist layout".into() };
        let ps = weigh(&sp, Some(&fr), Some("Below slab (26 05 33): PVC"));
        // F1: two 3" raceways under I-joists: under the slab, its alternative overhead.
        let f1 = ps.iter().find(|p| p.subject.contains("F1")).expect("F1");
        assert_eq!(f1.decision, "under_slab");
        assert!(
            f1.reasons.iter().any(|r| r.contains("parallel"))
                && f1.reasons.iter().any(|r| r.contains("I-joist"))
                && f1.reasons.iter().any(|r| r.contains("26 05 33"))
        );
        assert!(f1.alternative.starts_with("overhead"));
        // Nothing routes it yet: the proposal adds its route, stubbed up at both ends.
        let runs = proposal_runs(&sp, f1);
        assert_eq!(runs.len(), 2 * 3);
        assert!(runs.iter().all(|r| r.height_source == "proposed" && r.system == E_UG));
        // F2 (one 1" raceway, short): overhead.
        let f2 = ps.iter().find(|p| p.subject.contains("F2")).expect("F2");
        assert_eq!(f2.decision, "overhead");
        assert!(proposal_runs(&sp, f2).is_empty());
        // The lift's row: to the hoistway on the slab.
        let lift = ps.iter().find(|p| p.subject.contains("LIFT")).expect("lift");
        assert_eq!((lift.to.as_str(), lift.conduits, lift.size_in), ("LIFT", 1, 1.5));
        // No spec method: said, and it counts against.
        let none = weigh(&sp, None, None);
        assert!(none.iter().all(|p| p.reasons.iter().any(|r| r.contains("lists no method"))));
    }

    #[test]
    fn a_rows_feed_and_the_slabs_edge_are_read() {
        assert_eq!(row_feed("feeder 74: 1 set(s) of 4 4 cu, ground 8, in 1.25\" conduit"), Some((1, 1.25, "74".into())));
        assert_eq!(row_feed("feeder 31: 2 set(s) of 4 350 kcmil cu, ground 1/0, in 3\" conduit"), Some((2, 3.0, "31".into())));
        assert_eq!(row_feed("3#12, 1#12 G"), None);
        let b = [0.0, 0.0, 100.0, 50.0];
        assert_eq!(nearest_edge(b, [95.0, 25.0]), ([100.0, 25.0], [1.0, 0.0]));
        assert_eq!(nearest_edge(b, [50.0, 2.0]), ([50.0, 0.0], [0.0, -1.0]));
        assert_eq!(nearest_edge(b, [-20.0, 25.0]), ([0.0, 25.0], [-1.0, 0.0]));
        assert_eq!(sheet_trade("E-101"), Some("E"));
        assert_eq!(sheet_trade("P-201"), Some("P"));
        assert_eq!(sheet_trade("A201"), None);
    }
}
