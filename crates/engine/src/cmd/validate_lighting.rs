//! Switches and lights (Q6145): the lighting rules the prescreen measures with no agent.
//!
//! - **E-LUM-TYPE**: each luminaire's type tag has a row in the fixture schedule and a product in
//!   the submittal, and the model's product fields (maker, catalog number, voltage, wattage,
//!   lumens, CCT, dimming, finish) match them. Where the documents disagree — a plan tag the
//!   fixture schedule doesn't list, a scheduled type the submittal leaves out, a schedule field
//!   the submittal's cut sheet says otherwise — the finding is a `conflict` citing both.
//! - **E-LUM-CONTROL**: each luminaire has a control (a switch, dimmer or sensor of its room on
//!   its switch leg, or one linked to it) unless it is marked emergency or unswitched.
//! - **E-SWITCH-LEG**: each switch, dimmer and sensor reaches luminaires of its own room by its
//!   switch leg (or its link).
//! - **E-SWITCH-DIM**: a dimmer controls only dimmable drivers of its own protocol (0-10 V, ELV,
//!   TRIAC, DALI…).
//! - **E-SWITCH-3WAY**: a 3-way switch has its mate on the same leg in its room (a 4-way needs
//!   two 3-ways).
//! - **E-LUM-COUNT**: per room, the plan's luminaire and control symbols of each type against
//!   the model's.
//!
//! What a mark is comes from the job's documents (the element's own type words, its legend and
//! schedule rows) and the project's rules, never a built-in list of marks. Unknown data is
//! `unsure` with why, never a pass. The job's own review items of kind `conflict` about lighting
//! are carried in as `conflict` findings (source `build`).

use std::collections::{BTreeMap, BTreeSet, HashMap};

use cadaddy_doc::props::{Props, SourceKind};
use cadaddy_doc::{Drawing, Entity};
use cadaddy_walk3d::BuildingSpec;
use cadaddy_walk3d::validate::{Model, Prints, ScheduleRow};
use serde_json::{Value, json};

use super::validate::Frame;
use super::validate_kind::{Kinds, Said};
use super::validate_packet::{Area, inside, point_in, pt, r2};
use super::validate_rules::Effective;

pub const TYPE: &str = "E-LUM-TYPE";
pub const CONTROL: &str = "E-LUM-CONTROL";
pub const LEG: &str = "E-SWITCH-LEG";
pub const DIM: &str = "E-SWITCH-DIM";
pub const THREE_WAY: &str = "E-SWITCH-3WAY";
pub const COUNT: &str = "E-LUM-COUNT";
/// The lighting rules, in the order they are run.
pub const RULES: [&str; 6] = [TYPE, CONTROL, LEG, DIM, THREE_WAY, COUNT];
/// The verdicts each rule gives (listed with 0 when none).
pub const VERDICTS: [(&str, &[&str]); 6] = [
    (TYPE, &["pass", "fail", "conflict", "unsure"]),
    (CONTROL, &["pass", "fail", "conflict", "unsure"]),
    (LEG, &["pass", "fail", "unsure"]),
    (DIM, &["pass", "fail", "unsure"]),
    (THREE_WAY, &["pass", "missing", "unsure"]),
    (COUNT, &["pass", "missing", "extra"]),
];

/// Rows, products, symbols and review items read, at most.
const MAX_ROWS: usize = 200_000;
/// Words of one text looked at; characters of a citation; handles listed per finding.
const MAX_WORDS: usize = 2_000;
const MAX_CITE: usize = 160;
const MAX_LISTED: usize = 30;
/// Longest switch leg and mark read.
const MAX_LEG: usize = 8;
const MAX_MARK: usize = 60;

// ---------------------------------------------------------------- words

fn tokens(s: &str) -> Vec<String> {
    s.split(|c: char| !c.is_ascii_alphanumeric()).filter(|w| !w.is_empty()).take(MAX_WORDS).map(str::to_ascii_lowercase).collect()
}

fn cite(text: &str) -> String {
    let t = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if t.chars().count() > MAX_CITE { format!("{}…", t.chars().take(MAX_CITE).collect::<String>()) } else { t }
}

/// A key as compared: lower case letters and digits only (`catalogNumber` = `catalog_number`).
fn key_of(k: &str) -> String {
    k.chars().filter(char::is_ascii_alphanumeric).take(60).collect::<String>().to_ascii_lowercase()
}

fn mark_key(m: &str) -> Option<String> {
    let m = m.trim();
    (!m.is_empty() && m.len() <= MAX_MARK).then(|| m.to_ascii_uppercase())
}

/// The type a luminaire or control counts as (E-LUM-COUNT): its type mark, else its kind. A tag
/// numbered by its own kind's name (`EXIT_SIGN-3`) names one object, not a type.
fn type_key(mark: Option<&str>, kind: &str) -> String {
    let instance = |m: &str| {
        m.trim().rsplit_once('-').is_some_and(|(stem, n)| !n.is_empty() && n.chars().all(|c| c.is_ascii_digit()) && stem.eq_ignore_ascii_case(kind.trim()))
    };
    mark.filter(|m| !instance(m)).and_then(mark_key).unwrap_or_else(|| kind.to_string())
}

/// Do the words hold `a b` in a row (or `ab` as one word)?
fn pair(w: &[String], a: &str, b: &str) -> bool {
    w.windows(2).any(|x| matches!(x, [p, q] if p == a && q == b)) || w.iter().any(|x| *x == format!("{a}{b}"))
}

// ---------------------------------------------------------------- dimming protocols

/// How a driver or a dimmer dims.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Proto {
    /// Not dimmable.
    NonDim,
    /// Dimmable, the protocol not said.
    Dimmable,
    ZeroTen,
    /// Phase cut, the direction not said.
    Phase,
    /// Reverse (trailing-edge) phase: ELV.
    Elv,
    /// Forward (leading-edge) phase: TRIAC, MLV, incandescent.
    Triac,
    Dali,
    Dmx,
}

impl Proto {
    pub fn name(self) -> &'static str {
        match self {
            Proto::NonDim => "non-dim",
            Proto::Dimmable => "dimmable (protocol not stated)",
            Proto::ZeroTen => "0-10V",
            Proto::Phase => "phase",
            Proto::Elv => "ELV (reverse phase)",
            Proto::Triac => "TRIAC (forward phase)",
            Proto::Dali => "DALI",
            Proto::Dmx => "DMX",
        }
    }
}

/// What a text says of dimming: `0-10V`, `ELV`, `TRIAC`, `non-dim`…; none when it says nothing.
pub fn protocol(text: &str) -> Option<Proto> {
    let squeezed: String = text.chars().filter(|c| !c.is_whitespace()).take(4_000).collect::<String>().to_ascii_lowercase();
    let w = tokens(text);
    let has = |x: &str| w.iter().any(|t| t == x);
    if ["nondim", "non-dim", "notdimmable", "nodimming", "non-dimming", "nondimming", "on/offonly"].iter().any(|x| squeezed.contains(x)) {
        return Some(Proto::NonDim);
    }
    if squeezed.contains("0-10") || squeezed.contains("0–10") || squeezed.contains("1-10v") || has("010v") || squeezed == "010" {
        return Some(Proto::ZeroTen);
    }
    if has("elv") || pair(&w, "reverse", "phase") || pair(&w, "trailing", "edge") || squeezed.contains("electroniclowvoltage") {
        return Some(Proto::Elv);
    }
    if has("triac") || has("mlv") || has("incandescent") || pair(&w, "forward", "phase") || pair(&w, "leading", "edge") {
        return Some(Proto::Triac);
    }
    if has("dali") {
        return Some(Proto::Dali);
    }
    if has("dmx") {
        return Some(Proto::Dmx);
    }
    if has("phase") {
        return Some(Proto::Phase);
    }
    if has("dimmable") || has("dimming") || has("dimmer") || has("dim") {
        return Some(Proto::Dimmable);
    }
    None
}

/// Does a dimmer of protocol `dimmer` dim a driver of protocol `driver`? Yes, no (with why), or
/// not known (with why).
pub fn dims(dimmer: Option<Proto>, driver: Option<Proto>) -> std::result::Result<(), (bool, String)> {
    match (dimmer, driver) {
        (_, Some(Proto::NonDim)) => Err((true, "the driver is not dimmable".into())),
        (_, None) => Err((false, "the driver's dimming is not stated in the model or documents".into())),
        (None | Some(Proto::Dimmable), _) => Err((false, "the dimmer's protocol is not stated".into())),
        (_, Some(Proto::Dimmable)) => Err((false, "the driver is dimmable but its protocol is not stated".into())),
        (Some(a), Some(b)) if a == b => Ok(()),
        (Some(Proto::Phase), Some(Proto::Elv | Proto::Triac)) | (Some(Proto::Elv | Proto::Triac), Some(Proto::Phase)) => {
            Err((false, "a phase dimmer and driver, but which phase (forward or reverse) is not stated for both".into()))
        }
        (Some(a), Some(b)) => Err((true, format!("a {} dimmer does not dim a {} driver", a.name(), b.name()))),
    }
}

// ---------------------------------------------------------------- product fields

/// The product fields compared, with the names the documents give them.
const FIELDS: [(&str, &[&str]); 8] = [
    ("manufacturer", &["manufacturer", "maker", "mfr", "mfg", "brand"]),
    ("catalog", &["catalognumber", "catalog", "catno", "catalogno", "catalognum", "modelnumber"]),
    ("voltage", &["voltage", "volts", "volt", "inputvoltage"]),
    ("wattage", &["wattage", "watts", "inputwatts"]),
    ("lumens", &["lumens", "lumen", "deliveredlumens", "lumenoutput"]),
    ("cct", &["cct", "colortemperature", "colourtemperature", "kelvin", "colortemp"]),
    ("dimming", &["dimming", "dimmingtype", "dimmingprotocol", "driver", "dimmable"]),
    ("finish", &["finish"]),
];

/// The field a document's key is, if one compared.
fn field_of(key: &str) -> Option<&'static str> {
    let k = key_of(key);
    FIELDS.iter().find(|(_, names)| names.contains(&k.as_str())).map(|(f, _)| *f)
}

/// The numbers in a text (`120-277V` → 120, 277).
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

fn volts(text: &str) -> BTreeSet<u32> {
    let mut out: BTreeSet<u32> = numbers(text).into_iter().filter(|n| (90.0..=600.0).contains(n)).map(|n| n.round() as u32).collect();
    let w = tokens(text);
    if w.iter().any(|t| matches!(t.as_str(), "mvolt" | "unv" | "universal" | "univ" | "uvolt")) {
        out.extend([120, 277]);
    }
    out
}

fn kelvin(text: &str) -> Option<f64> {
    let n = numbers(text).into_iter().find(|n| *n > 0.0)?;
    let k = if n < 100.0 { n * 100.0 } else { n };
    (1_000.0..=10_000.0).contains(&k).then_some(k)
}

fn mounting_class(text: &str) -> Option<&'static str> {
    let w = tokens(text);
    let has = |x: &[&str]| w.iter().any(|t| x.contains(&t.as_str()));
    if has(&["recessed", "recess", "troffer", "downlight"]) {
        Some("recessed")
    } else if has(&["pendant", "suspended", "stem", "chain", "cable"]) {
        Some("pendant")
    } else if has(&["wall", "sconce", "bracket", "vanity"]) {
        Some("wall")
    } else if has(&["surface", "ceiling", "flush"]) {
        Some("surface")
    } else if has(&["pole", "bollard", "ground", "landscape"]) {
        Some("site")
    } else if has(&["track"]) {
        Some("track")
    } else {
        None
    }
}

/// Do two documents' values of `field` agree? None when they can't be compared.
pub fn agree(field: &str, a: &str, b: &str) -> Option<bool> {
    let alnum = |s: &str| s.chars().filter(char::is_ascii_alphanumeric).collect::<String>().to_ascii_uppercase();
    let close = |x: f64, y: f64| (x - y).abs() <= (0.02 * x.abs().max(y.abs())).max(1.0);
    match field {
        "manufacturer" => {
            let (wa, wb) = (tokens(a), tokens(b));
            let first = |w: &[String]| w.first().cloned();
            let (fa, fb) = (first(&wa)?, first(&wb)?);
            Some(wb.contains(&fa) || wa.contains(&fb))
        }
        "catalog" => {
            let (x, y) = (alnum(a), alnum(b));
            if x.len() < 3 || y.len() < 3 {
                return None;
            }
            Some(x.starts_with(&y) || y.starts_with(&x))
        }
        "voltage" => {
            let (x, y) = (volts(a), volts(b));
            (!x.is_empty() && !y.is_empty()).then(|| !x.is_disjoint(&y))
        }
        "wattage" | "lumens" => {
            let (x, y) = (numbers(a).into_iter().next()?, numbers(b).into_iter().next()?);
            Some(close(x, y))
        }
        "cct" => Some((kelvin(a)? - kelvin(b)?).abs() <= 50.0),
        "dimming" => {
            let (x, y) = (protocol(a)?, protocol(b)?);
            match (x, y) {
                (Proto::Dimmable, _) | (_, Proto::Dimmable) => (x != Proto::NonDim && y != Proto::NonDim).then_some(true).or(Some(false)),
                (Proto::Phase, Proto::Elv | Proto::Triac) | (Proto::Elv | Proto::Triac, Proto::Phase) => None,
                _ => Some(x == y),
            }
        }
        "mounting" => match (mounting_class(a), mounting_class(b)) {
            (Some(x), Some(y)) => Some(x == y),
            _ => None,
        },
        _ => {
            let (x, y) = (alnum(a), alnum(b));
            if x.is_empty() || y.is_empty() {
                return None;
            }
            Some(x.contains(&y) || y.contains(&x))
        }
    }
}

// ---------------------------------------------------------------- the documents

/// One fixture schedule row of a type.
#[derive(Clone, Debug)]
pub struct Row {
    /// `schedule LIGHTING FIXTURE SCHEDULE on E-601, mark A`.
    pub source: String,
    /// Its fields (a drawn schedule read as text has none, only `text`).
    pub fields: BTreeMap<String, String>,
    pub text: String,
}

impl Row {
    fn field(&self, f: &str) -> Option<&str> {
        self.fields.iter().find(|(k, v)| field_of(k) == Some(f) && !v.trim().is_empty()).map(|(_, v)| v.as_str())
    }
    fn words(&self) -> String {
        if self.text.is_empty() { self.fields.values().take(60).cloned().collect::<Vec<_>>().join("; ") } else { self.text.clone() }
    }
    fn to_json(&self) -> Value {
        if self.fields.is_empty() { json!({"source": self.source, "text": self.text}) } else { json!({"source": self.source, "fields": self.fields}) }
    }
}

/// A type's submittal product: each field with where it says so.
#[derive(Clone, Debug, Default)]
pub struct Product {
    /// field → (value, document).
    pub fields: BTreeMap<&'static str, (String, String)>,
    /// Every field the product holds, as named there (for the packet).
    pub all: BTreeMap<String, String>,
    /// The documents it was read from.
    pub sources: Vec<String>,
    /// Its submittal file and cut-sheet page, when known (`<file> p.<n>`).
    pub pages: BTreeSet<String>,
}

impl Product {
    fn add(&mut self, key: &str, value: &str, doc: &str) {
        let v = value.trim();
        if v.is_empty() || v.len() > 400 {
            return;
        }
        if self.all.len() < 60 {
            self.all.entry(key.to_string()).or_insert_with(|| v.to_string());
        }
        if let Some(f) = field_of(key) {
            self.fields.entry(f).or_insert_with(|| (v.to_string(), doc.to_string()));
        }
        if !self.sources.iter().any(|s| s == doc) && self.sources.len() < 8 {
            self.sources.push(doc.to_string());
        }
    }
    fn is_empty(&self) -> bool {
        self.all.is_empty()
    }
    fn to_json(&self) -> Value {
        json!({"fields": self.all, "sources": self.sources, "cut_sheets": self.pages})
    }
}

/// A page reference in a source (`p.32`, `p. 4`, `page 7`).
fn page_in(s: &str) -> Option<u32> {
    let w = tokens(s);
    w.windows(2).find_map(|x| match x {
        [p, n] if p == "p" || p == "page" || p == "pg" => n.parse::<u32>().ok().filter(|n| *n > 0 && *n < 100_000),
        _ => None,
    })
}

/// A submittal reference's file part (`Lighting.pdf p.3` → `Lighting.pdf`).
fn file_of(s: &str) -> String {
    let t = s.trim();
    match t.rfind(" p.") {
        Some(i) => t.get(..i).unwrap_or(t).trim().to_string(),
        None => t.to_string(),
    }
}

/// Is a schedule (by its name, its row's kind) a lighting fixture schedule? A bare `FIXTURE
/// SCHEDULE` is not (a plumbing fixture schedule is often called that): its rows must say they
/// are lighting ([`lighting_row`]).
fn lighting_schedule(name: &str, kind: Option<&str>) -> bool {
    let w = tokens(name);
    let has = |x: &[&str]| w.iter().any(|t| x.contains(&t.as_str()));
    if kind.is_some_and(|k| cadaddy_walk3d::validate_kinds::in_family(k, "luminaire") || cadaddy_walk3d::validate_kinds::in_family(k, "exit_sign")) {
        return true;
    }
    kind.is_none()
        && !has(&["plumbing", "panel", "door", "window", "finish", "equipment", "mechanical", "pipe"])
        && has(&["luminaire", "luminaires", "lighting", "light"])
}

/// Does a row of a schedule named only `FIXTURE SCHEDULE` say it is a light fixture (lamp,
/// lumens, LED, driver…) and not a plumbing one?
fn lighting_row(fields: &BTreeMap<String, String>) -> bool {
    let w: Vec<String> = fields.iter().take(60).flat_map(|(k, v)| tokens(&format!("{k} {v}"))).collect();
    let has = |x: &[&str]| w.iter().any(|t| x.contains(&t.as_str()));
    has(&["led", "lamp", "lamps", "lumen", "lumens", "lm", "driver", "downlight", "troffer", "sconce", "luminaire", "cct", "kelvin", "ballast"])
        && !has(&["lavatory", "sink", "urinal", "shower", "faucet", "flush", "closet", "drain", "trap", "gpf", "gpm", "tub"])
}

/// Is a legend heading a drawn lighting fixture schedule (not a note that refers to one: `SEE
/// LUMINAIRE SCHEDULE.`)?
fn schedule_heading(heading: &str) -> bool {
    let t = heading.trim();
    let w = tokens(t);
    !t.ends_with('.')
        && w.last().is_some_and(|x| x == "schedule")
        && w.first().is_some_and(|x| !matches!(x.as_str(), "on" | "see" | "per" | "as" | "in" | "to" | "from" | "refer"))
        && lighting_schedule(t, None)
}

/// What the job's documents say of its lighting: the fixture schedule's rows and the submittal's
/// products by type mark.
pub struct Docs {
    pub schedule: HashMap<String, Vec<Row>>,
    /// The documents hold a fixture schedule (some type has a row).
    pub schedule_read: bool,
    pub schedule_sheets: BTreeSet<String>,
    pub submittal: HashMap<String, Product>,
    /// The documents hold a submittal for some luminaire type.
    pub submittal_read: bool,
}

/// Is a BIM type field's source a submittal page (`p.32`, `submittal …`)?
fn submittal_source(s: &str) -> bool {
    let t = s.trim().to_ascii_lowercase();
    t.starts_with("p.") || t.starts_with("p ") || t.contains("submittal") || t.contains("cut sheet")
}

impl Docs {
    /// Read the documents; `lum_marks` are the marks the model's luminaires carry, `file_of_mark`
    /// the submittal file their elements name.
    pub fn of(d: &Drawing, kinds: &Kinds, prints: Option<&Prints>, lum_marks: &BTreeSet<String>, file_of_mark: &HashMap<String, String>) -> Docs {
        let mut schedule: HashMap<String, Vec<Row>> = HashMap::new();
        let mut sheets = BTreeSet::new();
        let rows: &[ScheduleRow] = prints.map_or(&[][..], |p| p.schedule.as_slice());
        for r in rows.iter().take(MAX_ROWS) {
            let Some(m) = mark_key(&r.mark) else { continue };
            let lit = lighting_schedule(&r.schedule, r.kind.as_deref())
                || (tokens(&r.schedule).iter().any(|t| t == "fixture" || t == "fixtures") && r.kind.is_none() && lighting_row(&r.fields));
            if !lit {
                continue;
            }
            sheets.insert(r.sheet.clone());
            let source = cite(&format!("schedule {} on {}, mark {}", r.schedule, r.sheet, r.mark));
            let list = schedule.entry(m).or_default();
            if list.len() < 4 {
                list.push(Row { source, fields: r.fields.clone(), text: String::new() });
            }
        }
        // A fixture schedule drawn on a sheet (read as legend rows under its heading).
        for (m, said) in kinds.legend_said() {
            let Some(h) = said.heading.as_deref().filter(|h| schedule_heading(h)) else { continue };
            let list = schedule.entry(m.clone()).or_default();
            if list.len() < 4 && !list.iter().any(|r| r.text == said.text) {
                list.push(Row { source: cite(&format!("{h} (drawn), row {m}")), fields: BTreeMap::new(), text: said.text.clone() });
            }
        }
        let schedule_read = !schedule.is_empty();
        let mut submittal: HashMap<String, Product> = HashMap::new();
        for s in prints.map_or(&[][..], |p| p.submittals.as_slice()).iter().take(MAX_ROWS) {
            let Some(m) = mark_key(&s.mark) else { continue };
            let doc =
                if s.section.trim().is_empty() { format!("submittal, type {m}") } else { format!("submittal (spec {}), type {m}", cite(&s.section)) };
            let p = submittal.entry(m).or_default();
            for (k, v) in s.fields.iter().take(60) {
                p.add(k, v, &doc);
            }
        }
        for t in d.bim_types.types.iter().take(MAX_ROWS) {
            let Some(m) = mark_key(&t.mark) else { continue };
            let is_lum = cadaddy_walk3d::validate_kinds::in_family(&t.category, "luminaire")
                || (super::validate_kind::is_generic(&t.category) && lum_marks.contains(&m));
            if !is_lum {
                continue;
            }
            let file = file_of_mark.get(&m).map(|f| file_of(f));
            let p = submittal.entry(m.clone()).or_default();
            for (k, f) in t.fields.iter().take(60) {
                if matches!(key_of(k).as_str(), "reviewstatus" | "submittalsource") || !submittal_source(&f.source) {
                    continue;
                }
                let page = page_in(&f.source);
                let doc = match (&file, page) {
                    (Some(file), Some(n)) => format!("submittal {file} p.{n}"),
                    (None, Some(n)) => format!("submittal p.{n}"),
                    (Some(file), None) => format!("submittal {file}"),
                    (None, None) => "submittal".to_string(),
                };
                if let (Some(file), Some(n)) = (&file, page)
                    && p.pages.len() < 4
                {
                    p.pages.insert(format!("{file} p.{n}"));
                }
                p.add(k, &f.value, &doc);
            }
        }
        submittal.retain(|_, p| !p.is_empty());
        let submittal_read = !submittal.is_empty();
        Docs { schedule, schedule_read, schedule_sheets: sheets, submittal, submittal_read }
    }
}

// ---------------------------------------------------------------- what is screened

/// One element the lighting rules look at.
pub(crate) struct Lit<'a> {
    pub handle: &'a str,
    pub kind: &'a str,
    pub kind_from: &'static str,
    pub category: Option<&'a str>,
    pub mark: Option<&'a str>,
    pub name: Option<&'a str>,
    pub level: &'a str,
    pub at: [f64; 2],
    pub built: bool,
    pub fields: Option<&'a BTreeMap<String, String>>,
    pub entity: Option<&'a Entity>,
    /// Its room (inside it, or on its wall line).
    pub room: Option<&'a Area>,
}

/// An element's own words and values: its properties, its source object's, its model fields.
struct Own {
    props: Props,
    source: Props,
}

impl Own {
    fn of(d: &Drawing, e: Option<&Entity>) -> Own {
        let props = e.map(|e| cadaddy_doc::props::read(&e.common.xdata)).unwrap_or_default();
        let source = e
            .and_then(super::bimlook::made_source)
            .and_then(|h| d.model.get(h))
            .map(|s| cadaddy_doc::props::read(&s.common.xdata))
            .unwrap_or_default();
        Own { props, source }
    }

    /// The first of `keys` with a value: (value, its source kind, its document).
    fn get(&self, it: &Lit, keys: &[&str]) -> Option<(String, Option<SourceKind>, Option<String>)> {
        for p in [&self.props, &self.source] {
            for (k, prop) in p.iter().take(400) {
                if keys.iter().any(|x| key_of(x) == key_of(k)) && prop.has_value() {
                    return Some((prop.text(), Some(prop.source.kind), prop.source.doc.clone()));
                }
            }
        }
        it.fields
            .into_iter()
            .flatten()
            .take(400)
            .find(|(k, v)| keys.iter().any(|x| key_of(x) == key_of(k)) && !v.trim().is_empty())
            .map(|(_, v)| (v.clone(), None, None))
    }

    fn text(&self, it: &Lit, keys: &[&str]) -> Option<String> {
        self.get(it, keys).map(|(v, _, _)| v.trim().to_string()).filter(|v| !v.is_empty())
    }

    /// Its own type words: kind, type, device type, legend, description, name.
    fn words(&self, it: &Lit) -> Vec<String> {
        let mut out: Vec<String> = vec![it.kind.replace('_', " ")];
        for k in ["type", "device_type", "legend", "description", "type_name", "legend_kind", "device_kind"] {
            if let Some(v) = self.text(it, &[k]) {
                out.push(v);
            }
        }
        if let Some(n) = it.name {
            out.push(n.to_string());
        }
        out
    }

    fn leg(&self, it: &Lit) -> Option<String> {
        self.text(it, &["switch_leg", "switchleg", "leg"]).map(|l| l.to_ascii_lowercase()).filter(|l| l.len() <= MAX_LEG)
    }

    /// The handles a link property names.
    fn links(&self, it: &Lit, keys: &[&str]) -> Vec<String> {
        self.text(it, keys)
            .map(|t| t.split([',', ';', ' ']).map(str::trim).filter(|x| !x.is_empty()).take(MAX_LISTED).map(str::to_ascii_uppercase).collect())
            .unwrap_or_default()
    }
}

/// A 3-way (3) or 4-way (4) switch, by its words.
fn ways(texts: &[String]) -> Option<u8> {
    for t in texts {
        let w = tokens(t);
        if pair(&w, "3", "way") || pair(&w, "three", "way") || w.iter().any(|x| x == "threeway") {
            return Some(3);
        }
        if pair(&w, "4", "way") || pair(&w, "four", "way") || w.iter().any(|x| x == "fourway") {
            return Some(4);
        }
    }
    None
}

/// Is a control a dimmer, by its words?
fn says_dimmer(texts: &[String]) -> bool {
    texts.iter().any(|t| {
        let w = tokens(t);
        w.iter().any(|x| x == "dimmer" || x == "dimmers")
            || (w.iter().any(|x| x == "dimming" || x == "dim") && w.iter().any(|x| x == "switch" || x == "control"))
    })
}

/// Why a luminaire needs no control: its emergency or unswitched marking, if its words say so.
fn no_control_needed(kind: &str, texts: &[(String, String)]) -> Option<String> {
    if cadaddy_walk3d::validate_kinds::in_family(kind, "exit_sign") {
        return Some("an exit sign: always lit".into());
    }
    for (place, t) in texts {
        let w = tokens(t);
        for (i, x) in w.iter().enumerate() {
            let hit = matches!(x.as_str(), "emergency" | "emer" | "emerg" | "em" | "unswitched" | "nightlight" | "24hr" | "battery")
                || (x == "night" && w.get(i + 1).is_some_and(|n| n == "light"))
                || (x == "always" && w.get(i + 1).is_some_and(|n| n == "on"));
            let denied = i > 0 && w.get(i - 1).is_some_and(|p| p == "non" || p == "not" || p == "no");
            if hit && !denied {
                return Some(format!("{place} says \"{}\"", cite(t)));
            }
        }
    }
    None
}

/// What the lighting rules found.
#[derive(Default)]
pub(crate) struct Found {
    /// (rank, finding).
    pub findings: Vec<(u8, Value)>,
    pub checked: BTreeMap<String, BTreeMap<String, u64>>,
    pub unmeasured: BTreeMap<String, BTreeMap<String, u64>>,
}

impl Found {
    fn tally(&mut self, rule: &str, verdict: &str) {
        *self.checked.entry(rule.to_string()).or_default().entry(verdict.to_string()).or_default() += 1;
    }
    fn skip(&mut self, rule: &str, why: &str) {
        *self.unmeasured.entry(rule.to_string()).or_default().entry(why.to_string()).or_default() += 1;
    }
}

fn rank(verdict: &str, severity: &str) -> u8 {
    match verdict {
        "pass" => 9,
        "unsure" => 5,
        "conflict" => 3,
        "extra" => 2,
        _ => match severity {
            "high" => 0,
            "medium" => 1,
            _ => 2,
        },
    }
}

#[allow(clippy::too_many_arguments)]
fn finding(r: &Effective, it: &Lit, verdict: &str, issue: String, expected: String, found: String, measured: Value) -> (u8, Value) {
    let id = format!("PS-{}", super::validate_run::hash8(&format!("{}|{}|{verdict}", r.rule.id, it.handle)));
    let f = json!({
        "id": id,
        "rule": r.rule.id,
        "handle": it.handle,
        "kind": it.kind,
        "kind_from": it.kind_from,
        "mark": it.mark,
        "built": it.built,
        "verdict": verdict,
        "issue": issue,
        "expected": expected,
        "found": found,
        "measured": measured,
        "evidence": [],
        "confidence": 1.0,
        "severity": r.rule.severity,
        "area": it.room.map(|a| a.id.clone()),
        "level": it.level,
        "at": pt([r2(it.at[0]), r2(it.at[1])]),
        "source": "prescreen",
    });
    (rank(verdict, &r.rule.severity), f)
}

/// The room key of an area: its level and its id before `#`.
fn room_key(a: &Area) -> String {
    format!("{}|{}", a.level.to_ascii_lowercase(), a.id.split('#').next().unwrap_or(&a.id))
}

fn room_label(a: &Area) -> String {
    match (&a.number, &a.name) {
        (Some(n), Some(name)) => format!("{name} {n}"),
        (Some(n), None) => format!("room {n}"),
        (None, Some(name)) => name.clone(),
        (None, None) => a.id.clone(),
    }
}

/// Everything the rules need of one luminaire or control.
struct Fact<'a> {
    it: &'a Lit<'a>,
    own: Own,
    lum: bool,
    leg: Option<String>,
    room: Option<String>,
}

/// The inputs of the lighting screen.
pub(crate) struct Input<'a> {
    pub d: &'a Drawing,
    pub sp: &'a BuildingSpec,
    pub frame: &'a Frame,
    pub prints: Option<&'a Prints>,
    pub kinds: &'a Kinds,
    pub rules: &'a [Effective],
    /// The rooms in scope.
    pub areas: &'a [Area],
    /// Only these rules (empty: all).
    pub only: &'a [String],
}

/// Is the rule `id` on (and asked for)?
fn rule_on<'r>(inp: &'r Input, id: &str) -> Option<&'r Effective> {
    inp.rules.iter().find(|r| r.rule.id == id && r.enabled).filter(|_| inp.only.is_empty() || inp.only.iter().any(|o| o == id))
}

/// Is `it` a luminaire (by E-LUM-TYPE's kinds) or a control (by E-SWITCH-LEG's)?
fn class(rules: &[Effective], it: &Lit) -> (bool, bool) {
    let applies = |id: &str| rules.iter().find(|r| r.rule.id == id).is_some_and(|r| r.rule.applies_to_element(it.kind, it.category, it.mark));
    let lum = applies(TYPE);
    (lum, !lum && applies(LEG))
}

/// Run the lighting rules over `items` (every element in scope, with its room).
pub(crate) fn screen(inp: &Input, items: &[Lit]) -> Found {
    let mut out = Found::default();
    for (id, verdicts) in VERDICTS {
        if rule_on(inp, id).is_some() {
            out.checked.insert(id.to_string(), verdicts.iter().map(|v| (v.to_string(), 0)).collect());
        }
    }
    if out.checked.is_empty() {
        return out;
    }
    let facts: Vec<Fact> = items
        .iter()
        .filter_map(|it| {
            let (lum, ctl) = class(inp.rules, it);
            (lum || ctl).then(|| {
                let own = Own::of(inp.d, it.entity);
                let leg = own.leg(it);
                Fact { it, own, lum, leg, room: it.room.map(room_key) }
            })
        })
        .collect();
    let lum_marks: BTreeSet<String> = facts.iter().filter(|f| f.lum).filter_map(|f| f.it.mark.and_then(mark_key)).collect();
    let mut file_of_mark: HashMap<String, String> = HashMap::new();
    for f in facts.iter().filter(|f| f.lum) {
        if let (Some(m), Some(s)) = (f.it.mark.and_then(mark_key), f.own.text(f.it, &["submittal"])) {
            file_of_mark.entry(m).or_insert(s);
        }
    }
    let docs = Docs::of(inp.d, inp.kinds, inp.prints, &lum_marks, &file_of_mark);
    // Who controls what: a control reaches its room's luminaires on its leg (an unlettered
    // control the unlettered ones), and any it or they are linked to.
    let by_handle: HashMap<String, usize> = facts.iter().enumerate().map(|(i, f)| (f.it.handle.to_ascii_uppercase(), i)).collect();
    let mut controls: Vec<BTreeSet<usize>> = vec![BTreeSet::new(); facts.len()];
    let mut controlled_by: Vec<BTreeSet<usize>> = vec![BTreeSet::new(); facts.len()];
    let mut by_room: HashMap<&str, Vec<usize>> = HashMap::new();
    for (i, f) in facts.iter().enumerate() {
        if let Some(r) = &f.room {
            by_room.entry(r.as_str()).or_default().push(i);
        }
    }
    for (c, f) in facts.iter().enumerate().filter(|(_, f)| !f.lum) {
        let mut reach: BTreeSet<usize> = f
            .own
            .links(f.it, &["controls"])
            .iter()
            .filter_map(|h| by_handle.get(h).copied())
            .filter(|i| facts.get(*i).is_some_and(|x| x.lum))
            .collect();
        if let Some(room) = &f.room {
            for &l in by_room.get(room.as_str()).into_iter().flatten() {
                if facts.get(l).is_some_and(|x| x.lum && x.leg == f.leg) {
                    reach.insert(l);
                }
            }
        }
        for l in &reach {
            if let Some(s) = controlled_by.get_mut(*l) {
                s.insert(c);
            }
        }
        if let Some(s) = controls.get_mut(c) {
            *s = reach;
        }
    }
    for (l, f) in facts.iter().enumerate().filter(|(_, f)| f.lum) {
        for c in f.own.links(f.it, &["controlled_by", "controlledby"]).iter().filter_map(|h| by_handle.get(h).copied()) {
            if facts.get(c).is_some_and(|x| !x.lum) {
                if let Some(s) = controlled_by.get_mut(l) {
                    s.insert(c);
                }
                if let Some(s) = controls.get_mut(c) {
                    s.insert(l);
                }
            }
        }
    }
    let label = |i: usize| facts.get(i).map(|f| format!("{} {}", f.it.mark.unwrap_or(f.it.kind), f.it.handle)).unwrap_or_default();
    let list = |s: &BTreeSet<usize>| s.iter().take(MAX_LISTED).map(|i| label(*i)).collect::<Vec<_>>();

    // E-LUM-TYPE.
    if let Some(r) = rule_on(inp, TYPE) {
        for f in facts.iter().filter(|f| f.lum) {
            let (verdict, issue, measured, documents) = lum_type(&docs, f);
            if verdict == "unsure" {
                for why in measured["unknown"].as_array().into_iter().flatten().filter_map(Value::as_str) {
                    out.skip(TYPE, why);
                }
            }
            out.tally(TYPE, verdict);
            let found = measured["type"].as_str().map_or_else(|| "no type tag".to_string(), |m| format!("type {m}"));
            let (rk, mut v) = finding(
                r,
                f.it,
                verdict,
                issue,
                "a fixture schedule row and a submittal product for its type tag, its product fields as they say".into(),
                found,
                measured,
            );
            if !documents.is_empty() {
                v["documents"] = json!(documents);
            }
            out.findings.push((rk, v));
        }
    }
    // E-LUM-CONTROL.
    if let Some(r) = rule_on(inp, CONTROL) {
        for (l, f) in facts.iter().enumerate().filter(|(_, f)| f.lum) {
            let by = controlled_by.get(l).cloned().unwrap_or_default();
            let mut texts: Vec<(String, String)> = f.own.words(f.it).into_iter().map(|t| ("its type".to_string(), t)).collect();
            for k in ["emergency", "unswitched", "circuit_note", "notes"] {
                if let Some(v) = f.own.text(f.it, &[k]) {
                    let flag = matches!(v.to_ascii_lowercase().as_str(), "true" | "yes" | "y" | "1");
                    texts.push((format!("its {k} property"), if flag { k.to_string() } else { v }));
                }
            }
            if let Some(m) = f.it.mark.and_then(mark_key) {
                for row in docs.schedule.get(&m).into_iter().flatten() {
                    texts.push((row.source.clone(), row.words()));
                }
                if let Some(p) = docs.submittal.get(&m) {
                    texts.extend(
                        p.all
                            .iter()
                            .filter(|(k, _)| matches!(key_of(k).as_str(), "description" | "type" | "notes" | "options"))
                            .map(|(_, v)| ("its submittal".to_string(), v.clone())),
                    );
                }
                texts.extend(inp.kinds.said_of(Some(&m), f.it.entity).into_iter().map(|s| (s.source.clone(), s.text.clone())));
            }
            let free = no_control_needed(f.it.kind, &texts);
            let room = f.it.room;
            let leg = f.leg.as_deref();
            let (verdict, issue) = if let Some(why) = &free {
                ("pass", format!("no control needed: {why}"))
            } else if !by.is_empty() {
                ("pass", format!("controlled by {}", list(&by).join(", ")))
            } else if room.is_none() {
                ("unsure", "not in a room: its control is not known".to_string())
            } else if leg.is_none()
                && f.room.as_ref().is_some_and(|rk| by_room.get(rk.as_str()).into_iter().flatten().any(|i| facts.get(*i).is_some_and(|x| !x.lum)))
            {
                ("unsure", "it has no switch leg and its room's controls all have one: which controls it is not stated".to_string())
            } else {
                let place = room.map(room_label).unwrap_or_default();
                (
                    "fail",
                    match leg {
                        Some(l) => format!(
                            "no switch, dimmer or sensor in {place} is on its switch leg {l}: nothing controls it, and nothing marks it emergency or unswitched"
                        ),
                        None => format!("no switch, dimmer or sensor in {place} controls it, and nothing marks it emergency or unswitched"),
                    },
                )
            };
            if verdict == "unsure" {
                out.skip(CONTROL, &issue);
            }
            out.tally(CONTROL, verdict);
            let measured = json!({"switch_leg": leg, "controlled_by": list(&by), "no_control_needed": free, "room": room.map(|a| a.id.clone())});
            let found = if by.is_empty() { "no control".to_string() } else { format!("{} control(s)", by.len()) };
            out.findings.push(finding(
                r,
                f.it,
                verdict,
                issue,
                "a control (switch, dimmer or sensor) of its room, unless emergency or unswitched".into(),
                found,
                measured,
            ));
        }
    }
    // E-SWITCH-LEG.
    if let Some(r) = rule_on(inp, LEG) {
        for (c, f) in facts.iter().enumerate().filter(|(_, f)| !f.lum) {
            let reach = controls.get(c).cloned().unwrap_or_default();
            let elsewhere: Vec<usize> = reach.iter().copied().filter(|l| facts.get(*l).is_some_and(|x| x.room != f.room)).collect();
            let room_lums = f
                .room
                .as_ref()
                .map(|rk| by_room.get(rk.as_str()).into_iter().flatten().filter(|i| facts.get(**i).is_some_and(|x| x.lum)).count())
                .unwrap_or(0);
            let place = f.it.room.map(room_label).unwrap_or_default();
            let leg = f.leg.as_deref();
            let (verdict, issue) = if f.it.room.is_none() {
                ("unsure", "not in a room: what it controls is not known".to_string())
            } else if !elsewhere.is_empty() {
                ("fail", format!("it controls fixtures outside {place}: {}", elsewhere.iter().map(|i| label(*i)).collect::<Vec<_>>().join(", ")))
            } else if !reach.is_empty() {
                ("pass", format!("switch leg {} controls {}", leg.unwrap_or("(none)"), list(&reach).join(", ")))
            } else if leg.is_none() && room_lums > 0 {
                ("unsure", "it has no switch leg and its room's fixtures all have one: what it controls is not stated".to_string())
            } else if room_lums == 0 {
                ("fail", format!("{place} has no light fixture for it to control"))
            } else {
                ("fail", format!("switch leg {} controls no fixture in {place}", leg.unwrap_or_default()))
            };
            if verdict == "unsure" {
                out.skip(LEG, &issue);
            }
            out.tally(LEG, verdict);
            let measured = json!({"switch_leg": leg, "controls": list(&reach), "room": f.it.room.map(|a| a.id.clone())});
            out.findings.push(finding(
                r,
                f.it,
                verdict,
                issue,
                "luminaires of its own room on its switch leg".into(),
                format!("{} fixture(s)", reach.len()),
                measured,
            ));
        }
    }
    // E-SWITCH-DIM.
    if let Some(r) = rule_on(inp, DIM) {
        for (c, f) in facts.iter().enumerate().filter(|(_, f)| !f.lum) {
            let mut words = f.own.words(f.it);
            words.extend(inp.kinds.said_of(f.it.mark, f.it.entity).into_iter().map(|s| s.text.clone()));
            let own_proto = f.own.text(f.it, &["dimming", "protocol", "dimmer_type", "dimming_protocol"]);
            if !(says_dimmer(&words) || own_proto.is_some()) {
                continue;
            }
            let reach = controls.get(c).cloned().unwrap_or_default();
            if reach.is_empty() {
                out.skip(DIM, "a dimmer that controls no fixture (see E-SWITCH-LEG)");
                continue;
            }
            let (proto, proto_from) = match own_proto.as_deref().and_then(protocol) {
                Some(p) => (Some(p), "its dimming property".to_string()),
                None => match words.iter().find_map(|w| protocol(w).filter(|p| *p != Proto::Dimmable).map(|p| (p, w.clone()))) {
                    Some((p, w)) => (Some(p), format!("its type \"{}\"", cite(&w))),
                    None => (None, "not stated".to_string()),
                },
            };
            let mut fails = Vec::new();
            let mut unknown = Vec::new();
            let mut drivers = Vec::new();
            for l in &reach {
                let Some(lf) = facts.get(*l) else { continue };
                let (dp, from) = driver_proto(&docs, lf);
                drivers.push(json!({"fixture": label(*l), "dimming": dp.map(Proto::name), "from": from}));
                match dims(proto, dp) {
                    Ok(()) => {}
                    Err((true, why)) => fails.push(format!("{}: {why}", label(*l))),
                    Err((false, why)) => unknown.push(format!("{}: {why}", label(*l))),
                }
            }
            let (verdict, issue) = if !fails.is_empty() {
                (
                    "fail",
                    format!("the dimmer ({}) controls a driver it can't dim: {}", proto.map_or("protocol not stated", Proto::name), fails.join("; ")),
                )
            } else if !unknown.is_empty() {
                ("unsure", unknown.join("; "))
            } else {
                ("pass", format!("a {} dimmer on {} driver(s) of its protocol", proto.map_or("", Proto::name), reach.len()))
            };
            if verdict == "unsure" {
                let why = if proto.is_none() { "the dimmer's protocol is not stated" } else { "a driver's dimming is not stated" };
                out.skip(DIM, why);
            }
            out.tally(DIM, verdict);
            let measured = json!({"protocol": proto.map(Proto::name), "protocol_from": proto_from, "drivers": drivers});
            out.findings.push(finding(
                r,
                f.it,
                verdict,
                issue,
                "dimmable drivers of the dimmer's protocol".into(),
                format!("{} driver(s)", reach.len()),
                measured,
            ));
        }
    }
    // E-SWITCH-3WAY.
    if let Some(r) = rule_on(inp, THREE_WAY) {
        let way: Vec<Option<u8>> = facts
            .iter()
            .map(|f| {
                if f.lum {
                    return None;
                }
                let mut words = f.own.words(f.it);
                words.extend(inp.kinds.said_of(f.it.mark, f.it.entity).into_iter().map(|s| s.text.clone()));
                ways(&words)
            })
            .collect();
        for (c, f) in facts.iter().enumerate() {
            let Some(n) = way.get(c).copied().flatten() else { continue };
            let Some(room) = &f.room else {
                out.skip(THREE_WAY, "not in a room: its mate is not known");
                out.tally(THREE_WAY, "unsure");
                out.findings.push(finding(
                    r,
                    f.it,
                    "unsure",
                    "not in a room: its mate is not known".into(),
                    "its mate on the same leg".into(),
                    "not known".into(),
                    json!({"ways": n}),
                ));
                continue;
            };
            let set: Vec<usize> = by_room
                .get(room.as_str())
                .into_iter()
                .flatten()
                .copied()
                .filter(|i| way.get(*i).copied().flatten().is_some())
                .filter(|i| facts.get(*i).is_some_and(|x| x.leg == f.leg || x.leg.is_none() || f.leg.is_none()))
                .collect();
            let threes: Vec<usize> = set.iter().copied().filter(|i| way.get(*i).copied().flatten() == Some(3)).collect();
            let enough = threes.iter().filter(|i| **i != c).count() >= if n == 3 { 1 } else { 2 };
            let leg = f.leg.as_deref().unwrap_or("(none)");
            let place = f.it.room.map(room_label).unwrap_or_default();
            let (verdict, issue) = if enough {
                ("pass", format!("a {n}-way set on leg {leg}: {}", set.iter().map(|i| label(*i)).collect::<Vec<_>>().join(", ")))
            } else if n == 3 {
                ("missing", format!("a 3-way switch on leg {leg} in {place} with no mate: its other 3-way switch is missing"))
            } else {
                ("missing", format!("a 4-way switch on leg {leg} in {place} without two 3-way switches"))
            };
            out.tally(THREE_WAY, verdict);
            let measured = json!({"ways": n, "switch_leg": f.leg, "set": set.iter().map(|i| label(*i)).collect::<Vec<_>>()});
            out.findings.push(finding(
                r,
                f.it,
                verdict,
                issue,
                if n == 3 { "a second 3-way switch on its leg".into() } else { "two 3-way switches on its leg".into() },
                format!("{} in the set", set.len()),
                measured,
            ));
        }
    }
    // E-LUM-COUNT.
    if let Some(r) = rule_on(inp, COUNT) {
        counts(inp, r, &facts, &mut out);
    }
    // The job's own conflict review items about lighting.
    carried(inp, &facts, &mut out);
    out
}

/// A luminaire's driver dimming and where it comes from.
fn driver_proto(docs: &Docs, f: &Fact) -> (Option<Proto>, String) {
    if let Some((v, kind, doc)) = f.own.get(f.it, &["dimming", "driver", "dimming_protocol", "dimmable"])
        && let Some(p) = protocol(&v)
    {
        let src = doc.map(|d| format!(" ({})", cite(&d))).or(kind.map(|k| format!(" ({})", k.as_str()))).unwrap_or_default();
        return (Some(p), format!("its dimming property{src}"));
    }
    if let Some(p) = super::validate_tr::product_of(f.it.entity, f.it.fields)
        .and_then(|v| {
            v.get("resolved").and_then(|r| ["dimming", "driver"].iter().find_map(|k| r.get(*k).and_then(Value::as_str).map(str::to_string)))
        })
        .and_then(|t| protocol(&t))
    {
        return (Some(p), "its product record".into());
    }
    let Some(m) = f.it.mark.and_then(mark_key) else { return (None, "not stated (no type tag)".into()) };
    if let Some((v, doc)) = docs.submittal.get(&m).and_then(|p| p.fields.get("dimming"))
        && let Some(p) = protocol(v)
    {
        return (Some(p), doc.clone());
    }
    for row in docs.schedule.get(&m).into_iter().flatten() {
        if let Some(p) = row.field("dimming").and_then(protocol).or_else(|| protocol(&row.words())) {
            return (Some(p), row.source.clone());
        }
    }
    (None, "not stated in the model or documents".into())
}

/// E-LUM-TYPE of one luminaire: (verdict, issue, measured, documents of its conflicts).
fn lum_type(docs: &Docs, f: &Fact) -> (&'static str, String, Value, Vec<Value>) {
    let Some(mark) = f.it.mark.and_then(mark_key) else {
        let why = "it has no type tag";
        return ("unsure", why.into(), json!({"type": null, "unknown": [why]}), Vec::new());
    };
    let plan = f
        .own
        .props
        .iter()
        .find(|(k, p)| matches!(key_of(k).as_str(), "typemark" | "type") && p.source.kind == SourceKind::Print)
        .and_then(|(_, p)| p.source.doc.clone())
        .map_or_else(|| format!("the plan's type tag {mark}"), |s| format!("plan {s}, type tag {mark}"));
    let rows: &[Row] = docs.schedule.get(&mark).map_or(&[], Vec::as_slice);
    // The submittal's product: the documents', else what the element holds from its submittal.
    let mut product = docs.submittal.get(&mark).cloned().unwrap_or_default();
    for (k, prop) in f.own.props.iter().take(400) {
        if prop.source.kind == SourceKind::Submittal && prop.has_value() && field_of(k).is_some() {
            let doc = prop.source.doc.clone().map_or_else(|| "submittal".to_string(), |d| format!("submittal {}", cite(&d)));
            product.add(k, &prop.text(), &doc);
        }
    }
    if let Some(rec) = super::validate_tr::product_of(f.it.entity, f.it.fields)
        && let Some(res) = rec.get("resolved").and_then(Value::as_object)
    {
        let doc = res
            .get("source")
            .and_then(Value::as_str)
            .map_or_else(|| "its product record".to_string(), |s| format!("its product record ({})", cite(s)));
        for (k, v) in res.iter().take(60) {
            if let Some(t) = v.as_str() {
                product.add(k, t, &doc);
            }
        }
    }
    let mut conflicts: Vec<Value> = Vec::new();
    let mut documents: Vec<Value> = Vec::new();
    let mut fails: Vec<Value> = Vec::new();
    let mut unknown: Vec<String> = Vec::new();
    let mut doc_pair = |a: (&str, String), b: (&str, String)| {
        for (doc, says) in [a, b] {
            let d = json!({"document": doc, "says": says});
            if !documents.contains(&d) && documents.len() < 12 {
                documents.push(d);
            }
        }
    };
    if rows.is_empty() {
        if docs.schedule_read {
            let sched = docs.schedule_sheets.iter().take(4).cloned().collect::<Vec<_>>().join(", ");
            let sched = if sched.is_empty() { "the fixture schedule".to_string() } else { format!("the fixture schedule ({sched})") };
            conflicts.push(json!({"field": "type", "issue": format!("type {mark} is on the plan but not in {sched}")}));
            doc_pair((&plan, format!("type {mark}")), (&sched, format!("no row {mark}")));
        } else {
            unknown.push("the documents read hold no fixture schedule".to_string());
        }
    }
    if product.is_empty() {
        if docs.submittal_read && !rows.is_empty() {
            let row = rows.first().map(|r| r.source.clone()).unwrap_or_default();
            conflicts.push(
                json!({"field": "submittal", "issue": format!("the fixture schedule has type {mark} but the submittal has no product for it")}),
            );
            doc_pair((&row, format!("type {mark}")), ("the submittal", format!("no product for type {mark}")));
        } else if !docs.submittal_read {
            unknown.push("no submittal product read for any luminaire type".to_string());
        } else {
            unknown.push(format!("no submittal product for type {mark}"));
        }
    }
    // Schedule against submittal, field by field.
    let mut want: BTreeMap<&str, (String, String)> = BTreeMap::new();
    for (field, _) in FIELDS {
        let s = rows.iter().find_map(|r| r.field(field).map(|v| (v.to_string(), r.source.clone())));
        let p = product.fields.get(field).cloned();
        match (s, p) {
            (Some(s), Some(p)) => {
                match agree(field, &s.0, &p.0) {
                    Some(false) => {
                        conflicts.push(json!({"field": field, "issue": format!("{field}: the schedule says {} but the submittal says {}", cite(&s.0), cite(&p.0))}));
                        doc_pair((&s.1, format!("{field} {}", cite(&s.0))), (&p.1, format!("{field} {}", cite(&p.0))));
                    }
                    _ => {
                        want.insert(field, p);
                    }
                }
            }
            (Some(x), None) | (None, Some(x)) => {
                want.insert(field, x);
            }
            (None, None) => {}
        }
    }
    // The model's own values (not its copy of the submittal) against what the documents say.
    let mut model: BTreeMap<&str, String> = BTreeMap::new();
    for (k, prop) in f.own.props.iter().take(400) {
        if let Some(field) = field_of(k)
            && prop.has_value()
            && prop.source.kind != SourceKind::Submittal
        {
            model.entry(field).or_insert_with(|| prop.text());
        }
    }
    for (k, v) in f.it.fields.into_iter().flatten().take(400) {
        if let Some(field) = field_of(k) {
            model.entry(field).or_insert_with(|| v.clone());
        }
    }
    for (field, value) in &model {
        if let Some((doc_value, doc)) = want.get(field)
            && agree(field, value, doc_value) == Some(false)
        {
            fails.push(json!({"field": field, "model": cite(value), "expected": cite(doc_value), "from": doc}));
        }
    }
    let measured = json!({
        "type": mark,
        "schedule": rows.iter().map(Row::to_json).collect::<Vec<_>>(),
        "submittal": if product.is_empty() { Value::Null } else { product.to_json() },
        "model": model,
        "conflicts": conflicts,
        "mismatches": fails,
        "unknown": unknown,
    });
    let say = |v: &[Value], k: &str| v.iter().filter_map(|x| x[k].as_str()).map(str::to_string).collect::<Vec<_>>().join("; ");
    if !fails.is_empty() {
        let what = fails
            .iter()
            .map(|x| {
                format!(
                    "{} {} where {} says {}",
                    x["field"].as_str().unwrap_or_default(),
                    x["model"].as_str().unwrap_or_default(),
                    x["from"].as_str().unwrap_or_default(),
                    x["expected"].as_str().unwrap_or_default()
                )
            })
            .collect::<Vec<_>>()
            .join("; ");
        return ("fail", format!("type {mark}'s product in the model differs from the documents: {what}"), measured, documents);
    }
    if !conflicts.is_empty() {
        return ("conflict", format!("the documents disagree on type {mark}: {}", say(&conflicts, "issue")), measured, documents);
    }
    if !unknown.is_empty() {
        return ("unsure", format!("type {mark}: {}", unknown.join("; ")), measured, documents);
    }
    let fields = want.keys().copied().collect::<Vec<_>>().join(", ");
    ("pass", format!("type {mark} is in the fixture schedule and the submittal, which agree ({fields})"), measured, documents)
}

/// E-LUM-COUNT: the plan's symbols per room and type against the model's.
fn counts(inp: &Input, r: &Effective, facts: &[Fact], out: &mut Found) {
    let rooms: Vec<&Area> = {
        let mut seen = BTreeSet::new();
        inp.areas.iter().filter(|a| a.kind != "zone").filter(|a| seen.insert(room_key(a))).collect()
    };
    let mut by_level: HashMap<String, Vec<&Area>> = HashMap::new();
    for a in &rooms {
        by_level.entry(a.level.to_ascii_lowercase()).or_default().push(a);
    }
    // The plan's symbols: the prints', else (a level they don't cover) the job's symbol table,
    // the building spec's device tags as read off the plans.
    struct Sym<'s> {
        level: &'s str,
        at: [f64; 2],
        kind: &'s str,
        mark: Option<&'s str>,
    }
    let mut syms: Vec<Sym> = Vec::new();
    let mut from: HashMap<String, &'static str> = HashMap::new();
    for s in inp.prints.map_or(&[][..], |p| p.symbols.as_slice()).iter().take(MAX_ROWS) {
        from.insert(s.level.to_ascii_lowercase(), "prints");
        syms.push(Sym { level: &s.level, at: s.at, kind: &s.kind, mark: s.mark.as_deref().or(s.tag.as_deref()) });
    }
    for t in inp.sp.tags.iter().take(MAX_ROWS).filter(|t| t.id.starts_with("device:")) {
        let lv = t.level.to_ascii_lowercase();
        if from.get(&lv).is_some_and(|f| *f == "prints") || !by_level.contains_key(&lv) {
            continue;
        }
        from.insert(lv, "the job's symbol table (its device tags read off the plans)");
        let mark = ["type", "type_mark", "mark"].iter().find_map(|k| t.fields.get(*k).map(|f| f.value.as_str()));
        // A tag's point is its drawing point over units per foot; on the level's plan, feet from
        // the level's base.
        let upf = inp.frame.upf();
        let at = inp.frame.to_ft_on(&t.level, cadaddy_geom::Vec2::new(t.at_ft[0] * upf, t.at_ft[1] * upf));
        syms.push(Sym { level: &t.level, at, kind: &t.kind, mark });
    }
    // Each symbol's room among all of its level's (one on a room's wall line is that room's),
    // counted when that room is in scope.
    let all_rooms = super::validate_packet::model_areas(inp.d, inp.frame);
    let mut all_by_level: HashMap<String, Vec<&Area>> = HashMap::new();
    for a in all_rooms.iter().filter(|a| by_level.contains_key(&a.level.to_ascii_lowercase())) {
        all_by_level.entry(a.level.to_ascii_lowercase()).or_default().push(a);
    }
    // The plan's lighting symbols by room and type.
    let mut plan: HashMap<String, BTreeMap<String, u32>> = HashMap::new();
    for s in &syms {
        if !s.at.iter().all(|v| v.is_finite()) {
            continue;
        }
        let res = inp.kinds.resolve(s.kind, s.mark, None);
        let lit = Lit {
            handle: "",
            kind: &res.kind,
            kind_from: res.from,
            category: res.category.as_deref(),
            mark: s.mark,
            name: None,
            level: s.level,
            at: s.at,
            built: false,
            fields: None,
            entity: None,
            room: None,
        };
        let (lum, ctl) = class(inp.rules, &lit);
        if !(lum || ctl) {
            continue;
        }
        let Some(list) = all_by_level.get(&s.level.to_ascii_lowercase()) else { continue };
        let Some(home) = symbol_home(list, s.at) else { continue };
        let Some(a) = rooms.iter().find(|a| a.id == home.id) else { continue };
        let key = type_key(s.mark, &res.kind);
        *plan.entry(room_key(a)).or_default().entry(key).or_default() += 1;
    }
    let mut model: HashMap<String, BTreeMap<String, (u32, Vec<String>)>> = HashMap::new();
    for f in facts {
        let Some(rk) = &f.room else { continue };
        let key = type_key(f.it.mark, f.it.kind);
        let e = model.entry(rk.clone()).or_default().entry(key).or_default();
        e.0 += 1;
        if e.1.len() < MAX_LISTED {
            e.1.push(f.it.handle.to_string());
        }
    }
    for a in rooms {
        let rk = room_key(a);
        let p = plan.get(&rk);
        let m = model.get(&rk);
        if p.is_none() && m.is_none() {
            continue;
        }
        let Some(plan_from) = from.get(&a.level.to_ascii_lowercase()).copied() else {
            out.skip(COUNT, "no plan symbols read for its level");
            continue;
        };
        let keys: BTreeSet<&String> = p.into_iter().flat_map(|x| x.keys()).chain(m.into_iter().flat_map(|x| x.keys())).collect();
        for key in keys {
            let n_plan = p.and_then(|x| x.get(key)).copied().unwrap_or(0);
            let (n_model, handles) = m.and_then(|x| x.get(key)).cloned().unwrap_or_default();
            let verdict = match n_model.cmp(&n_plan) {
                std::cmp::Ordering::Equal => "pass",
                std::cmp::Ordering::Less => "missing",
                std::cmp::Ordering::Greater => "extra",
            };
            out.tally(COUNT, verdict);
            let label = room_label(a);
            let issue = match verdict {
                "pass" => format!("{label}: {n_plan} {key} on the plan, {n_model} in the model"),
                "missing" => format!("{label}: the plan shows {n_plan} {key}, the model has {n_model}"),
                _ => format!("{label}: the model has {n_model} {key}, the plan shows {n_plan}"),
            };
            let id = format!("PS-{}", super::validate_run::hash8(&format!("{COUNT}|{rk}|{key}|{verdict}")));
            let centre = [(a.bbox[0] + a.bbox[2]) / 2.0, (a.bbox[1] + a.bbox[3]) / 2.0];
            let f = json!({
                "id": id,
                "rule": r.rule.id,
                "verdict": verdict,
                "issue": issue,
                "expected": format!("{n_plan} {key}"),
                "found": format!("{n_model}"),
                "measured": {"type": key, "plan": n_plan, "model": n_model, "handles": handles, "plan_from": plan_from},
                "evidence": [],
                "confidence": 1.0,
                "severity": if verdict == "extra" { "low".to_string() } else { r.rule.severity.clone() },
                "area": a.id.split('#').next().unwrap_or(&a.id),
                "level": a.level,
                "at": pt([r2(centre[0]), r2(centre[1])]),
                "source": "prescreen",
            });
            out.findings.push((rank(verdict, &r.rule.severity), f));
        }
    }
}

/// How far outside every room a plan symbol may be drawn and still be its nearest room's, feet:
/// a device's symbol is drawn beside the wall line it sits on, and the build moves the device
/// onto that wall's face (often more than the foot a model element is allowed).
const SYMBOL_REACH_FT: f64 = 2.0;

/// The room a plan symbol is in: the first whose outline holds it, else the nearest within
/// [`SYMBOL_REACH_FT`] of its outline.
fn symbol_home<'a>(rooms: &[&'a Area], p: [f64; 2]) -> Option<&'a Area> {
    if let Some(a) = rooms.iter().find(|a| a.outline.len() >= 3 && point_in(&a.outline, p)) {
        return Some(a);
    }
    rooms
        .iter()
        .filter(|a| inside(super::validate_packet::grow(a.bbox, SYMBOL_REACH_FT), p))
        .filter_map(|a| {
            let d = a
                .outline
                .iter()
                .zip(a.outline.iter().cycle().skip(1))
                .take(10_000)
                .map(|(x, y)| super::validate_packet::seg_dist(p, *x, *y).0)
                .fold(f64::INFINITY, f64::min);
            (d <= SYMBOL_REACH_FT).then_some((*a, d))
        })
        .min_by(|a, b| a.1.total_cmp(&b.1).then_with(|| a.0.id.cmp(&b.0.id)))
        .map(|(a, _)| a)
}

/// Words that make a review item a lighting one, and the ones that make it about controls.
const LIGHT_WORDS: [&str; 14] = [
    "luminaire",
    "luminaires",
    "fixture",
    "fixtures",
    "light",
    "lights",
    "lighting",
    "lamp",
    "driver",
    "dimmer",
    "dimming",
    "switch",
    "sensor",
    "occupancy",
];
const CONTROL_WORDS: [&str; 9] = ["switch", "switches", "dimmer", "dimming", "sensor", "occupancy", "vacancy", "control", "leg"];

/// The job's own `conflict` review items about lighting, as `conflict` findings: the building
/// spec's open review items of a conflict kind in the areas, and any a luminaire or control
/// carries (`conflict`).
fn carried(inp: &Input, facts: &[Fact], out: &mut Found) {
    let about = |text: &str| -> Option<&'static str> {
        let w = tokens(text);
        let has = |xs: &[&str]| w.iter().any(|t| xs.contains(&t.as_str()));
        if !has(&LIGHT_WORDS) {
            return None;
        }
        Some(if has(&CONTROL_WORDS) { CONTROL } else { TYPE })
    };
    let levels: BTreeSet<String> = inp.areas.iter().map(|a| a.level.to_ascii_lowercase()).collect();
    for item in inp.sp.rfis.iter().take(MAX_ROWS) {
        if !matches!(item.status, cadaddy_walk3d::rfi::RfiStatus::Open | cadaddy_walk3d::rfi::RfiStatus::Sent) {
            continue;
        }
        if !item.kind_name().contains("conflict") {
            continue;
        }
        let text = format!("{} {} {}", item.about, item.issue, item.assumed);
        let Some(id) = about(&text) else { continue };
        let Some(r) = rule_on(inp, id) else { continue };
        if !item.level.is_empty() && !levels.contains(&item.level.to_ascii_lowercase()) {
            continue;
        }
        let at = (!item.level.is_empty()).then(|| inp.frame.to_ft_on(&item.level, cadaddy_geom::Vec2::new(item.at[0], item.at[1])));
        let area = at.and_then(|p| {
            inp.areas
                .iter()
                .find(|a| a.level.eq_ignore_ascii_case(&item.level) && inside(a.bbox, p) && (a.outline.len() < 3 || point_in(&a.outline, p)))
        });
        // A single area's screen: only the items in it.
        if inp.areas.len() == 1 && area.is_none() {
            continue;
        }
        out.tally(id, "conflict");
        let key = format!("{}|{}|{}", item.id, item.about, item.issue);
        let f = json!({
            "id": format!("PS-{}", super::validate_run::hash8(&format!("build|{key}"))),
            "rule": r.rule.id,
            "verdict": "conflict",
            "issue": cite(&item.issue),
            "expected": "the documents agree",
            "found": cite(&item.about),
            "measured": {"review_item": item.id, "kind": item.kind_name(), "assumed": cite(&item.assumed)},
            "documents": item.sheets.iter().take(8).map(|s| json!({"document": s})).collect::<Vec<_>>(),
            "evidence": [],
            "confidence": 1.0,
            "severity": r.rule.severity,
            "area": area.map(|a| a.id.clone()),
            "level": item.level,
            "at": at.map(|p| pt([r2(p[0]), r2(p[1])])),
            "source": "build",
        });
        out.findings.push((rank("conflict", &r.rule.severity), f));
    }
    for f in facts {
        let Some(text) = f.own.text(f.it, &["conflict", "conflicts"]) else { continue };
        let id = if f.lum { TYPE } else { CONTROL };
        let Some(r) = rule_on(inp, id) else { continue };
        out.tally(id, "conflict");
        let (rk, mut v) = finding(
            r,
            f.it,
            "conflict",
            cite(&text),
            "the documents agree".into(),
            "the job found its documents disagree".into(),
            json!({"review_item": text}),
        );
        v["source"] = json!("build");
        v["id"] = json!(format!("PS-{}", super::validate_run::hash8(&format!("build|{}|{text}", f.it.handle))));
        out.findings.push((rk, v));
    }
}

// ---------------------------------------------------------------- packets

/// What a packet carries of its area's lighting: each luminaire type present with its fixture
/// schedule rows and submittal product (and its cut-sheet pages), and the lighting-control
/// legend (the legend rows of the controls present). Also each luminaire's and control's facts
/// on its element. Returns the submittal pages (`<file> p.<n>`) to add to the packet, and none
/// when the area has no lighting.
pub(crate) fn annotate(
    d: &Drawing,
    model: &Model,
    kinds: &Kinds,
    prints: Option<&Prints>,
    rules: &[Effective],
    elements: &mut [Value],
) -> Option<(Value, BTreeSet<String>)> {
    struct El {
        i: usize,
        lum: bool,
        mark: Option<String>,
    }
    let mut els: Vec<El> = Vec::new();
    for (i, e) in elements.iter().enumerate() {
        let s = |k: &str| e.get(k).and_then(Value::as_str);
        let kind = s("kind").unwrap_or_default();
        let lit = Lit {
            handle: "",
            kind,
            kind_from: "element",
            category: s("category"),
            mark: s("mark"),
            name: None,
            level: "",
            at: [0.0, 0.0],
            built: false,
            fields: None,
            entity: None,
            room: None,
        };
        let (lum, ctl) = class(rules, &lit);
        if lum || ctl {
            els.push(El { i, lum, mark: s("mark").and_then(mark_key) });
        }
    }
    if els.is_empty() {
        return None;
    }
    let ent =
        |e: &Value| e.get("handle").and_then(Value::as_str).and_then(cadaddy_doc::Handle::parse_hex).and_then(|h| d.model.get(h)).map(|x| x.as_ref());
    let lum_marks: BTreeSet<String> = els.iter().filter(|x| x.lum).filter_map(|x| x.mark.clone()).collect();
    let mut file_of_mark: HashMap<String, String> = HashMap::new();
    for x in els.iter().filter(|x| x.lum) {
        if let (Some(m), Some(sub)) = (&x.mark, elements.get(x.i).and_then(|e| e.get("submittal")).and_then(Value::as_str)) {
            file_of_mark.entry(m.clone()).or_insert_with(|| sub.to_string());
        }
    }
    let docs = Docs::of(d, kinds, prints, &lum_marks, &file_of_mark);
    let mut pages: BTreeSet<String> = BTreeSet::new();
    let mut types = Vec::new();
    for m in &lum_marks {
        let rows: Vec<Value> = docs.schedule.get(m).into_iter().flatten().map(Row::to_json).collect();
        let product = docs.submittal.get(m);
        if let Some(p) = product {
            pages.extend(p.pages.iter().cloned());
        }
        let count = els.iter().filter(|x| x.lum && x.mark.as_ref() == Some(m)).count();
        let unknown: Vec<&str> = [
            (!docs.schedule.contains_key(m)).then_some(if docs.schedule_read { "not in the fixture schedule" } else { "no fixture schedule read" }),
            product.is_none().then_some("no submittal product read"),
        ]
        .into_iter()
        .flatten()
        .collect();
        types.push(json!({
            "type": m,
            "count": count,
            "schedule": rows,
            "submittal": product.map(Product::to_json),
            "unknown": unknown,
        }));
    }
    let mut legend = Vec::new();
    let mut seen = BTreeSet::new();
    for x in els.iter().filter(|x| !x.lum) {
        let Some(m) = &x.mark else { continue };
        if !seen.insert(m.clone()) {
            continue;
        }
        let e = elements.get(x.i);
        let rows: Vec<Value> = kinds.said_of(Some(m), None).into_iter().take(4).map(|s: &Said| json!({"source": s.source, "text": s.text})).collect();
        // What the job read its type as (off the legend), on the element itself.
        let entity = e.and_then(ent);
        let kind = e.and_then(|e| e.get("kind")).and_then(Value::as_str).unwrap_or_default();
        let lit = Lit {
            handle: "",
            kind,
            kind_from: "element",
            category: None,
            mark: Some(m),
            name: None,
            level: "",
            at: [0.0, 0.0],
            built: false,
            fields: None,
            entity,
            room: None,
        };
        let ty = Own::of(d, entity).text(&lit, &["type", "device_type", "legend"]);
        legend.push(json!({"mark": m, "kind": kind, "type": ty, "rows": rows}));
    }
    // Each element's lighting facts.
    for x in &els {
        let Some(e) = elements.get_mut(x.i) else { continue };
        let entity = ent(e);
        let handle = e.get("handle").and_then(Value::as_str).unwrap_or_default().to_string();
        let el = model.elements.iter().find(|m| m.id.eq_ignore_ascii_case(&handle));
        let kind = e.get("kind").and_then(Value::as_str).unwrap_or_default().to_string();
        let mark = e.get("mark").and_then(Value::as_str).map(str::to_string);
        let lit = Lit {
            handle: &handle,
            kind: &kind,
            kind_from: "element",
            category: None,
            mark: mark.as_deref(),
            name: None,
            level: "",
            at: [0.0, 0.0],
            built: false,
            fields: el.map(|m| &m.fields),
            entity,
            room: None,
        };
        let own = Own::of(d, entity);
        let f = Fact { it: &lit, leg: own.leg(&lit), own, lum: x.lum, room: None };
        let v = if x.lum {
            let (p, from) = driver_proto(&docs, &f);
            json!({"type": x.mark, "switch_leg": f.leg, "dimming": p.map(Proto::name), "dimming_from": from})
        } else {
            let mut words = f.own.words(&lit);
            words.extend(kinds.said_of(lit.mark, entity).into_iter().map(|s| s.text.clone()));
            let proto = f
                .own
                .text(&lit, &["dimming", "protocol", "dimmer_type"])
                .as_deref()
                .and_then(protocol)
                .or_else(|| words.iter().find_map(|w| protocol(w).filter(|p| *p != Proto::Dimmable)));
            json!({"switch_leg": f.leg, "dimmer": says_dimmer(&words), "protocol": proto.map(Proto::name), "ways": ways(&words)})
        };
        e["lighting"] = v;
    }
    let summary = json!({
        "about": "the lighting documents for the types present: each luminaire type's fixture schedule rows and submittal product (cut-sheet pages are in submittals/prints), and the legend rows of the controls present (the lighting-control legend)",
        "fixture_schedule_read": docs.schedule_read,
        "submittal_read": docs.submittal_read,
        "types": types,
        "control_legend": legend,
    });
    Some((summary, pages))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn protocols_read_from_words() {
        for (t, p) in [
            ("0-10V", Proto::ZeroTen),
            ("0-10 V DIMMING DRIVER", Proto::ZeroTen),
            ("ELV DIMMER", Proto::Elv),
            ("reverse phase", Proto::Elv),
            ("TRIAC", Proto::Triac),
            ("forward phase / MLV", Proto::Triac),
            ("phase", Proto::Phase),
            ("NON-DIM", Proto::NonDim),
            ("non dimming", Proto::NonDim),
            ("DALI", Proto::Dali),
            ("switch, dimmer", Proto::Dimmable),
        ] {
            assert_eq!(protocol(t), Some(p), "{t}");
        }
        assert_eq!(protocol("switch, 3-way"), None);
        assert_eq!(protocol(""), None);
    }

    #[test]
    fn a_dimmer_dims_only_drivers_of_its_protocol() {
        assert!(dims(Some(Proto::ZeroTen), Some(Proto::ZeroTen)).is_ok());
        assert_eq!(dims(Some(Proto::ZeroTen), Some(Proto::NonDim)).map_err(|e| e.0), Err(true));
        assert_eq!(dims(None, Some(Proto::NonDim)).map_err(|e| e.0), Err(true), "a non-dim driver fails whatever the dimmer");
        assert_eq!(dims(Some(Proto::ZeroTen), Some(Proto::Elv)).map_err(|e| e.0), Err(true));
        assert_eq!(dims(Some(Proto::Triac), Some(Proto::Elv)).map_err(|e| e.0), Err(true));
        for (a, b) in [
            (None, Some(Proto::ZeroTen)),
            (Some(Proto::ZeroTen), None),
            (Some(Proto::Phase), Some(Proto::Elv)),
            (Some(Proto::Dimmable), Some(Proto::ZeroTen)),
        ] {
            assert_eq!(dims(a, b).map_err(|e| e.0), Err(false), "{a:?} {b:?}: not known");
        }
    }

    #[test]
    fn product_fields_agree_or_not() {
        assert_eq!(agree("catalog", "XB-100", "XB100-L40-MVOLT"), Some(true), "a base catalog number and its ordering string");
        assert_eq!(agree("catalog", "XB-100", "ZZ-900"), Some(false));
        assert_eq!(agree("manufacturer", "ACME", "Acme Lighting Co"), Some(true));
        assert_eq!(agree("manufacturer", "ACME", "Other Co"), Some(false));
        assert_eq!(agree("voltage", "120-277V", "MVOLT"), Some(true));
        assert_eq!(agree("voltage", "120V", "277V"), Some(false));
        assert_eq!(agree("cct", "3000K", "30K"), Some(true));
        assert_eq!(agree("cct", "3000K", "4000K"), Some(false));
        assert_eq!(agree("lumens", "1500", "1510 lm"), Some(true));
        assert_eq!(agree("dimming", "0-10V", "NON-DIM"), Some(false));
        assert_eq!(agree("dimming", "0-10V", "phase"), Some(false));
        assert_eq!(agree("dimming", "phase", "ELV"), None);
        assert_eq!(agree("mounting", "RECESSED", "recessed downlight"), Some(true));
        assert_eq!(agree("mounting", "RECESSED", "pendant"), Some(false));
        assert_eq!(agree("wattage", "", "12W"), None);
    }

    #[test]
    fn emergency_and_unswitched_fixtures_need_no_control() {
        let t = |s: &str| vec![("its type".to_string(), s.to_string())];
        assert!(no_control_needed("luminaire", &t("DOWNLIGHT, EMERGENCY BATTERY PACK")).is_some());
        assert!(no_control_needed("luminaire", &t("A-EM")).is_some());
        assert!(no_control_needed("luminaire", &t("NIGHT LIGHT")).is_some());
        assert!(no_control_needed("luminaire", &t("UNSWITCHED")).is_some());
        assert!(no_control_needed("exit_sign", &t("")).is_some());
        assert!(no_control_needed("luminaire", &t("NON-EMERGENCY DOWNLIGHT")).is_none());
        assert!(no_control_needed("luminaire", &t("2X4 TROFFER")).is_none());
    }

    #[test]
    fn three_ways_and_dimmers_by_their_words() {
        let w = |s: &str| vec![s.to_string()];
        assert_eq!(ways(&w("switch, 3-way")), Some(3));
        assert_eq!(ways(&w("THREE WAY SWITCH")), Some(3));
        assert_eq!(ways(&w("three_way_switch")), Some(3));
        assert_eq!(ways(&w("4-WAY")), Some(4));
        assert_eq!(ways(&w("switch, 2 pole")), None);
        assert!(says_dimmer(&w("switch, dimmer")));
        assert!(says_dimmer(&w("DIMMING SWITCH")));
        assert!(!says_dimmer(&w("switch, 3-way")));
    }

    #[test]
    fn schedule_headings_and_names() {
        assert!(schedule_heading("LIGHTING FIXTURE SCHEDULE"));
        assert!(schedule_heading("LUMINAIRE SCHEDULE"));
        assert!(!schedule_heading("AS INDICATED ON THE LUMINAIRE SCHEDULE."), "a note referring to one");
        assert!(!schedule_heading("SEE LUMINAIRE SCHEDULE"));
        assert!(!schedule_heading("PLUMBING FIXTURE SCHEDULE"));
        assert!(lighting_schedule("LIGHT FIXTURE SCHEDULE", None));
        assert!(!lighting_schedule("PANEL SCHEDULE", None));
        assert!(lighting_schedule("FIXTURE SCHEDULE", Some("luminaire")));
        assert!(!lighting_schedule("FIXTURE SCHEDULE", Some("plumbing_fixture")));
        assert!(!lighting_schedule("FIXTURE SCHEDULE", None), "a bare fixture schedule may be plumbing's");
        assert!(!schedule_heading("FIXTURE SCHEDULE"));
        let row = |k: &str, v: &str| BTreeMap::from([(k.to_string(), v.to_string())]);
        assert!(lighting_row(&row("description", "6\" LED DOWNLIGHT, 1500 LM")));
        assert!(!lighting_row(&row("description", "LAVATORY, WALL HUNG")));
        assert!(!lighting_row(&row("description", "X Y W")));
        assert_eq!(page_in("p.32 (ordering row)"), Some(32));
        assert_eq!(page_in("Lighting.pdf page 7"), Some(7));
        assert_eq!(file_of("Lighting Package p.32"), "Lighting Package");
    }
}
