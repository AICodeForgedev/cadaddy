//! Lighting from its documents (Q6144): each luminaire's product, its controls, and the documents
//! checked against each other.
//!
//! - **Product**: each field of a luminaire (maker, catalog, lamp/LED, CCT, lumens, voltage,
//!   wattage, dimming, mounting type, finish) comes from the approved submittal's cut sheet for its
//!   type, else the light fixture schedule, else the specification's luminaire section, each with
//!   its source ([`resolve`]). Where two of them both speak and disagree it is a conflict citing
//!   both, never a silent pick.
//! - **Controls** ([`link_controls`]): a switch's leg letter links it to the luminaires of that
//!   letter near it (`controls` on the switch, `controlled_by` on the luminaire), a 3-way switch
//!   to its mate, a sensor to its room's luminaires; emergency and exit fixtures are unswitched by
//!   design; a dimmer over a driver that doesn't dim (or dims another way) is a conflict.
//! - **Conflicts** ([`Conflict`]): every disagreement between the plans, the schedule, the
//!   submittal, the specification and the energy code, each a review item of kind `conflict` with
//!   both sides cited (an RFI of kind `conflict` in the building spec).

use std::collections::{BTreeMap, BTreeSet};

use cadaddy_doc::props::SourceKind;
use cadaddy_walk3d::FixtureType;

/// The product fields a luminaire carries, by property key, with their plain names.
pub(crate) const FIELDS: [(&str, &str); 10] = [
    ("manufacturer", "maker"),
    ("catalog_number", "catalog number"),
    ("lamp", "lamp/LED source"),
    ("cct", "CCT"),
    ("lumens", "lumens"),
    ("voltage", "voltage"),
    ("wattage", "wattage"),
    ("dimming", "driver and dimming"),
    ("mounting_type", "mounting"),
    ("finish", "finish"),
];

/// Conflicts listed per topic, at most (the rest counted).
pub(crate) const MAX_PER_TOPIC: usize = 400;
/// Elements one conflict names, at most.
const MAX_ELEMENTS: usize = cadaddy_walk3d::rfi::MAX_RFI_ELEMENTS;

/// What one document says of a field.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Said {
    pub value: String,
    pub kind: SourceKind,
    /// The document and where in it (`E-2 LIGHT FIXTURE SCHEDULE type A`, `Lighting Package p.3`).
    pub doc: String,
}

impl Said {
    fn new(value: impl Into<String>, kind: SourceKind, doc: &str) -> Said {
        Said { value: value.into(), kind, doc: doc.to_string() }
    }
}

/// One document's fields.
pub(crate) type Sayings = BTreeMap<&'static str, Said>;

// ─── Values in a catalog number or a line of text ────────────────────────────────────────────

/// Words of a catalog number or a line, upper case.
fn tokens(text: &str) -> Vec<String> {
    text.to_ascii_uppercase()
        .split(|c: char| !c.is_ascii_alphanumeric() && c != '.')
        .map(|t| t.trim_matches('.'))
        .filter(|t| !t.is_empty())
        .map(str::to_string)
        .collect()
}

/// Colour temperatures luminaires are made in, kelvin / 100.
const CCT_CODES: [u32; 14] = [18, 20, 22, 24, 25, 27, 30, 35, 40, 45, 50, 57, 60, 65];

/// The colour temperature a catalog number or a line gives, kelvin: `3000K`, `40K`, the LED
/// colour code (`830` = CRI 80s at 3000 K, `LP840`), `3500 K`.
pub(crate) fn cct_of(text: &str) -> Option<u32> {
    let ts = tokens(text);
    for (i, t) in ts.iter().enumerate() {
        let digits: String = t.chars().take_while(char::is_ascii_digit).collect();
        let rest = t.get(digits.len()..).unwrap_or("");
        // `3000K`, `40K`; `3500` then `K`.
        let k = rest == "K" || (rest.is_empty() && ts.get(i + 1).is_some_and(|n| n == "K" || n == "KELVIN"));
        if k {
            if let Ok(n) = digits.parse::<u32>() {
                if digits.len() == 4 && (1800..=6500).contains(&n) && n % 100 == 0 {
                    return Some(n);
                }
                if digits.len() == 2 && CCT_CODES.contains(&n) {
                    return Some(n * 100);
                }
            }
            continue;
        }
        // The LED colour code: three digits, the first the CRI's tens (7, 8, 9), the last two the
        // CCT's hundreds; alone or after a short option prefix (`LP830`).
        let lead: String = t.chars().take_while(char::is_ascii_alphabetic).collect();
        let code = t.get(lead.len()..).unwrap_or("");
        if lead.len() <= 3 && code.len() == 3 && code.chars().all(|c| c.is_ascii_digit()) {
            let first = code.chars().next().and_then(|c| c.to_digit(10)).unwrap_or(0);
            let cct = code.get(1..).and_then(|c| c.parse::<u32>().ok()).unwrap_or(0);
            if (7..=9).contains(&first) && CCT_CODES.contains(&cct) {
                return Some(cct * 100);
            }
        }
    }
    None
}

/// The lumens a catalog number or line gives: `4000LM`, `4000 LM`, `4000 LUMENS`.
pub(crate) fn lumens_of(text: &str) -> Option<u32> {
    let ts = tokens(text);
    for (i, t) in ts.iter().enumerate() {
        let digits: String = t.chars().take_while(char::is_ascii_digit).collect();
        if digits.len() < 3 || digits.len() > 6 {
            continue;
        }
        let rest = t.get(digits.len()..).unwrap_or("");
        let lm = rest == "LM" || (rest.is_empty() && ts.get(i + 1).is_some_and(|n| n == "LM" || n == "LUMENS" || n == "LUMEN"));
        if lm {
            return digits.parse().ok();
        }
    }
    None
}

/// The voltages a catalog number or line allows, (lowest, highest): `MVOLT`/`UNV` 120-277,
/// `120-277V`, `120V`, `277`.
pub(crate) fn volts_of(text: &str) -> Option<(u32, u32)> {
    let up = text.to_ascii_uppercase();
    let ts = tokens(&up);
    if ts.iter().any(|t| t == "MVOLT" || t == "UNV" || t == "UNIV" || t == "MV") {
        return Some((120, 277));
    }
    let nominal = |n: u32| [12, 24, 120, 208, 240, 277, 347, 480].contains(&n);
    let num = |t: &str| t.strip_suffix("VAC").or_else(|| t.strip_suffix('V')).unwrap_or(t).parse::<u32>().ok().filter(|n| nominal(*n));
    // `120-277V` and `120/277`.
    for w in up.split_whitespace() {
        let w = w.trim_matches(|c: char| !c.is_ascii_alphanumeric());
        if let Some((a, b)) = w.split_once(['-', '/']).and_then(|(a, b)| Some((num(a)?, num(b)?))) {
            return Some((a.min(b), a.max(b)));
        }
    }
    let found: Vec<u32> = ts.iter().filter_map(|t| num(t)).collect();
    let lo = found.iter().min()?;
    let hi = found.iter().max()?;
    Some((*lo, *hi))
}

/// The wattage (or VA) a line gives: `13VA`, `LED 39 VA`, `24W`.
pub(crate) fn watts_of(text: &str) -> Option<f64> {
    let ts = tokens(text);
    for (i, t) in ts.iter().enumerate() {
        let num: String = t.chars().take_while(|c| c.is_ascii_digit() || *c == '.').collect();
        if num.is_empty() {
            continue;
        }
        let rest = t.get(num.len()..).unwrap_or("");
        let unit = if rest.is_empty() { ts.get(i + 1).map_or("", String::as_str) } else { rest };
        if matches!(unit, "W" | "VA" | "WATT" | "WATTS") {
            return num.trim_end_matches('.').parse::<f64>().ok().filter(|w| w.is_finite() && *w > 0.0 && *w < 5_000.0);
        }
    }
    None
}

/// How a driver dims by the words of a line: `0-10V`, `phase` (ELV, MLV, TRIAC, LE/TE,
/// forward/reverse phase), `DALI`, `non-dim`.
pub(crate) fn dimming_of(text: &str) -> Option<&'static str> {
    let up = text.to_ascii_uppercase();
    let has = |w: &str| up.contains(w);
    if has("NON-DIM") || has("NON DIM") || has("NONDIM") || has("NOT DIMMABLE") {
        Some("non-dim")
    } else if has("0-10") || has("0 - 10") {
        Some("0-10V")
    } else if has("DALI") {
        Some("DALI")
    } else if has("ELV") || has("MLV") || has("TRIAC") || has("PHASE") || has("LE/TE") || has("FORWARD") || has("REVERSE") {
        Some("phase")
    } else {
        None
    }
}

/// How a luminaire is mounted by the words of a line: recessed, pendant, wall, surface,
/// ceiling, pole; an `AFF` height alone is on a wall.
pub(crate) fn mounting_of(text: &str) -> Option<&'static str> {
    let up = text.to_ascii_uppercase();
    let has = |w: &str| up.contains(w);
    if has("VARIES") || has("TBD") {
        None
    } else if has("RECESS") || has("FLUSH") || has("DOWNLIGHT") && !has("SURFACE") {
        Some("recessed")
    } else if has("PENDANT") || has("SUSPEND") || has("STEM") || has("CHAIN") || has("CABLE HUNG") {
        Some("pendant")
    } else if has("POLE") {
        Some("pole")
    } else if has("WALL") || has("SCONCE") || has("WALL PACK") || has("BRACKET") {
        Some("wall")
    } else if has("CEILING") {
        Some("ceiling")
    } else if has("SURFACE") {
        Some("surface")
    } else if super::propfill::mount_height_in(&up).is_some() {
        Some("wall")
    } else {
        None
    }
}

/// The finish a line names (its colour or plating).
pub(crate) fn finish_of(text: &str) -> Option<&'static str> {
    const FINISHES: [(&str, &str); 14] = [
        ("DARK BRONZE", "dark bronze"),
        ("OIL RUBBED BRONZE", "oil rubbed bronze"),
        ("BRONZE", "bronze"),
        ("BRUSHED NICKEL", "brushed nickel"),
        ("SATIN NICKEL", "satin nickel"),
        ("NICKEL", "nickel"),
        ("CHROME", "chrome"),
        ("BRASS", "brass"),
        ("MATTE BLACK", "black"),
        ("BLACK", "black"),
        ("WHITE", "white"),
        ("SILVER", "silver"),
        ("GRAPHITE", "graphite"),
        ("ALUMINUM", "aluminum"),
    ];
    let up = text.to_ascii_uppercase();
    FINISHES.iter().find(|(w, _)| up.contains(w)).map(|(_, f)| *f)
}

/// A value that says the document leaves it to another (`REFER TO SITE LIGHTING PACKAGE`,
/// `BY OTHERS`, `TBD`): no value.
fn deferred(v: &str) -> bool {
    let up = v.trim().to_ascii_uppercase();
    up.is_empty() || up.starts_with("REFER TO") || up.starts_with("SEE ") || up == "TBD" || up.contains("BY OTHERS") || up == "-" || up == "N/A"
}

// ─── Each document's fields ──────────────────────────────────────────────────────────────────

/// What a fixture schedule's row says of each field.
pub(crate) fn schedule_says(row: &FixtureType) -> Sayings {
    let doc = format!("{} type {}", row.source, row.tag);
    let mut out = Sayings::new();
    let all = format!("{} {} {}", row.model, row.lamp, row.description);
    let mut put = |k: &'static str, v: String| {
        if !deferred(&v) {
            out.insert(k, Said::new(v, SourceKind::Schedule, &doc));
        }
    };
    put("manufacturer", row.manufacturer.trim().to_string());
    put("catalog_number", row.model.trim().to_string());
    put("lamp", row.lamp.trim().to_string());
    put("voltage", row.voltage.trim().to_string());
    if let Some(k) = cct_of(&format!("{} {}", row.model, row.lamp)) {
        put("cct", format!("{k}K"));
    }
    if let Some(l) = lumens_of(&format!("{} {}", row.model, row.lamp)) {
        put("lumens", l.to_string());
    }
    if let Some(w) = row.va.or_else(|| watts_of(&row.lamp)) {
        put("wattage", format!("{} VA", trim_num(w)));
    }
    if let Some(dm) = dimming_of(&all) {
        put("dimming", dm.to_string());
    }
    put("mounting_type", row.installation.trim().to_string());
    if let Some(f) = finish_of(&row.description) {
        put("finish", f.to_string());
    }
    out
}

fn trim_num(v: f64) -> String {
    let r = (v * 10.0).round() / 10.0;
    if r.fract() == 0.0 { format!("{r:.0}") } else { format!("{r}") }
}

/// What the specification's luminaire section requires of every luminaire it covers: its CCT,
/// its least CRI, its voltage, its driver's dimming (each with the section and page).
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct SpecLighting {
    /// `26 51 19 LED INTERIOR LIGHTING`.
    pub section: String,
    pub said: Vec<(&'static str, String, String)>,
}

impl SpecLighting {
    /// Its fields as one document's sayings (the section and where in it as the document).
    pub(crate) fn says(&self) -> Sayings {
        let mut out = Sayings::new();
        for (k, v, at) in &self.said {
            let doc = if at.is_empty() { self.section.clone() } else { format!("{} ({at})", self.section) };
            out.entry(k).or_insert_with(|| Said::new(v.clone(), SourceKind::Spec, &doc));
        }
        out
    }
}

/// The luminaire section's requirements from its text (`--- page N ---` between pages): `CCT of
/// 3500 K`, `CRI of minimum 80`, `Nominal Operating Voltage: 120 V ac`. Only PART 2 (products)
/// speaks; a value "as noted on drawings" is none.
pub(crate) fn read_spec_lighting(text: &str, file: &str) -> SpecLighting {
    let mut out = SpecLighting { section: file.to_string(), said: Vec::new() };
    let mut in_products = false;
    let mut page = String::new();
    for raw in text.lines().take(20_000) {
        let line = raw.trim();
        if line.is_empty() {
            continue;
        }
        if let Some(p) = line.strip_prefix("--- page ") {
            page = format!("page {}", p.split_whitespace().next().unwrap_or(""));
            continue;
        }
        let up = line.to_ascii_uppercase();
        if let Some(rest) = up.strip_prefix("SECTION ") {
            let (num, title) = rest.split_once(" - ").unwrap_or((rest, ""));
            let digits: String = num.chars().filter(char::is_ascii_digit).collect();
            if digits.len() >= 6 {
                out.section = format!("{} {} {} {}", &digits[..2], &digits[2..4], &digits[4..6], title.trim()).trim().to_string();
            }
            continue;
        }
        if up.starts_with("PART 2") {
            in_products = true;
            continue;
        }
        if up.starts_with("PART 3") {
            in_products = false;
            continue;
        }
        if !in_products || up.contains("AS NOTED") || up.contains("AS INDICATED") || up.contains("AS SCHEDULED") {
            continue;
        }
        let has = |said: &[(&'static str, String, String)], k: &str| said.iter().any(|(f, _, _)| *f == k);
        if (up.contains("CCT") || up.contains("COLOR TEMPERATURE"))
            && let Some(k) = cct_of(&up).filter(|_| !has(&out.said, "cct"))
        {
            out.said.push(("cct", format!("{k}K"), page.clone()));
        }
        if up.contains("CRI") || up.contains("COLOR RENDERING") {
            let after = up.split("CRI").nth(1).or_else(|| up.split("COLOR RENDERING").nth(1)).unwrap_or("");
            if let Some(n) = after
                .split(|c: char| !c.is_ascii_digit())
                .find(|w| w.len() == 2)
                .and_then(|w| w.parse::<u32>().ok())
                .filter(|_| !has(&out.said, "cri"))
            {
                out.said.push(("cri", format!("{n} minimum"), page.clone()));
            }
        }
        if up.contains("VOLTAGE")
            && !has(&out.said, "voltage")
            && let Some((lo, hi)) = volts_of(&up)
        {
            out.said.push(("voltage", if lo == hi { format!("{lo}") } else { format!("{lo}-{hi}") }, page.clone()));
        }
    }
    out
}

/// What a luminaire's submittal says (its element's properties `bim.submittals` set), by field:
/// `(key, value, document)`.
pub(crate) fn submittal_says(props: &[(&'static str, String, String)]) -> Sayings {
    let mut out = Sayings::new();
    for (k, v, doc) in props {
        if deferred(v) {
            continue;
        }
        out.entry(k).or_insert_with(|| Said::new(v.clone(), SourceKind::Submittal, doc));
    }
    // A catalog number holds what its cut sheet orders: its CCT, lumens and voltage.
    if let Some(cat) = out.get("catalog_number").cloned() {
        let doc = format!("{} (its catalog number {})", cat.doc, cat.value);
        if let Some(k) = cct_of(&cat.value) {
            out.entry("cct").or_insert_with(|| Said::new(format!("{k}K"), SourceKind::Submittal, &doc));
        }
        if let Some(l) = lumens_of(&cat.value) {
            out.entry("lumens").or_insert_with(|| Said::new(l.to_string(), SourceKind::Submittal, &doc));
        }
        if let Some((lo, hi)) = volts_of(&cat.value) {
            let v = if lo == hi { format!("{lo}") } else { format!("{lo}-{hi}") };
            out.entry("voltage").or_insert_with(|| Said::new(v, SourceKind::Submittal, &doc));
        }
    }
    out
}

// ─── Comparing ───────────────────────────────────────────────────────────────────────────────

/// Whether two documents' values of a field agree; `None` when they can't be compared (one
/// says nothing comparable).
pub(crate) fn agree(field: &str, a: &str, b: &str) -> Option<bool> {
    match field {
        "manufacturer" => Some(super::lightsched::same_maker(a, b)),
        "catalog_number" => Some(super::lightsched::same_product(a, b)),
        "cct" => Some(cct_of(a)?.abs_diff(cct_of(b)?) <= 50),
        "lumens" => {
            let (x, y) = (
                f64::from(lumens_of(&format!("{a} LM")).or_else(|| a.trim().parse().ok())?),
                f64::from(lumens_of(&format!("{b} LM")).or_else(|| b.trim().parse().ok())?),
            );
            Some((x - y).abs() <= 0.1 * x.max(y))
        }
        "voltage" => {
            let (x, y) = (volts_of(a)?, volts_of(b)?);
            // A driver taking 120-277 V runs on 120 V: they agree when one range holds the other.
            Some((x.0 <= y.0 && y.1 <= x.1) || (y.0 <= x.0 && x.1 <= y.1))
        }
        "wattage" => {
            let (x, y) = (watts_of(a)?, watts_of(b)?);
            Some((x - y).abs() <= 0.2 * x.max(y) + 1.0)
        }
        "dimming" => {
            let (x, y) = (dimming_of(a).or_else(|| norm_dim(a))?, dimming_of(b).or_else(|| norm_dim(b))?);
            Some(x == y)
        }
        "mounting_type" => {
            let (x, y) = (mounting_of(a)?, mounting_of(b)?);
            // A surface luminaire is on a ceiling or a wall; a ceiling one is surface or recessed
            // unless it says.
            let fits = |p: &str, q: &str| {
                p == q || matches!((p, q), ("surface", "ceiling") | ("ceiling", "surface") | ("surface", "wall") | ("wall", "surface"))
            };
            Some(fits(x, y))
        }
        "finish" => {
            let (x, y) = (finish_of(a)?, finish_of(b)?);
            Some(x == y || x.ends_with(y) || y.ends_with(x))
        }
        _ => None,
    }
}

/// A stored dimming value's class (`bim.submittals` writes `0-10V`, `phase`, `DALI`, `non-dim`).
fn norm_dim(v: &str) -> Option<&'static str> {
    match v.trim().to_ascii_lowercase().as_str() {
        "0-10v" => Some("0-10V"),
        "phase" => Some("phase"),
        "dali" => Some("DALI"),
        "non-dim" | "none" => Some("non-dim"),
        _ => None,
    }
}

/// One field two or more documents disagree on: each side's value and document.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct FieldConflict {
    pub field: &'static str,
    pub sides: Vec<Said>,
}

/// Each field's value from the documents in their order (the submittal's, else the schedule's,
/// else the specification's), and the fields they disagree on (every side that disagrees with
/// another, in that order). A field one document alone gives is that document's.
pub(crate) fn resolve(docs: &[&Sayings]) -> (Sayings, Vec<FieldConflict>) {
    let mut chosen = Sayings::new();
    let mut conflicts = Vec::new();
    for (k, _) in FIELDS.iter().chain([("cri", "CRI")].iter()) {
        let said: Vec<&Said> = docs.iter().filter_map(|d| d.get(k)).collect();
        if let Some(first) = said.first() {
            chosen.insert(*k, (*first).clone());
        }
        let mut sides: Vec<Said> = Vec::new();
        for (i, a) in said.iter().enumerate() {
            for b in said.iter().skip(i + 1) {
                if agree(k, &a.value, &b.value) == Some(false) {
                    for x in [a, b] {
                        if !sides.iter().any(|s| s.doc == x.doc) {
                            sides.push((*x).clone());
                        }
                    }
                }
            }
        }
        if sides.len() >= 2 {
            conflicts.push(FieldConflict { field: k, sides });
        }
    }
    (chosen, conflicts)
}

/// Whether the specification's luminaire section speaks for a fixture type: an interior one (its
/// description doesn't say exterior, site, exit or emergency: other sections cover those).
pub(crate) fn interior(row: &FixtureType) -> bool {
    let up = row.description.to_ascii_uppercase();
    let unit = up.contains("EMERGENCY") && !backup(&up);
    !unit && !["EXTERIOR", "SITE", "WALL PACK", "EXIT", "PARKING", "POLE"].iter().any(|w| up.contains(w))
}

/// Whether a description is a luminaire with an emergency battery in it (`PROVIDE WITH EMERGENCY
/// BATTERY BACK-UP`): switched like the others, its battery's charging feed unswitched.
fn backup(up: &str) -> bool {
    up.contains("BACK-UP") || up.contains("BACKUP") || up.contains("BACK UP") || up.contains("BATTERY")
}

/// Whether a fixture type is emergency or exit lighting by its schedule row (description, lamp,
/// notes): it stays on (unswitched, ahead of switching) by design.
pub(crate) fn emergency_type(row: &FixtureType) -> Option<Emergency> {
    let up = format!("{} {}", row.description, row.lamp).to_ascii_uppercase();
    let words = ["EXIT", "EMERGENCY", "BATTERY", "EGRESS", "NIGHT LIGHT", "NIGHTLIGHT"];
    let w = words.iter().find(|w| up.contains(*w))?;
    let why = format!("{} type {}: {}", row.source, row.tag, row.description.trim());
    // A luminaire with a battery in it is switched like the others; an exit sign, an emergency
    // unit or a night light stays on.
    let switched = backup(&up) && !up.contains("EXIT") && *w != "NIGHT LIGHT" && *w != "NIGHTLIGHT";
    Some(Emergency { why, switched })
}

/// A fixture type that lights the way out: why (its schedule row), and whether it is switched
/// like other luminaires (a battery inside one) or stays on (an exit sign, an emergency unit).
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Emergency {
    pub why: String,
    pub switched: bool,
}

// ─── Conflicts ───────────────────────────────────────────────────────────────────────────────

/// One disagreement between the documents, for the review queue.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Conflict {
    /// Which check found it (`catalog number`, `switch leg with no fixture`).
    pub topic: String,
    /// What it is about (`lighting type F1`, `room 101 leg a`).
    pub about: String,
    pub issue: String,
    /// Each document's word: (document, what it says).
    pub sides: Vec<(String, String)>,
    pub fix: String,
    pub level: String,
    /// Where on its level's plan (drawing units), when it is at one place.
    pub at: Option<[f64; 2]>,
    pub elements: Vec<String>,
    pub severity: Option<cadaddy_walk3d::rfi::Severity>,
}

impl Conflict {
    pub(crate) fn new(topic: &str, about: impl Into<String>, issue: impl Into<String>, fix: impl Into<String>) -> Conflict {
        Conflict { topic: topic.to_string(), about: about.into(), issue: issue.into(), fix: fix.into(), ..Conflict::default() }
    }

    pub(crate) fn side(mut self, document: impl Into<String>, says: impl Into<String>) -> Conflict {
        let (d, s) = (document.into(), says.into());
        if !self.sides.iter().any(|(x, y)| *x == d && *y == s) {
            self.sides.push((d, s));
        }
        self
    }

    pub(crate) fn with_elements(mut self, hs: impl IntoIterator<Item = String>) -> Conflict {
        self.elements.extend(hs);
        self.elements.truncate(MAX_ELEMENTS);
        self
    }

    /// Its RFI id: the same disagreement keeps its id from one build to the next.
    pub(crate) fn id(&self) -> String {
        let mut h: u64 = 0xcbf2_9ce4_8422_2325;
        for b in format!("{}|{}|{}|{}", self.topic, self.about, self.level, self.issue).bytes() {
            h = (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3);
        }
        format!("RFI-C{:08x}", h as u32)
    }

    /// As the building spec's RFI of kind `conflict`.
    pub(crate) fn to_rfi(&self, trade: &str) -> cadaddy_walk3d::Rfi {
        let kind = cadaddy_walk3d::RfiKind::Conflict;
        cadaddy_walk3d::Rfi {
            id: self.id(),
            kind: Some(kind),
            severity: self.severity.unwrap_or_else(|| kind.severity()),
            level: self.level.clone(),
            at: self.at.unwrap_or([0.0, 0.0]),
            sheets: self.sides.iter().map(|(d, _)| d.clone()).take(8).collect(),
            about: self.about.clone(),
            issue: self.issue.clone(),
            assumed: "The model keeps the first document's value (the submittal, then the schedule, then the specification) until answered.".into(),
            proposed: (!self.fix.is_empty()).then(|| self.fix.clone()),
            topic: format!("{trade}: {}", self.topic),
            sides: self.sides.iter().map(|(d, s)| cadaddy_walk3d::RfiSide { document: d.clone(), says: s.clone() }).collect(),
            elements: self.elements.clone(),
            ..cadaddy_walk3d::Rfi::default()
        }
    }
}

/// The conflicts by topic, each topic's count.
pub(crate) fn by_topic(cs: &[Conflict]) -> BTreeMap<String, usize> {
    let mut out = BTreeMap::new();
    for c in cs {
        *out.entry(c.topic.clone()).or_default() += 1;
    }
    out
}

/// The product conflicts of a fixture type (fields its documents disagree on), one per field.
pub(crate) fn field_conflicts(ty: &str, fcs: &[FieldConflict], elements: &[String]) -> Vec<Conflict> {
    fcs.iter()
        .map(|fc| {
            let label = FIELDS.iter().find(|(k, _)| *k == fc.field).map_or(fc.field, |(_, l)| *l);
            let said: Vec<String> = fc.sides.iter().map(|s| format!("{} ({})", s.value, s.doc)).collect();
            let mut c = Conflict::new(
                label,
                format!("lighting type {ty}"),
                format!("type {ty} {label}: the documents disagree: {}", said.join(" vs ")),
                format!("Confirm type {ty}'s {label} (resubmit, or revise the schedule or the specification)"),
            )
            .with_elements(elements.iter().cloned());
            for s in &fc.sides {
                c = c.side(s.doc.clone(), s.value.clone());
            }
            c
        })
        .collect()
}

/// The schedule, the submittal and the plans' counts of each type against each other: a plan's
/// type the schedule lacks; a scheduled type no submittal covers; a submitted type the schedule
/// lacks; one submitted product for two types; a scheduled (or submitted) type no plan shows; a
/// plan count other than the schedule's or submittal's quantity.
pub(crate) struct TypeCheck<'a> {
    pub schedule: &'a [FixtureType],
    /// The submittal's products by type: (maker, catalog number, document).
    pub submitted: &'a BTreeMap<String, (String, String, String)>,
    /// Luminaires built of each type, and their handles.
    pub on_plans: &'a BTreeMap<String, Vec<String>>,
    /// The submittal's quantity of each type, where it states one: (quantity, document).
    pub submitted_qty: &'a BTreeMap<String, (u32, String)>,
    /// The documents' names: the submittal and the plans.
    pub submittal_name: &'a str,
}

pub(crate) fn type_conflicts(t: &TypeCheck) -> Vec<Conflict> {
    let mut out = Vec::new();
    let sched = |ty: &str| t.schedule.iter().find(|r| r.tag == ty);
    let row_doc = |r: &FixtureType| format!("{} type {}", r.source, r.tag);
    for (ty, hs) in t.on_plans {
        if sched(ty).is_none() && !t.schedule.is_empty() {
            let src = t.schedule.first().map_or_else(String::new, |r| r.source.clone());
            out.push(
                Conflict::new(
                    "plan type with no schedule row",
                    format!("lighting type {ty}"),
                    format!("{} luminaires of type {ty} on the plans; the light fixture schedule has no type {ty}", hs.len()),
                    "Add the type to the schedule (basis of design, lamp, voltage, mounting) or correct the plans' tags",
                )
                .side("the plans", format!("{} luminaires tagged {ty}", hs.len()))
                .side(src, format!("no type {ty}"))
                .with_elements(hs.iter().cloned()),
            );
        }
    }
    for r in t.schedule {
        let hs = t.on_plans.get(&r.tag).cloned().unwrap_or_default();
        if !t.submitted.is_empty() && !t.submitted.contains_key(&r.tag) {
            out.push(
                Conflict::new(
                    "scheduled type with no submittal",
                    format!("lighting type {}", r.tag),
                    format!("type {} is scheduled ({} {}); no submittal product covers it", r.tag, r.manufacturer, r.model),
                    format!("Submit type {}'s product data, or confirm it is by others", r.tag),
                )
                .side(row_doc(r), format!("{} {}", r.manufacturer, r.model).trim().to_string())
                .side(t.submittal_name, format!("no type {}", r.tag))
                .with_elements(hs.iter().cloned()),
            );
        }
        let plan_n = hs.len() as u32;
        if plan_n == 0 {
            let mut c = Conflict::new(
                "scheduled type on no plan",
                format!("lighting type {}", r.tag),
                format!("type {} is scheduled{}; no plan shows one", r.tag, if t.submitted.contains_key(&r.tag) { " and submitted" } else { "" }),
                format!("Show type {}'s luminaires on the plans, or take it off the schedule", r.tag),
            )
            .side(row_doc(r), r.description.clone())
            .side("the plans", "none");
            if let Some((m, cat, doc)) = t.submitted.get(&r.tag) {
                c = c.side(doc.clone(), product_words(m, cat));
            }
            c.severity = Some(cadaddy_walk3d::rfi::Severity::Low);
            out.push(c);
        }
        if let Some(q) = r.quantity.filter(|q| *q != plan_n && plan_n > 0) {
            out.push(
                Conflict::new(
                    "plan count vs quantity",
                    format!("lighting type {}", r.tag),
                    format!("type {}: the plans show {plan_n}; the schedule's quantity is {q}", r.tag),
                    "Confirm the count (the plans, or the schedule's quantity)",
                )
                .side("the plans", plan_n.to_string())
                .side(row_doc(r), q.to_string())
                .with_elements(hs.iter().cloned()),
            );
        }
    }
    for (ty, (q, doc)) in t.submitted_qty {
        let plan_n = t.on_plans.get(ty).map_or(0, Vec::len) as u32;
        if *q != plan_n && plan_n > 0 {
            out.push(
                Conflict::new(
                    "plan count vs quantity",
                    format!("lighting type {ty}"),
                    format!("type {ty}: the plans show {plan_n}; the submittal's quantity is {q}"),
                    "Confirm the count with the supplier's order",
                )
                .side("the plans", plan_n.to_string())
                .side(doc.clone(), q.to_string())
                .with_elements(t.on_plans.get(ty).into_iter().flatten().cloned()),
            );
        }
    }
    for (ty, (m, cat, doc)) in t.submitted {
        if sched(ty).is_none() && !t.schedule.is_empty() {
            let src = t.schedule.first().map_or_else(String::new, |r| r.source.clone());
            let shown = t.on_plans.get(ty).map_or(0, Vec::len);
            out.push(
                Conflict::new(
                    "submitted type with no schedule row",
                    format!("lighting type {ty}"),
                    format!(
                        "the submittal has a product for type {ty} ({m} {cat}); the light fixture schedule has no type {ty}{}",
                        if shown == 0 { " and no plan shows one" } else { "" }
                    ),
                    format!("Confirm type {ty} belongs to this project (add it to the schedule) or take it out of the submittal"),
                )
                .side(doc.clone(), product_words(m, cat))
                .side(src, format!("no type {ty}")),
            );
            if shown == 0
                && let Some(c) = out.last_mut()
            {
                c.severity = Some(cadaddy_walk3d::rfi::Severity::Low);
            }
        }
    }
    // One submitted catalog number for two types: one of them was submitted wrong (or the types
    // are one).
    let mut by_cat: BTreeMap<String, Vec<&String>> = BTreeMap::new();
    for (ty, (_, cat, _)) in t.submitted {
        let key: String = cat.chars().filter(char::is_ascii_alphanumeric).collect::<String>().to_ascii_uppercase();
        if key.len() >= 4 {
            by_cat.entry(key).or_default().push(ty);
        }
    }
    for (_, tys) in by_cat.into_iter().filter(|(_, v)| v.len() >= 2) {
        let names: Vec<&str> = tys.iter().map(|t| t.as_str()).collect();
        let mut c = Conflict::new(
            "one product for two types",
            format!("lighting types {}", names.join(", ")),
            format!("the submittal gives one product for types {}", names.join(" and ")),
            "Confirm each type's product: one of them is the wrong cut sheet, or the types are one",
        );
        for ty in &tys {
            if let Some((m, cat, doc)) = t.submitted.get(*ty) {
                c = c.side(format!("{doc} type {ty}"), product_words(m, cat));
            }
            if let Some(r) = sched(ty) {
                c = c.side(row_doc(r), format!("{} {} ({})", r.manufacturer, r.model, r.description).trim().to_string());
            }
        }
        out.push(c);
    }
    out
}

/// A submitted product in words: its maker and catalog number, or that its cut sheet names
/// neither (a shop drawing whose title block wasn't read).
fn product_words(maker: &str, catalog: &str) -> String {
    let w = format!("{maker} {catalog}").trim().to_string();
    if w.is_empty() { "a cut sheet (no maker or catalog number read)".into() } else { w }
}

/// Distinct values, sorted.
pub(crate) fn distinct<I: IntoIterator<Item = String>>(v: I) -> Vec<String> {
    v.into_iter().collect::<BTreeSet<_>>().into_iter().collect()
}

// ─── Controls ────────────────────────────────────────────────────────────────────────────────

/// A luminaire, a control (switch, dimmer, sensor) or another load a leg letter may switch, as the
/// controls check reads it from its element.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Dev {
    pub h: String,
    pub room: String,
    /// Its switch leg letter (lower case), when the plans print one.
    pub leg: String,
    pub mark: String,
    pub kind: String,
    /// How it dims (a luminaire: its driver's; `bim.submittals`).
    pub dimming: String,
    /// What its product record says (a control's: `DIMMER`, `0-10V`, `THREE WAY`...), upper case.
    pub product: String,
    /// (panel, circuit number).
    pub circuit: (String, String),
    pub level: String,
    /// Where, feet.
    pub at: Option<[f64; 2]>,
}

/// How far a switch leg's switch may be from its fixtures, and a 3-way from its pair, feet.
pub(crate) const CONTROL_REACH_FT: f64 = 40.0;
pub(crate) const THREE_WAY_REACH_FT: f64 = 150.0;

/// Each luminaire's control and the links between controls and what they control.
#[derive(Clone, Debug, Default)]
pub(crate) struct Links {
    /// A luminaire's control in words, its source (kind, document), and how it was found.
    pub control: BTreeMap<String, (String, SourceKind, String, &'static str)>,
    /// A luminaire's controls (handles).
    pub controlled_by: BTreeMap<String, Vec<String>>,
    /// A control's luminaires and other loads (handles).
    pub controls: BTreeMap<String, Vec<String>>,
    /// A 3-way switch's mates (handles).
    pub three_way_with: BTreeMap<String, Vec<String>>,
    /// Luminaires that stay on by design (emergency, exit, night light) and why.
    pub unswitched: BTreeMap<String, String>,
    /// Luminaires that light the way out (those that stay on, and those with a battery) and why.
    pub emergency: BTreeMap<String, String>,
    pub tally: BTreeMap<&'static str, usize>,
    pub conflicts: Vec<Conflict>,
}

/// Whether a control dims: a dimmer mark (`SD`, `D`, `DS`), or its product says dimmer.
fn is_dimmer(c: &Dev) -> bool {
    let m = c.mark.to_ascii_uppercase();
    c.kind == "dimmer"
        || m.starts_with("SD")
        || m == "DS"
        || (m.starts_with('D') && m.len() <= 3)
        || m.split(',').any(|p| p.trim() == "D")
        || c.product.contains("DIMMER")
}

/// Whether a control is a 3-way switch.
fn is_three_way(c: &Dev) -> bool {
    let m = c.mark.to_ascii_uppercase();
    c.kind == "3way_switch"
        || m.starts_with("S3")
        || m.split(',').any(|p| p.trim() == "3")
        || c.product.contains("THREE WAY")
        || c.product.contains("3-WAY")
}

fn is_sensor(c: &Dev) -> bool {
    c.kind == "occupancy_sensor" || c.product.contains("SENSOR")
}

/// A dimmer's (or a driver's) way of dimming, from its words.
fn dim_kind(text: &str) -> Option<&'static str> {
    dimming_of(text).or_else(|| norm_dim(text)).map(|k| if k == "DALI" { "DALI" } else { k })
}

fn dist(a: Option<[f64; 2]>, b: Option<[f64; 2]>) -> Option<f64> {
    let (p, q) = (a?, b?);
    Some((p[0] - q[0]).hypot(p[1] - q[1]))
}

/// Within reach on one level (a leg's switch is often across a door, in the next space); with no
/// positions, the same room.
fn near(a: &Dev, b: &Dev, reach: f64) -> bool {
    let same_level = a.level.is_empty() || b.level.is_empty() || a.level.eq_ignore_ascii_case(&b.level);
    same_level
        && match dist(a.at, b.at) {
            Some(d) => d <= reach,
            None => !a.room.is_empty() && a.room == b.room,
        }
}

/// The controls of a luminaire's leg: those of its letter in its room; else, the nearest one of
/// its letter within reach whose own room has no fixture of that letter (a switch by a door
/// controls the space through it; rooms side by side use the same letters, and a switch whose
/// letter lights its own room isn't the next room's), with that one's 3-way mates.
fn leg_controls<'a>(l: &Dev, ctrls: &'a [Dev], lums: &[Dev]) -> Vec<&'a Dev> {
    if l.leg.is_empty() {
        return Vec::new();
    }
    let lettered: Vec<&Dev> = ctrls.iter().filter(|c| c.leg == l.leg && near(l, c, CONTROL_REACH_FT)).collect();
    let mine: Vec<&Dev> = lettered.iter().copied().filter(|c| !l.room.is_empty() && c.room == l.room).collect();
    if !mine.is_empty() {
        return mine;
    }
    let busy = |c: &Dev| !c.room.is_empty() && lums.iter().any(|o| o.h != l.h && o.leg == c.leg && o.room == c.room);
    let lettered: Vec<&Dev> = lettered.into_iter().filter(|c| !busy(c)).collect();
    let gap = |c: &Dev| dist(l.at, c.at).unwrap_or(f64::INFINITY);
    let Some(first) = lettered.iter().copied().min_by(|a, b| gap(a).total_cmp(&gap(b))) else { return Vec::new() };
    let mut out = vec![first];
    if is_three_way(first) {
        out.extend(three_way_mates(first, ctrls));
    }
    out
}

/// A 3-way switch's mates: the other 3-way switches of its letter in its room; else the nearest
/// one of its letter within reach (its pair at the other door, the other end of a corridor).
fn three_way_mates<'a>(c: &Dev, ctrls: &'a [Dev]) -> Vec<&'a Dev> {
    let others: Vec<&Dev> = ctrls.iter().filter(|o| o.h != c.h && is_three_way(o) && o.leg == c.leg && near(o, c, THREE_WAY_REACH_FT)).collect();
    let mine: Vec<&Dev> = others.iter().copied().filter(|o| !c.room.is_empty() && o.room == c.room).collect();
    if !mine.is_empty() {
        return mine;
    }
    let gap = |o: &Dev| dist(c.at, o.at).unwrap_or(f64::INFINITY);
    others.into_iter().min_by(|a, b| gap(a).total_cmp(&gap(b))).into_iter().collect()
}

fn where_of(d: &Dev) -> String {
    match (d.level.is_empty(), d.room.is_empty()) {
        (_, false) => format!("{} room {}", d.level, d.room).trim().to_string(),
        (false, true) => format!("{} (no room)", d.level),
        _ => "no room".into(),
    }
}

/// What the controls check reads besides the elements: each circuit's panel-schedule codes (`C`
/// contactor, `TS` time switch, `EM`), and each fixture type's emergency/exit reason.
pub(crate) struct ControlDocs<'a> {
    pub codes: &'a BTreeMap<(String, String), Vec<String>>,
    pub emergency: &'a BTreeMap<String, Emergency>,
    /// The plans' name for the conflicts' plan side (`the electrical plans`).
    pub plans: &'a str,
}

/// Each luminaire's control: by its leg letter and the switches of that letter near it; else its
/// circuit's contactor or time switch (the panel schedule's codes); else unswitched by design
/// (emergency, exit); else its room's sensor or switches (no letters: a stamped unit's fixtures
/// carry none); else none. The links both ways, 3-way mates, and the conflicts: a fixture with no
/// control, a fixture's leg no switch has, a switch leg no fixture has, a 3-way without its mate,
/// a dimmer over a driver that doesn't dim (or dims another way).
pub(crate) fn link_controls(lums: &[Dev], ctrls: &[Dev], others: &[Dev], docs: &ControlDocs) -> Links {
    let mut out = Links::default();
    // (topic) → the places, for one conflict per topic and room.
    let mut issues: BTreeMap<(&'static str, String, String), Vec<&Dev>> = BTreeMap::new();
    for l in lums {
        let in_room: Vec<&Dev> = ctrls.iter().filter(|c| !l.room.is_empty() && c.room == l.room).collect();
        let by_leg = leg_controls(l, ctrls, lums);
        let circuit_codes = docs.codes.get(&l.circuit).cloned().unwrap_or_default();
        let em = docs.emergency.get(&l.mark).cloned().or_else(|| {
            circuit_codes
                .iter()
                .any(|c| c == "EM")
                .then(|| Emergency { why: format!("panel {} circuit {}: EM", l.circuit.0, l.circuit.1), switched: false })
        });
        if let Some(e) = &em {
            out.emergency.insert(l.h.clone(), e.why.clone());
        }
        // One that stays on by design: no control needed.
        let emergency = em.filter(|e| !e.switched).map(|e| e.why);
        let list = |cs: &[&Dev]| cs.iter().map(|c| format!("{} {}", c.mark, c.h)).collect::<Vec<_>>().join(", ");
        let (control, kind, doc, how): (String, SourceKind, String, &'static str) = if !by_leg.is_empty() {
            (format!("switch leg {}: {}", l.leg, list(&by_leg)), SourceKind::Print, "switch leg letters on the plans".into(), "leg")
        } else if circuit_codes.iter().any(|c| c == "C" || c == "TS") {
            (
                format!(
                    "{} (panel {} circuit {}: {})",
                    if circuit_codes.iter().any(|c| c == "TS") { "time switch" } else { "contactor" },
                    l.circuit.0,
                    l.circuit.1,
                    circuit_codes.join("/")
                ),
                SourceKind::Schedule,
                "panel schedule codes".into(),
                "contactor",
            )
        } else if let Some(why) = &emergency {
            out.unswitched.insert(l.h.clone(), why.clone());
            ("unswitched: emergency / exit".to_string(), SourceKind::Schedule, why.clone(), "emergency")
        } else if !in_room.is_empty() && l.leg.is_empty() {
            (format!("the room's controls: {}", list(&in_room)), SourceKind::Computed, String::new(), "room")
        } else {
            ("none found".to_string(), SourceKind::Computed, String::new(), "none")
        };
        *out.tally.entry(how).or_default() += 1;
        let linked: Vec<&Dev> = match how {
            "leg" => by_leg.clone(),
            "room" => in_room.clone(),
            _ => Vec::new(),
        };
        for c in &linked {
            out.controlled_by.entry(l.h.clone()).or_default().push(c.h.clone());
            out.controls.entry(c.h.clone()).or_default().push(l.h.clone());
        }
        // An exit sign or emergency unit on a leg too: it is still unswitched (its feed ahead of
        // the switching).
        if how == "leg"
            && let Some(why) = &emergency
        {
            out.unswitched.insert(l.h.clone(), why.clone());
        }
        if how == "none" {
            let topic = if l.leg.is_empty() { "fixture with no control" } else { "fixture leg with no switch" };
            issues.entry((topic, l.level.clone(), l.room.clone())).or_default().push(l);
        }
        // A dimmer over a fixture that doesn't dim, or dims another way.
        for c in linked.iter().filter(|c| is_dimmer(c)) {
            let (dk, lk) = (dim_kind(&c.product), dim_kind(&l.dimming));
            if lk == Some("non-dim") || matches!((dk, lk), (Some(a), Some(b)) if a != b) {
                let mut x = Conflict::new(
                    "dimmer over a driver that doesn't dim that way",
                    format!("{} {}", where_of(l), c.mark),
                    format!(
                        "{}: dimmer {} {} ({}) controls luminaire {} {} whose driver is {}",
                        where_of(l),
                        c.mark,
                        c.h,
                        dk.unwrap_or("a dimmer of unknown protocol"),
                        l.mark,
                        l.h,
                        lk.unwrap_or("?")
                    ),
                    "Match the dimmer to the driver (a 0-10V dimmer for 0-10V drivers, a phase dimmer for phase-dimmable ones), or the fixture to the dimmer",
                )
                .side(docs.plans, format!("dimmer {} on leg {}", c.mark, l.leg))
                .side(if l.dimming.is_empty() { "the luminaire's product data".to_string() } else { "the luminaire's submittal".to_string() }, format!("{} driver: {}", l.mark, lk.unwrap_or("?")))
                .with_elements([l.h.clone(), c.h.clone()]);
                x.level.clone_from(&l.level);
                x.at = l.at;
                out.conflicts.push(x);
            }
        }
        out.control.insert(l.h.clone(), (control, kind, doc, how));
    }
    // Switches: a leg no fixture near it has; a 3-way with no other 3-way of its leg within
    // reach (its mate is usually at the other door or the other end of a corridor or stair).
    for c in ctrls {
        if is_sensor(c) {
            continue;
        }
        let lettered: Vec<&Dev> =
            lums.iter().chain(others.iter()).filter(|l| !c.leg.is_empty() && l.leg == c.leg && near(l, c, CONTROL_REACH_FT)).collect();
        for l in lettered.iter().filter(|l| others.iter().any(|o| o.h == l.h)) {
            out.controls.entry(c.h.clone()).or_default().push(l.h.clone());
        }
        // Unlettered fixtures near it are presumed its (a stamped unit's fixtures carry none).
        let presumed = lums.iter().any(|l| l.leg.is_empty() && near(l, c, CONTROL_REACH_FT));
        if !c.leg.is_empty() && lettered.is_empty() && !presumed {
            issues.entry(("switch leg with no fixture", c.level.clone(), c.room.clone())).or_default().push(c);
        }
        if is_three_way(c) {
            let mates = three_way_mates(c, ctrls);
            if mates.is_empty() {
                issues.entry(("3-way switch without its mate", c.level.clone(), c.room.clone())).or_default().push(c);
            } else {
                out.three_way_with.insert(c.h.clone(), mates.iter().map(|m| m.h.clone()).collect());
            }
        }
    }
    for v in out.controls.values_mut() {
        v.sort();
        v.dedup();
    }
    for ((topic, level, room), devs) in issues {
        let place = if room.is_empty() { format!("{level} (no room)") } else { format!("{level} room {room}") }.trim().to_string();
        let marks: Vec<String> = devs.iter().map(|d| if d.leg.is_empty() { d.mark.clone() } else { format!("{} leg {}", d.mark, d.leg) }).collect();
        let (issue, fix, plan_says, other) = match topic {
            "fixture with no control" => (
                format!("{place}: {} luminaires with no control ({})", devs.len(), distinct(marks.clone()).join(", ")),
                "Show their switch leg (or contactor / sensor), or note them unswitched",
                format!("luminaires {} with no switch leg letter, no switch or sensor in the room", distinct(marks).join(", ")),
                ("the panel schedule", "no contactor or time switch on their circuit".to_string()),
            ),
            "fixture leg with no switch" => (
                format!("{place}: {} luminaires on a leg no switch near them has ({})", devs.len(), distinct(marks.clone()).join(", ")),
                "Add the switch for that leg, or correct the fixtures' leg letter",
                format!("luminaires {}", distinct(marks).join(", ")),
                (docs.plans, "no switch of that leg letter within reach".to_string()),
            ),
            "switch leg with no fixture" => (
                format!("{place}: switch {} controls a leg no luminaire near it has", distinct(marks.clone()).join(", ")),
                "Correct the switch's leg letter, or add the fixtures it controls",
                format!("switch {}", distinct(marks).join(", ")),
                (docs.plans, "no luminaire or load of that leg letter within reach".to_string()),
            ),
            _ => (
                format!("{place}: 3-way switch {} has no mate of its leg within reach", distinct(marks.clone()).join(", ")),
                "Show the second 3-way switch, or make it single pole",
                format!("3-way switch {}", distinct(marks).join(", ")),
                (docs.plans, "no second 3-way switch of that leg".to_string()),
            ),
        };
        let mut x = Conflict::new(topic, place.clone(), issue, fix)
            .side(docs.plans, plan_says)
            .side(other.0, other.1)
            .with_elements(devs.iter().map(|d| d.h.clone()));
        x.level = level;
        x.at = devs.first().and_then(|d| d.at);
        out.conflicts.push(x);
    }
    out
}

/// The catalog number a legend line names for its device (`WALL MOUNTED OCCUPANCY SENSOR
/// AB-XY2`): its last word when that is a part number (letters and digits joined by dashes, or
/// both), not a word or a rating (`3-WAY`, `0-10V`, `20A`).
pub(crate) fn legend_catalog(text: &str) -> Option<String> {
    let last = text.split_whitespace().last()?.trim_matches(|c: char| matches!(c, ',' | '.' | ';' | ')' | '('));
    let up = last.to_ascii_uppercase();
    let letters = up.chars().filter(char::is_ascii_alphabetic).count();
    let digits = up.chars().filter(char::is_ascii_digit).count();
    let dashed = up.contains('-') && up.split('-').all(|p| !p.is_empty() && p.chars().all(|c| c.is_ascii_alphanumeric()));
    let rating = ["WAY", "POLE", "V", "A", "AMP", "VOLT", "VAC", "W"]
        .iter()
        .any(|r| up.split('-').next_back().is_some_and(|t| t.trim_start_matches(|c: char| c.is_ascii_digit()) == *r));
    let part =
        up.len() >= 5 && letters >= 2 && (dashed || digits >= 1) && !rating && up.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '/');
    part.then_some(up)
}

// ─── Sensors the energy code or the specification asks for ───────────────────────────────────

/// The energy code's lighting control rule (`coderules.json` E-LIGHT-OCC).
#[derive(Clone, Debug, Default, serde::Deserialize)]
#[serde(default)]
pub(crate) struct SensorRule {
    pub id: String,
    pub cite: String,
    pub rule: String,
    /// The section by edition (`2018` → `IECC 2018 C405.2.1`).
    pub editions: BTreeMap<String, String>,
    /// The spaces whose lights an occupant sensor controls: all of a space's words in its name.
    pub spaces: Vec<SpaceWords>,
    pub guest_rooms: GuestRule,
}

#[derive(Clone, Debug, Default, serde::Deserialize)]
#[serde(default)]
pub(crate) struct SpaceWords {
    pub all: Vec<String>,
}

/// A guest room's master control (a card key, captive key or occupant sensor that turns its
/// lights off when it is left).
#[derive(Clone, Debug, Default, serde::Deserialize)]
#[serde(default)]
pub(crate) struct GuestRule {
    pub editions: BTreeMap<String, String>,
    pub rule: String,
    /// Words of a control that is one (`CARD`, `KEY`, `MASTER`, `OCCUPANCY`).
    pub controls: Vec<String>,
}

pub(crate) fn sensor_rule() -> SensorRule {
    let v: serde_json::Value = serde_json::from_str(include_str!("coderules.json")).unwrap_or_default();
    v.get("rules")
        .and_then(serde_json::Value::as_array)
        .and_then(|a| a.iter().find(|r| r.get("id").and_then(serde_json::Value::as_str) == Some("E-LIGHT-OCC")))
        .and_then(|r| serde_json::from_value(r.clone()).ok())
        .unwrap_or_default()
}

/// The energy code the documents name (`2018 INTERNATIONAL ENERGY CONSERVATION CODE`, `IECC
/// 2018`, `2018 IECC`): its year.
pub(crate) fn energy_code_year<'a, I: IntoIterator<Item = &'a str>>(texts: I) -> Option<String> {
    for t in texts {
        let up = t.to_ascii_uppercase();
        let ws: Vec<&str> = up.split(|c: char| !c.is_ascii_alphanumeric()).filter(|w| !w.is_empty()).collect();
        let year = |w: &str| w.len() == 4 && w.starts_with("20") && w.chars().all(|c| c.is_ascii_digit());
        for (i, w) in ws.iter().enumerate() {
            let named = *w == "IECC" || (*w == "ENERGY" && ws.get(i.wrapping_sub(1)).is_some_and(|p| *p == "INTERNATIONAL"));
            if !named {
                continue;
            }
            let before = if *w == "IECC" { i.checked_sub(1) } else { i.checked_sub(2) };
            if let Some(y) = before.and_then(|b| ws.get(b)).filter(|y| year(y)) {
                return Some((*y).to_string());
            }
            if let Some(y) = ws.get(i + 1).filter(|y| year(y)) {
                return Some((*y).to_string());
            }
        }
    }
    None
}

/// The spaces a specification's lines put sensors in (`Provide occupancy sensors in all
/// restrooms and storage rooms`): the rule's spaces each such line names, with the line.
pub(crate) fn spec_sensor_spaces(rule: &SensorRule, lines: &[(String, String)]) -> Vec<(usize, String)> {
    let mut out = Vec::new();
    for (line, doc) in lines.iter().take(20_000) {
        let up = line.to_ascii_uppercase();
        if !(up.contains("SENSOR") && (up.contains("PROVIDE") || up.contains("INSTALL") || up.contains("SHALL") || up.contains("REQUIRED"))) {
            continue;
        }
        let ws = words(&up);
        for (i, sp) in rule.spaces.iter().enumerate() {
            if !sp.all.is_empty() && sp.all.iter().all(|w| ws.iter().any(|x| x.starts_with(w.as_str()))) && !out.iter().any(|(j, _)| *j == i) {
                out.push((i, format!("{doc}: {}", line.trim())));
            }
        }
    }
    out
}

fn words(up: &str) -> Vec<String> {
    up.split(|c: char| !c.is_ascii_alphanumeric()).filter(|w| !w.is_empty()).map(str::to_string).collect()
}

/// A room the sensor check looks at: its number (what devices carry as `room`), its name, its
/// level, and its unit type when it is a guest room.
#[derive(Clone, Debug, Default)]
pub(crate) struct SensorRoom {
    pub number: String,
    pub name: String,
    pub level: String,
    pub unit_type: Option<String>,
}

/// A space needing a sensor (the energy code's list, or the specification's) whose luminaires no
/// sensor controls; a guest room with no master control; and a sensor where neither asks for
/// one. `year`: the energy code edition the documents name.
pub(crate) fn sensor_conflicts(
    rule: &SensorRule,
    year: Option<&str>,
    spec_spaces: &[(usize, String)],
    rooms: &[SensorRoom],
    lums: &[Dev],
    ctrls: &[Dev],
    plans: &str,
) -> Vec<Conflict> {
    let mut out = Vec::new();
    if rule.id.is_empty() {
        return out;
    }
    let cite = |eds: &BTreeMap<String, String>, fallback: &str| -> String {
        year.and_then(|y| eds.get(y).cloned()).unwrap_or_else(|| match year {
            Some(y) => format!("{fallback} ({y} edition named by the documents; section not in the rules data)"),
            None => format!("{fallback} (edition not named by the documents)"),
        })
    };
    let code = cite(&rule.editions, &rule.cite);
    let guest_code = cite(&rule.guest_rooms.editions, &rule.cite);
    let room_key = |r: &SensorRoom| r.number.trim().to_ascii_uppercase();
    let in_room = |r: &SensorRoom, d: &Dev| {
        let k = room_key(r);
        !k.is_empty()
            && d.room.trim().eq_ignore_ascii_case(&k)
            && (d.level.is_empty() || r.level.is_empty() || d.level.eq_ignore_ascii_case(&r.level))
    };
    // Guest rooms by type: the rooms with no master control.
    let mut guest: BTreeMap<String, (Vec<String>, Vec<String>, BTreeSet<String>)> = BTreeMap::new();
    // Spaces with a sensor neither asks for, by name: their rooms, sensor marks and handles.
    let mut unasked: BTreeMap<String, (Vec<String>, BTreeSet<String>, Vec<String>)> = BTreeMap::new();
    let master = |c: &Dev| {
        is_sensor(c) || {
            let u = format!("{} {}", c.mark, c.product).to_ascii_uppercase();
            rule.guest_rooms.controls.iter().any(|w| u.contains(w.as_str()))
        }
    };
    for r in rooms {
        let lit: Vec<&Dev> = lums.iter().filter(|l| in_room(r, l)).collect();
        if lit.is_empty() {
            continue;
        }
        let mine: Vec<&Dev> = ctrls.iter().filter(|c| in_room(r, c)).collect();
        let sensed = mine.iter().any(|c| is_sensor(c));
        if let Some(t) = &r.unit_type {
            if !mine.iter().any(|c| master(c)) && !rule.guest_rooms.rule.is_empty() {
                let g = guest.entry(t.clone()).or_default();
                g.0.push(room_key(r));
                g.1.extend(lit.iter().map(|l| l.h.clone()));
                g.2.extend(mine.iter().map(|c| c.mark.clone()));
            }
            continue;
        }
        let name = r.name.to_ascii_uppercase();
        let ws = words(&name);
        let space = rule.spaces.iter().position(|sp| !sp.all.is_empty() && sp.all.iter().all(|w| ws.iter().any(|x| x.starts_with(w.as_str()))));
        let by_spec = space.and_then(|i| spec_spaces.iter().find(|(j, _)| *j == i));
        let place = format!("{}room {} {}", if r.level.is_empty() { String::new() } else { format!("level {} ", r.level) }, r.number, r.name)
            .trim()
            .to_string();
        match (space, sensed) {
            (Some(_), false) => {
                let mut c = Conflict::new(
                    "sensor required, none shown",
                    place.clone(),
                    format!(
                        "{place}: {} requires an occupant sensor for its lights; the plans show none ({} luminaires, controls {})",
                        by_spec.map_or(code.as_str(), |(_, d)| d.as_str()),
                        lit.len(),
                        if mine.is_empty() { "none".to_string() } else { distinct(mine.iter().map(|c| c.mark.clone())).join(", ") }
                    ),
                    "Add an occupancy or vacancy sensor (or confirm the space is exempt)",
                )
                .side(code.clone(), rule.rule.clone())
                .side(plans, format!("no sensor in room {}", r.number))
                .with_elements(lit.iter().map(|l| l.h.clone()));
                if let Some((_, d)) = by_spec {
                    c = c.side(d.clone(), "sensors in this space");
                }
                c.severity = Some(cadaddy_walk3d::rfi::Severity::High);
                c.level.clone_from(&r.level);
                c.at = lit.first().and_then(|l| l.at);
                out.push(c);
            }
            (None, true) => {
                let e = unasked.entry(name.clone()).or_default();
                e.0.push(format!("{}{}", if r.level.is_empty() { String::new() } else { format!("level {} ", r.level) }, r.number));
                e.1.extend(mine.iter().filter(|c| is_sensor(c)).map(|c| c.mark.clone()));
                e.2.extend(mine.iter().filter(|c| is_sensor(c)).map(|c| c.h.clone()));
            }
            _ => {}
        }
    }
    // Sensors in spaces neither asks for one in: one item per space name.
    for (name, (places, marks, hs)) in unasked {
        let mut c = Conflict::new(
            "sensor where none is required",
            format!("spaces named {name}"),
            format!(
                "{name} ({}): the plans show a sensor; neither the energy code's list ({code}) nor the specification asks for one in a space so named",
                places.join(", ")
            ),
            "Confirm the sensor is wanted (it is extra material and labor), or name the space so its need is clear",
        )
        .side(plans, format!("sensor {}", marks.into_iter().collect::<Vec<_>>().join(", ")))
        .side(code.clone(), format!("not among its spaces ({name})"))
        .with_elements(hs);
        c.severity = Some(cadaddy_walk3d::rfi::Severity::Low);
        out.push(c);
    }
    for (t, (rooms, lit, marks)) in guest {
        let shown: Vec<&String> = rooms.iter().take(12).collect();
        let more = if rooms.len() > shown.len() { format!(" and {} more", rooms.len() - shown.len()) } else { String::new() };
        let mut c = Conflict::new(
                "guest room master control",
                format!("guest rooms {t}"),
                format!(
                    "{} {t} guest rooms ({}{more}): {guest_code} asks for a master control turning the room's lights off when it is left; the plans show only {}",
                    rooms.len(),
                    shown.iter().map(|x| x.as_str()).collect::<Vec<_>>().join(", "),
                    if marks.is_empty() { "no control".to_string() } else { format!("switches {}", marks.into_iter().collect::<Vec<_>>().join(", ")) }
                ),
                "Add the master control (card key or captive key switch, or an occupant sensor), or confirm the project's energy compliance path exempts it",
            )
            .side(guest_code.clone(), rule.guest_rooms.rule.clone())
            .side(plans, format!("no card key, master switch or sensor in the {t} rooms"))
            .with_elements(lit);
        c.severity = Some(cadaddy_walk3d::rfi::Severity::High);
        out.push(c);
    }
    out
}

// ─── The enlarged unit plans against the floor plans ─────────────────────────────────────────

/// A fixture or switch a floor plan shows in a typical room that the enlarged plan of its type
/// doesn't (or tags otherwise).
#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub(crate) struct PlanDiff {
    pub room: String,
    pub level: String,
    /// The typical view (`E-6 UNIT ELECTRICAL PLAN - …`).
    pub view: String,
    pub kind: String,
    pub floor_tag: String,
    pub floor_sheet: String,
    /// The enlarged plan's tag of that kind nearest it, when it has one there.
    pub enlarged_tag: Option<String>,
    pub at: [f64; 2],
}

/// Whether a kind is lighting (a luminaire or its control).
pub(crate) fn lighting_kind(kind: &str) -> bool {
    let k = kind.to_ascii_lowercase();
    ["luminaire", "light", "switch", "dimmer", "sensor"].iter().any(|w| k.contains(w))
}

/// The differences by view, kind and tags: one conflict each, its rooms listed.
pub(crate) fn plan_diff_conflicts(diffs: &[PlanDiff]) -> Vec<Conflict> {
    let mut groups: BTreeMap<(String, String, String, Option<String>), Vec<&PlanDiff>> = BTreeMap::new();
    for d in diffs {
        groups.entry((d.view.clone(), d.kind.clone(), d.floor_tag.clone(), d.enlarged_tag.clone())).or_default().push(d);
    }
    groups
        .into_iter()
        .map(|((view, kind, ft, et), ds)| {
            let rooms = distinct(ds.iter().map(|d| d.room.clone()));
            let shown: Vec<&String> = rooms.iter().take(12).collect();
            let more = if rooms.len() > shown.len() { format!(" and {} more", rooms.len() - shown.len()) } else { String::new() };
            let floor = distinct(ds.iter().map(|d| d.floor_sheet.clone())).join(", ");
            let (topic, issue, enlarged_says) = match &et {
                Some(e) => (
                    "enlarged plan tag differs from the floor plan",
                    format!("{kind} tagged {ft} on the floor plan ({floor}) is tagged {e} on the enlarged plan {view}, in {} rooms", rooms.len()),
                    format!("{kind} {e}"),
                ),
                None => (
                    "floor plan shows what the enlarged plan doesn't",
                    format!("the floor plan ({floor}) shows {kind} {ft} in {} rooms where the enlarged plan {view} has no {kind} {ft}", rooms.len()),
                    format!("no {kind} {ft}"),
                ),
            };
            let mut c = Conflict::new(
                topic,
                format!("{view} {kind} {ft}"),
                format!("{issue}: {}{more}", shown.iter().map(|x| x.as_str()).collect::<Vec<_>>().join(", ")),
                "Confirm which plan holds (the enlarged plan usually governs the typical room), and correct the other",
            )
            .side(floor, format!("{kind} {ft}"))
            .side(view.clone(), enlarged_says);
            c.level = ds.first().map(|d| d.level.clone()).unwrap_or_default();
            c.at = ds.first().map(|d| d.at);
            c
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(tag: &str, maker: &str, model: &str, lamp: &str, install: &str, desc: &str) -> FixtureType {
        FixtureType {
            tag: tag.into(),
            manufacturer: maker.into(),
            model: model.into(),
            lamp: lamp.into(),
            va: watts_of(lamp),
            voltage: "120".into(),
            installation: install.into(),
            description: desc.into(),
            source: "E-9 LIGHT FIXTURE SCHEDULE".into(),
            ..FixtureType::default()
        }
    }

    #[test]
    fn catalog_numbers_and_lines_give_their_values() {
        assert_eq!(cct_of("CSVT L48 4000LM MVOLT 40K 80CRI"), Some(4000));
        assert_eq!(cct_of("FMLWL 48 840"), Some(4000));
        assert_eq!(cct_of("WL4 40L GZ1 LP830"), Some(3000));
        assert_eq!(cct_of("CRI of minimum 80. CCT of 3500 K."), Some(3500));
        assert_eq!(cct_of("D-10413-08 L3G9D2527K"), None, "a wattage run into a CCT is no CCT");
        assert_eq!(cct_of("SMX6RLSFS010"), None);
        assert_eq!(lumens_of("CSVT L48 4000LM MVOLT"), Some(4000));
        assert_eq!(lumens_of("6000 LUMENS"), Some(6000));
        assert_eq!(volts_of("CSVT L48 MVOLT"), Some((120, 277)));
        assert_eq!(volts_of("120-277V"), Some((120, 277)));
        assert_eq!(volts_of("120"), Some((120, 120)));
        assert_eq!(volts_of("Nominal Operating Voltage: 120 V ac."), Some((120, 120)));
        assert_eq!(watts_of("LED 13.8VA"), Some(13.8));
        assert_eq!(watts_of("LED 39 VA"), Some(39.0));
        assert_eq!(dimming_of("120V LE/TE phase cut"), Some("phase"));
        assert_eq!(dimming_of("0-10V DIMMING DRIVER"), Some("0-10V"));
        assert_eq!(mounting_of("SURFACE/CEILING"), Some("ceiling"));
        assert_eq!(mounting_of("7'-6\" AFF"), Some("wall"));
        assert_eq!(mounting_of("CEILING/VARIES"), None);
        assert_eq!(finish_of("POWDER COATED MATTE BLACK"), Some("black"));
        assert_eq!(legend_catalog("WALL MOUNTED OCCUPANCY SENSOR AB-XY2"), Some("AB-XY2".into()));
        assert_eq!(legend_catalog("WALL MOUNTED OCCUPANCY SENSOR WSQ-PDT"), Some("WSQ-PDT".into()));
        assert_eq!(legend_catalog("switch, 3-way"), None);
        assert_eq!(legend_catalog("DIMMER, 0-10V"), None);
        assert_eq!(legend_catalog("SINGLE POLE WALL SWITCH"), None);
        assert_eq!(legend_catalog("SWITCH 20A"), None);
    }

    #[test]
    fn the_submittal_speaks_first_and_a_disagreement_cites_both_documents() {
        let r = row("Q2", "MAKER ONE", "AB12 L48 3000LM MVOLT 35K", "LED 30VA", "RECESSED", "OFFICE TROFFER");
        let sched = schedule_says(&r);
        assert_eq!(sched["cct"].value, "3500K");
        assert_eq!(sched["mounting_type"].value, "RECESSED");
        let sub = submittal_says(&[
            ("manufacturer", "Maker One Lighting".into(), "Package p.4".into()),
            ("catalog_number", "AB12 L48 4000LM MVOLT 40K".into(), "Package p.4".into()),
            ("mounting_type", "pendant".into(), "Package p.4".into()),
        ]);
        let spec = read_spec_lighting(
            "SECTION 265100 - INTERIOR LIGHTING\nPART 2 - PRODUCTS\n--- page 3 ---\nCRI of minimum 80. CCT of 3500 K.\nNominal Operating Voltage: 120 V ac.\nFinish color as noted on drawings.\nPART 3 - EXECUTION\nCCT of 5000 K.",
            "spec",
        );
        assert_eq!(spec.section, "26 51 00 INTERIOR LIGHTING");
        let sp = spec.says();
        assert_eq!(sp["cct"].value, "3500K", "PART 3 doesn't set products");
        assert_eq!(sp["voltage"].value, "120");
        let (chosen, conflicts) = resolve(&[&sub, &sched, &sp]);
        // Each field from the first document that gives it.
        assert_eq!(chosen["manufacturer"].kind, SourceKind::Submittal);
        assert_eq!(chosen["cct"].value, "4000K", "decoded from the submitted catalog number");
        assert_eq!(chosen["lamp"].kind, SourceKind::Schedule, "the submittal says nothing of it: the schedule's");
        assert_eq!(chosen["cri"].kind, SourceKind::Spec);
        // MVOLT runs on 120 V: no conflict. The maker is one. The CCT, lumens and mounting differ.
        let fields: Vec<&str> = conflicts.iter().map(|c| c.field).collect();
        assert_eq!(fields, vec!["cct", "lumens", "mounting_type"], "{conflicts:?}");
        let cct = conflicts.iter().find(|c| c.field == "cct").unwrap();
        let docs: Vec<SourceKind> = cct.sides.iter().map(|s| s.kind).collect();
        assert_eq!(docs, vec![SourceKind::Submittal, SourceKind::Schedule, SourceKind::Spec]);
        let items = field_conflicts("Q2", &conflicts, &["1a".into()]);
        assert_eq!(items.len(), 3);
        assert!(items[0].sides.len() >= 2 && items[0].issue.contains("vs"), "{:?}", items[0]);
        let rfi = items[0].to_rfi("lighting");
        assert_eq!(rfi.kind_name(), "conflict");
        assert_eq!(rfi.topic, "lighting: CCT");
        assert_eq!(rfi.sides.len(), 3);
        assert_eq!(rfi.elements, vec!["1a".to_string()]);
    }

    #[test]
    fn a_value_left_to_another_document_is_none() {
        let r = row("Q9", "REFER TO SITE LIGHTING PACKAGE", "REFER TO SITE LIGHTING PACKAGE", "LED", "SURFACE", "EXTERIOR WALL PACK");
        let s = schedule_says(&r);
        assert!(!s.contains_key("manufacturer") && !s.contains_key("catalog_number"));
        assert!(!interior(&r));
        assert!(emergency_type(&r).is_none());
        let e = row("Q8", "MAKER", "EM2", "LED 1VA", "7'-6\" AFF", "EMERGENCY LIGHT");
        assert!(emergency_type(&e).is_some_and(|w| w.why.contains("EMERGENCY") && !w.switched));
        assert!(!interior(&e), "an emergency unit is the emergency lighting section's");
        let b = row("Q6", "MAKER", "WL4 40L LP830", "LED 39VA", "TBD", "LINEAR STAIR LIGHT *PROVIDE WITH EMERGENCY BATTERY BACK-UP");
        assert!(interior(&b), "a luminaire with a battery is an interior luminaire");
        assert!(emergency_type(&b).is_some_and(|w| w.switched));
    }

    #[test]
    fn the_types_are_checked_across_the_plans_schedule_and_submittal() {
        let mut a = row("Q1", "MAKER ONE", "AB12", "LED 13VA", "RECESSED", "DOWNLIGHT");
        a.quantity = Some(3);
        let schedule = vec![
            a,
            row("Q3", "MAKER TWO", "CS71SD", "LED 1VA", "CEILING", "EXIT SIGN"),
            row("Q4", "MAKER TWO", "EM2", "LED", "WALL", "EMERGENCY LIGHT"),
            row("Q5", "MAKER", "X1", "LED", "SURFACE", "LOBBY"),
        ];
        let mut submitted = BTreeMap::new();
        submitted.insert("Q1".to_string(), ("Maker One".to_string(), "AB12".to_string(), "Package p.2".to_string()));
        submitted.insert("Q3".to_string(), ("Maker Two".to_string(), "CX71SD".to_string(), "Package p.9".to_string()));
        submitted.insert("Q4".to_string(), ("Maker Two".to_string(), "CX71SD".to_string(), "Package p.7".to_string()));
        submitted.insert("Q7".to_string(), ("Maker Three".to_string(), "ZZ1".to_string(), "Package p.12".to_string()));
        let mut on_plans = BTreeMap::new();
        on_plans.insert("Q1".to_string(), vec!["a1".to_string(), "a2".to_string()]);
        on_plans.insert("Q3".to_string(), vec!["b1".to_string()]);
        on_plans.insert("Q4".to_string(), vec!["c1".to_string()]);
        on_plans.insert("Q6".to_string(), vec!["d1".to_string()]);
        let qty = BTreeMap::new();
        let cs = type_conflicts(&TypeCheck {
            schedule: &schedule,
            submitted: &submitted,
            on_plans: &on_plans,
            submitted_qty: &qty,
            submittal_name: "Package",
        });
        let topics = by_topic(&cs);
        assert_eq!(topics.get("plan type with no schedule row"), Some(&1), "{topics:?}");
        assert_eq!(topics.get("scheduled type with no submittal"), Some(&1), "Q5");
        assert_eq!(topics.get("scheduled type on no plan"), Some(&1), "Q5");
        assert_eq!(topics.get("plan count vs quantity"), Some(&1), "Q1: 2 vs 3");
        assert_eq!(topics.get("submitted type with no schedule row"), Some(&1), "Q7");
        assert_eq!(topics.get("one product for two types"), Some(&1), "Q3 and Q4");
        assert!(cs.iter().all(|c| c.sides.len() >= 2), "every conflict cites both sides: {cs:?}");
        let two = cs.iter().find(|c| c.topic == "one product for two types").unwrap();
        assert!(two.sides.iter().any(|(_, s)| s.contains("EXIT SIGN")) && two.sides.iter().any(|(_, s)| s.contains("EMERGENCY LIGHT")), "{two:?}");
    }

    fn dev(h: &str, kind: &str, mark: &str, room: &str, leg: &str, at: [f64; 2]) -> Dev {
        Dev {
            h: h.into(),
            kind: kind.into(),
            mark: mark.into(),
            room: room.into(),
            leg: leg.into(),
            level: "L1".into(),
            at: Some(at),
            ..Dev::default()
        }
    }

    #[test]
    fn controls_link_both_ways_and_each_gap_is_a_conflict_citing_both_sides() {
        let mut dim = dev("d1", "luminaire", "Q4", "101", "b", [5.0, 5.0]);
        dim.dimming = "phase".into();
        let mut contacted = dev("k1", "luminaire", "Q1", "", "", [300.0, 0.0]);
        contacted.circuit = ("P".into(), "7".into());
        let lums = vec![
            dev("l1", "luminaire", "Q1", "101", "a", [0.0, 0.0]),
            dev("l2", "luminaire", "Q1", "101", "a", [10.0, 0.0]),
            dim,
            dev("e1", "luminaire", "Q8", "101", "", [8.0, 8.0]),
            dev("l3", "luminaire", "Q1", "103", "c", [100.0, 0.0]),
            dev("l4", "luminaire", "Q1", "105", "", [200.0, 0.0]),
            dev("l5", "luminaire", "Q1", "107", "e", [400.0, 0.0]),
            contacted,
        ];
        let mut sd = dev("s3", "dimmer", "SD", "101", "b", [6.0, 1.0]);
        sd.product = "0-10V DIMMER".into();
        let ctrls = vec![
            dev("s1", "3way_switch", "S3", "101", "a", [2.0, 0.0]),
            dev("s2", "3way_switch", "S3", "101", "a", [30.0, 0.0]),
            sd,
            dev("s4", "switch", "S", "102", "d", [600.0, 0.0]),
            dev("s5", "3way_switch", "S3", "107", "e", [402.0, 0.0]),
        ];
        let mut codes = BTreeMap::new();
        codes.insert(("P".to_string(), "7".to_string()), vec!["C".to_string()]);
        let mut emergency = BTreeMap::new();
        emergency.insert("Q8".to_string(), Emergency { why: "E-9 LIGHT FIXTURE SCHEDULE type Q8: EMERGENCY LIGHT".into(), switched: false });
        // The room next door uses the same letters: its switches are its own.
        let mut lums = lums;
        lums.push(dev("n1", "luminaire", "Q1", "103A", "a", [14.0, 3.0]));
        let mut ctrls = ctrls;
        ctrls.push(dev("n2", "3way_switch", "S3", "103A", "a", [16.0, 3.0]));
        ctrls.push(dev("n3", "3way_switch", "S3", "103A", "a", [24.0, 3.0]));
        // A bath light of the room next door, its own room's switches lettered otherwise: not this
        // room's 3-way (its letter lights this room).
        lums.push(dev("n4", "luminaire", "Q2", "103B", "a", [12.0, -3.0]));
        ctrls.push(dev("n5", "switch", "S", "103B", "b", [12.0, -6.0]));
        let links = link_controls(&lums, &ctrls, &[], &ControlDocs { codes: &codes, emergency: &emergency, plans: "the plans" });
        assert_eq!(links.controlled_by["n1"], vec!["n2".to_string(), "n3".to_string()]);
        assert!(!links.controlled_by.contains_key("n4"), "{:?}", links.controlled_by.get("n4"));
        assert_eq!(links.three_way_with["n2"], vec!["n3".to_string()]);
        // Both ways: the leg's fixtures and its 3-way pair.
        assert_eq!(links.controlled_by["l1"], vec!["s1".to_string(), "s2".to_string()]);
        assert_eq!(links.controls["s1"], vec!["l1".to_string(), "l2".to_string()]);
        assert_eq!(links.three_way_with["s1"], vec!["s2".to_string()]);
        assert_eq!(links.control["k1"].3, "contactor");
        assert_eq!(links.control["e1"].3, "emergency");
        assert!(links.unswitched["e1"].contains("EMERGENCY"));
        let topics = by_topic(&links.conflicts);
        assert_eq!(topics.get("dimmer over a driver that doesn't dim that way"), Some(&1), "{topics:?}");
        assert_eq!(topics.get("fixture leg with no switch"), Some(&2), "l3, and n4 (its room has no switch of its letter)");
        assert_eq!(topics.get("fixture with no control"), Some(&1), "l4");
        assert_eq!(topics.get("switch leg with no fixture"), Some(&1), "s4");
        assert_eq!(topics.get("3-way switch without its mate"), Some(&1), "s5");
        assert!(links.conflicts.iter().all(|c| c.sides.len() >= 2 && !c.elements.is_empty()), "{:?}", links.conflicts);
        let mixed = links.conflicts.iter().find(|c| c.topic.starts_with("dimmer")).unwrap();
        assert!(mixed.issue.contains("0-10V") && mixed.issue.contains("phase"), "{}", mixed.issue);
    }

    #[test]
    fn the_energy_code_asks_for_sensors_and_guest_room_master_controls() {
        let rule = sensor_rule();
        assert_eq!(rule.id, "E-LIGHT-OCC");
        assert_eq!(energy_code_year(["COMPLY WITH THE 2018 INTERNATIONAL ENERGY CONSERVATION CODE"]), Some("2018".into()));
        assert_eq!(energy_code_year(["IECC 2021 COMPLIANCE"]), Some("2021".into()));
        assert_eq!(energy_code_year(["2015 IECC"]), Some("2015".into()));
        assert_eq!(energy_code_year(["2018 INTERNATIONAL BUILDING CODE"]), None);
        let rooms = vec![
            SensorRoom { number: "110".into(), name: "STORAGE".into(), level: "L1".into(), unit_type: None },
            SensorRoom { number: "111".into(), name: "CORRIDOR".into(), level: "L1".into(), unit_type: None },
            SensorRoom { number: "112".into(), name: "MEN'S RESTROOM".into(), level: "L1".into(), unit_type: None },
            SensorRoom { number: "201".into(), name: "TYPE 7A".into(), level: "L2".into(), unit_type: Some("TYPE 7A".into()) },
            SensorRoom { number: "202".into(), name: "TYPE 7A".into(), level: "L2".into(), unit_type: Some("TYPE 7A".into()) },
            SensorRoom { number: "203".into(), name: "TYPE 7B".into(), level: "L2".into(), unit_type: Some("TYPE 7B".into()) },
        ];
        let l = |h: &str, room: &str, level: &str| Dev {
            h: h.into(),
            kind: "luminaire".into(),
            mark: "Q1".into(),
            room: room.into(),
            level: level.into(),
            ..Dev::default()
        };
        let lums = vec![l("a", "110", "L1"), l("b", "111", "L1"), l("c", "112", "L1"), l("d", "201", "L2"), l("e", "202", "L2"), l("f", "203", "L2")];
        let c = |h: &str, kind: &str, mark: &str, room: &str, level: &str| Dev {
            h: h.into(),
            kind: kind.into(),
            mark: mark.into(),
            room: room.into(),
            level: level.into(),
            ..Dev::default()
        };
        let ctrls = vec![
            c("s1", "switch", "S", "110", "L1"),
            c("o1", "occupancy_sensor", "OS1", "111", "L1"),
            c("o2", "occupancy_sensor", "OS2", "112", "L1"),
            c("s2", "switch", "S", "201", "L2"),
            c("o3", "occupancy_sensor", "OS1", "203", "L2"),
        ];
        let spec_lines = vec![("Provide occupancy sensors in all storage rooms.".to_string(), "26 09 23 p.4".to_string())];
        let spec = spec_sensor_spaces(&rule, &spec_lines);
        assert_eq!(spec.len(), 1);
        let cs = sensor_conflicts(&rule, Some("2018"), &spec, &rooms, &lums, &ctrls, "the plans");
        let topics = by_topic(&cs);
        assert_eq!(topics.get("sensor required, none shown"), Some(&1), "{cs:?}");
        assert_eq!(topics.get("sensor where none is required"), Some(&1), "the corridor");
        assert_eq!(topics.get("guest room master control"), Some(&1), "TYPE 7A, not 7B (it has a sensor)");
        let need = cs.iter().find(|c| c.topic == "sensor required, none shown").unwrap();
        assert!(need.sides.iter().any(|(d, _)| d == "IECC 2018 C405.2.1") && need.sides.iter().any(|(d, _)| d.starts_with("26 09 23")), "{need:?}");
        let guest = cs.iter().find(|c| c.topic == "guest room master control").unwrap();
        assert!(
            guest.issue.starts_with("2 TYPE 7A guest rooms (201, 202)") && guest.sides.iter().any(|(d, _)| d == "IECC 2018 C405.2.4"),
            "{guest:?}"
        );
        // No edition named: the rule's section, said so.
        let cs = sensor_conflicts(&rule, None, &[], &rooms, &lums, &ctrls, "the plans");
        assert!(cs.iter().any(|c| c.sides.iter().any(|(d, _)| d.contains("edition not named"))));
    }

    #[test]
    fn enlarged_plan_differences_are_one_conflict_per_view_kind_and_tags() {
        let d = |room: &str, et: Option<&str>| PlanDiff {
            room: room.into(),
            level: "L2".into(),
            view: "E-9 UNIT PLAN TYPE 7A".into(),
            kind: "luminaire".into(),
            floor_tag: "Q1".into(),
            floor_sheet: "E-3".into(),
            enlarged_tag: et.map(str::to_string),
            at: [0.0, 0.0],
        };
        let cs = plan_diff_conflicts(&[d("201", Some("Q2")), d("202", Some("Q2")), d("203", None)]);
        assert_eq!(cs.len(), 2);
        let tag = cs.iter().find(|c| c.topic.contains("tag differs")).unwrap();
        assert!(tag.issue.contains("in 2 rooms") && tag.issue.contains("201, 202"), "{}", tag.issue);
        assert_eq!(tag.sides.len(), 2);
        assert!(lighting_kind("wall luminaire") && lighting_kind("3way_switch") && !lighting_kind("receptacle"));
    }
}
