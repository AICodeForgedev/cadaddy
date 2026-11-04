//! Products against the submittals, the specifications and the code (Q6137): the G-PRODUCT rule
//! family the prescreen measures with no agent.
//!
//! - **G-PRODUCT**: each device, luminaire, plumbing fixture and piece of equipment, field by field
//!   — maker, catalog number, colour/finish, configuration, rating and required features — against
//!   what the documents require of it: the maker in the approved list (its type's submittal first,
//!   else the specification's list of makers), the catalog number its type's submittal gives, and
//!   the colour, configuration, rating and features its specification section (or the prints'
//!   spec rules) requires. A field the model doesn't know is `unsure` ("no product data"), counted
//!   per kind and never passed; a field no document requires is not checked (counted in
//!   `unmeasured`).
//! - **G-PRODUCT-CODE**: the features the code requires by location, from the code rules data
//!   (`coderules.json`, the one file the build applies with `bim.coderules` and Validate checks
//!   again), each finding citing the rule's text: tamper-resistant receptacles in guest rooms and
//!   dwelling units, GFCI protection judged on the whole circuit, weather-resistant receptacles
//!   outdoors.
//!
//! GFCI is judged per circuit. The receptacle's circuit (panel + number) and that breaker's type
//! are read again from the panel schedule; the build's stamped verdicts (`gfci_breaker`, `gfci`,
//! `protected_by`) are never read. Under a GFCI breaker the receptacle must be a standard device
//! (a GFCI one there is double protection: `fail`). Where the location needs GFCI (the plan tags
//! it so, it is within reach of a sink, or its room or circuit is a place the rules list) and the
//! breaker isn't GFCI, the receptacle must be a GFCI device or on the load side of one on the same
//! circuit (a GFCI receptacle on that circuit nearer its panel), which the finding names. An
//! unknown circuit or breaker type is `unsure` ("breaker type unknown"), never a pass. A finding
//! cites the breaker's schedule row as well as the device.
//!
//! What the model's product IS comes from the element: its own properties (and its source
//! object's), then its product record's `resolved` product, then its type's fields. What the
//! documents REQUIRE comes from the submittals (the prints' submittal rows, the type catalog's
//! submittal-sourced fields, the record's `approved` submittal), the specification (the record's
//! `required` section, page and product; the prints' spec rules) and the code rules data.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use cadaddy_doc::props::Props;
use cadaddy_doc::{Drawing, Entity};
use cadaddy_walk3d::BuildingSpec;
use cadaddy_walk3d::validate::{Prints, SpecRule};
use cadaddy_walk3d::validate_kinds;
use serde_json::{Value, json};

use super::coderules::{self as cr, CodeRule};
use super::validate_lighting::{Lit, agree};
use super::validate_packet::{Area, pt, r2};
use super::validate_prescreen::Units;
use super::validate_rules::Effective;

/// The product fields against the submittals and the specification.
pub const RULE: &str = "G-PRODUCT";
/// The features the code requires by location.
pub const CODE: &str = "G-PRODUCT-CODE";
/// The rule family, in the order it is run.
pub const RULES: [&str; 2] = [RULE, CODE];
/// The verdicts each rule gives (listed with 0 when none).
pub const VERDICTS: [(&str, &[&str]); 2] = [(RULE, &["pass", "fail", "unsure"]), (CODE, &["pass", "fail", "unsure"])];
/// What an unsure verdict for an unknown model field says.
pub const NO_DATA: &str = "no product data";
/// What an unsure GFCI verdict says when the circuit or its breaker isn't known.
pub const BREAKER_UNKNOWN: &str = "breaker type unknown";
/// The product fields G-PRODUCT compares.
pub const FIELDS: [&str; 6] = ["maker", "catalog", "color", "configuration", "rating", "features"];

/// Rows, elements and products read, at most.
const MAX_ROWS: usize = 200_000;
/// Characters of a citation; words of one text looked at.
const MAX_CITE: usize = 160;
const MAX_WORDS: usize = 2_000;
/// Kinds listed in the product counts, at most.
const MAX_KINDS: usize = 60;
/// Half a sink's width: the model keeps a fixture's middle, the code's reach is from its edge.
const SINK_HALF_FT: f64 = 1.0;
/// The code rules data, shared with the build (`bim.coderules`).
const CODERULES: &str = include_str!("coderules.json");

// ---------------------------------------------------------------- small helpers

/// Lower-case words (letters and digits).
fn tokens(s: &str) -> Vec<String> {
    s.split(|c: char| !c.is_ascii_alphanumeric()).filter(|w| !w.is_empty()).take(MAX_WORDS).map(str::to_ascii_lowercase).collect()
}

/// `text` cut for a citation.
fn cite(text: &str) -> String {
    let t = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if t.chars().count() > MAX_CITE { format!("{}…", t.chars().take(MAX_CITE).collect::<String>()) } else { t }
}

/// A key compared without case, spaces or punctuation.
fn key_of(k: &str) -> String {
    k.chars().filter(char::is_ascii_alphanumeric).take(80).collect::<String>().to_ascii_lowercase()
}

/// A type mark compared without case, spaces or punctuation.
fn mark_key(m: &str) -> Option<String> {
    let k: String = m.chars().filter(char::is_ascii_alphanumeric).take(61).collect::<String>().to_ascii_uppercase();
    (!k.is_empty() && k.len() <= 60).then_some(k)
}

/// A yes/no a property's text says.
fn flag(text: &str) -> Option<bool> {
    match text.trim().to_ascii_lowercase().as_str() {
        "true" | "yes" | "y" | "1" => Some(true),
        "false" | "no" | "n" | "0" | "none" => Some(false),
        _ => None,
    }
}

/// A text value of a JSON field (a string, a number, or a list joined).
fn text_of(v: Option<&Value>) -> Option<String> {
    let t = match v? {
        Value::String(s) => s.trim().to_string(),
        Value::Number(n) => n.to_string(),
        Value::Array(a) => a.iter().take(60).filter_map(Value::as_str).map(str::trim).filter(|s| !s.is_empty()).collect::<Vec<_>>().join(", "),
        _ => return None,
    };
    (!t.is_empty() && t.len() <= 2_000).then_some(t)
}

/// A JSON list of texts (or one comma text).
fn list_of(v: Option<&Value>) -> Option<Vec<String>> {
    let out: Vec<String> = match v? {
        Value::Array(a) => a.iter().take(100).filter_map(Value::as_str).map(|s| s.trim().chars().take(100).collect()).collect(),
        Value::String(s) => s.split([',', ';']).take(100).map(|x| x.trim().chars().take(100).collect()).collect(),
        _ => return None,
    };
    let out: Vec<String> = out.into_iter().filter(|s: &String| !s.is_empty()).collect();
    (!out.is_empty()).then_some(out)
}

/// A property's source in words: its kind and its document.
fn source_words(src: &cadaddy_doc::props::Source) -> String {
    let kind = format!("{:?}", src.kind).to_ascii_lowercase();
    match src.doc.as_deref().map(str::trim).filter(|d| !d.is_empty()) {
        Some(doc) => format!("{kind}: {}", cite(doc)),
        None => kind,
    }
}

// ---------------------------------------------------------------- comparing values

/// Colour abbreviations a catalog or schedule uses, and the colour they stand for.
const COLOURS: [(&str, &str); 12] = [
    ("wh", "white"),
    ("wht", "white"),
    ("iv", "ivory"),
    ("ivy", "ivory"),
    ("gy", "gray"),
    ("gry", "gray"),
    ("grey", "gray"),
    ("bk", "black"),
    ("blk", "black"),
    ("br", "brown"),
    ("brn", "brown"),
    ("rd", "red"),
];

/// A colour text's words with abbreviations spelt out.
fn colour_words(text: &str) -> BTreeSet<String> {
    tokens(text).into_iter().map(|w| COLOURS.iter().find(|(a, _)| *a == w).map_or(w, |(_, c)| (*c).to_string())).collect()
}

/// Amps and volts a rating text gives (`20A 125V`, `20 AMP, 125/250 V`, `5-20R`).
fn amps_volts(text: &str) -> (BTreeSet<u32>, BTreeSet<u32>) {
    let mut amps = BTreeSet::new();
    let mut volts = BTreeSet::new();
    let low = text.to_ascii_lowercase();
    // A chunk (`125/250V`): its numbers take the unit the chunk ends with, or the next word.
    let chunks: Vec<&str> = low.split(|c: char| c.is_whitespace() || c == ',' || c == ';').filter(|c| !c.is_empty()).take(MAX_WORDS).collect();
    for (i, c) in chunks.iter().enumerate() {
        let nums: Vec<u32> = c
            .split(|ch: char| !ch.is_ascii_digit() && ch != '.')
            .filter_map(|n| n.trim_matches('.').parse::<f64>().ok())
            .filter(|n| n.is_finite() && *n > 0.0 && *n < 100_000.0)
            .map(|n| n.round() as u32)
            .collect();
        if nums.is_empty() {
            continue;
        }
        let tail: String = c.chars().rev().take_while(|ch| ch.is_ascii_alphabetic()).collect::<Vec<_>>().into_iter().rev().collect();
        let unit =
            if tail.is_empty() { chunks.get(i + 1).map(|n| n.trim_matches(|ch: char| !ch.is_ascii_alphabetic()).to_string()) } else { Some(tail) };
        match unit.as_deref() {
            Some("a" | "amp" | "amps" | "ampere" | "amperes") => amps.extend(nums),
            Some("v" | "volt" | "volts" | "vac") => volts.extend(nums),
            // A NEMA configuration (`5-20R`): 5- is 125 V, 6- 250 V, 14- 125/250 V.
            Some("r" | "p") if c.contains('-') => {
                let mut it = nums.iter();
                if let (Some(n), Some(a)) = (it.next(), it.next()) {
                    amps.insert(*a);
                    match n {
                        5 => {
                            volts.insert(125);
                        }
                        6 => {
                            volts.insert(250);
                        }
                        14 => {
                            volts.extend([125, 250]);
                        }
                        _ => {}
                    }
                }
            }
            _ => {}
        }
    }
    (amps, volts)
}

/// Do a model's value and a document's value of `field` agree? None: they can't be compared.
pub fn same(field: &str, model: &str, doc: &str) -> Option<bool> {
    match field {
        "maker" => agree("manufacturer", model, doc),
        "catalog" => agree("catalog", model, doc),
        "color" => {
            let (a, b) = (colour_words(model), colour_words(doc));
            if a.is_empty() || b.is_empty() {
                return None;
            }
            // The model's colour words all in the document's (`white` in "white unless otherwise
            // indicated"), or the document's in the model's.
            Some(a.is_subset(&b) || b.is_subset(&a) || a.iter().any(|w| b.contains(w) && w.len() > 2))
        }
        "rating" => {
            let ((ma, mv), (da, dv)) = (amps_volts(model), amps_volts(doc));
            let mut said = None;
            if !ma.is_empty() && !da.is_empty() {
                said = Some(!ma.is_disjoint(&da));
            }
            if !mv.is_empty() && !dv.is_empty() {
                said = Some(said.unwrap_or(true) && !mv.is_disjoint(&dv));
            }
            said.or_else(|| agree("other", model, doc))
        }
        _ => agree("other", model, doc),
    }
}

/// The features a text names, canonical: `tamper_resistant`, `gfci`, `weather_resistant`, `usb`,
/// `hospital_grade`, `self_test`; any other the text's letters and digits.
pub fn feature_keys(text: &str) -> BTreeSet<String> {
    let w = tokens(text);
    let has = |x: &str| w.iter().any(|t| t == x);
    let pair = |a: &str, b: &str| w.windows(2).any(|p| matches!(p, [x, y] if x == a && y.starts_with(b)));
    let mut out = BTreeSet::new();
    if super::validate_tr::says_tr(text, true) == Some(true) {
        out.insert("tamper_resistant".to_string());
    }
    if has("gfci") || has("gfi") || has("gf") || pair("ground", "fault") {
        out.insert("gfci".to_string());
    }
    if has("wr") || has("wrtr") || has("trwr") || pair("weather", "resist") {
        out.insert("weather_resistant".to_string());
    }
    if has("usb") {
        out.insert("usb".to_string());
    }
    if pair("hospital", "grade") {
        out.insert("hospital_grade".to_string());
    }
    if has("selftest") || pair("self", "test") {
        out.insert("self_test".to_string());
    }
    if out.is_empty() {
        let k = key_of(text);
        if !k.is_empty() {
            out.insert(k);
        }
    }
    out
}

// ---------------------------------------------------------------- the model's product

/// What a model field holds: the value and where it was read.
#[derive(Clone, Debug, PartialEq)]
pub struct Said {
    pub value: String,
    pub from: String,
}

/// The names an element's properties give each product field.
const MODEL_KEYS: [(&str, &[&str]); 5] = [
    ("maker", &["manufacturer", "maker", "mfr", "mfg", "brand"]),
    ("catalog", &["catalognumber", "catalog", "catalogno", "catno", "modelnumber"]),
    ("color", &["color", "colour", "finish", "devicecolor"]),
    ("configuration", &["configuration", "config"]),
    ("rating", &["rating", "nema", "nemaconfiguration"]),
];

/// An element's own product: its properties, its source object's, its product record, its type's
/// fields.
pub(crate) struct Own<'a> {
    props: Props,
    source: Props,
    pub record: Option<Value>,
    fields: Option<&'a BTreeMap<String, String>>,
    kind: String,
    mark: Option<String>,
    name: Option<String>,
}

impl<'a> Own<'a> {
    pub(crate) fn of(d: &Drawing, it: &Lit<'a>) -> Own<'a> {
        let props = it.entity.map(|e| cadaddy_doc::props::read(&e.common.xdata)).unwrap_or_default();
        let source = it
            .entity
            .and_then(super::bimlook::made_source)
            .and_then(|h| d.model.get(h))
            .map(|s| cadaddy_doc::props::read(&s.common.xdata))
            .unwrap_or_default();
        let record = super::validate_tr::product_of(it.entity, it.fields);
        let mark = it
            .mark
            .map(str::to_string)
            .or_else(|| [&props, &source].iter().find_map(|p| p.get("type_mark").filter(|x| x.has_value()).map(|x| x.text().trim().to_string())));
        Own { props, source, record, fields: it.fields, kind: it.kind.to_string(), mark, name: it.name.map(str::to_string) }
    }

    /// The first property of `keys` with a value (its own, then its source object's).
    fn prop(&self, keys: &[&str]) -> Option<Said> {
        for p in [&self.props, &self.source] {
            for (k, prop) in p.iter().take(400) {
                if keys.contains(&key_of(k).as_str()) && prop.has_value() {
                    let v = prop.text().trim().to_string();
                    if !v.is_empty() {
                        return Some(Said { value: v, from: format!("its {k} property ({})", source_words(&prop.source)) });
                    }
                }
            }
        }
        None
    }

    fn resolved(&self) -> Option<&Value> {
        self.record.as_ref().and_then(|r| r.get("resolved")).filter(|r| r.is_object())
    }

    /// The model's value of a product field.
    pub(crate) fn value(&self, field: &str) -> Option<Said> {
        let keys = MODEL_KEYS.iter().find(|(f, _)| *f == field).map(|(_, k)| *k)?;
        if let Some(s) = self.prop(keys) {
            return Some(s);
        }
        if field == "rating" {
            let a = self.prop(&["amps", "amperage", "ampacity"]);
            let v = self.prop(&["volts", "voltage"]);
            if a.is_some() || v.is_some() {
                let value = [
                    a.as_ref().map(|x| format!("{} A", x.value.trim_end_matches(['A', 'a']).trim())),
                    v.as_ref().map(|x| format!("{} V", x.value.trim_end_matches(['V', 'v']).trim())),
                ]
                .into_iter()
                .flatten()
                .collect::<Vec<_>>()
                .join(" ");
                let from = [a, v].into_iter().flatten().map(|x| x.from).collect::<Vec<_>>().join("; ");
                return Some(Said { value, from });
            }
        }
        let rkey = match field {
            "maker" => "maker",
            "catalog" => "catalog",
            "color" => "color",
            "configuration" => "configuration",
            _ => "rating",
        };
        if let Some(v) = text_of(self.resolved().and_then(|r| r.get(rkey))) {
            return Some(Said { value: v, from: format!("its product record (resolved {rkey})") });
        }
        self.fields
            .into_iter()
            .flatten()
            .take(400)
            .find(|(k, v)| keys.contains(&key_of(k).as_str()) && !v.trim().is_empty())
            .map(|(k, v)| Said { value: v.trim().to_string(), from: format!("its type's {k}") })
    }

    /// The features the model's product has: its record's resolved features, its `features`
    /// property, and its flags (tamper-resistant, weather-resistant, a GFCI receptacle type).
    pub(crate) fn features(&self) -> Option<(BTreeSet<String>, String)> {
        let mut keys = BTreeSet::new();
        let mut from: Vec<String> = Vec::new();
        if let Some(list) = list_of(self.resolved().and_then(|r| r.get("features"))) {
            for f in &list {
                keys.extend(feature_keys(f));
            }
            from.push(format!("its product record's features ({})", cite(&list.join(", "))));
        }
        if let Some(s) = self.prop(&["features"]) {
            for f in s.value.split([',', ';']).take(100) {
                keys.extend(feature_keys(f));
            }
            from.push(s.from);
        }
        for (k, name) in [("tamperresistant", "tamper_resistant"), ("weatherresistant", "weather_resistant")] {
            if let Some(s) = self.prop(&[k]).filter(|s| flag(&s.value) == Some(true)) {
                keys.insert(name.to_string());
                from.push(s.from);
            }
        }
        if let Some(s) = self.prop(&["receptacletype"]).filter(|s| feature_keys(&s.value).contains("gfci")) {
            keys.insert("gfci".to_string());
            from.push(s.from);
        }
        (!from.is_empty()).then(|| (keys, from.join("; ")))
    }

    /// Does the model know anything of the element's product?
    pub(crate) fn any(&self) -> bool {
        FIELDS.iter().take(5).any(|f| self.value(f).is_some()) || self.features().is_some()
    }

    /// The element's own words: its mark, name, kind and type properties.
    fn words(&self) -> Vec<(String, String)> {
        let mut out: Vec<(String, String)> = Vec::new();
        if let Some(m) = &self.mark {
            out.push(("its mark".into(), m.clone()));
        }
        for k in ["type", "device_type", "description", "type_name", "configuration"] {
            if let Some(s) = self.prop(&[key_of(k).as_str()]) {
                out.push((s.from, s.value));
            }
        }
        if let Some(n) = &self.name {
            out.push(("its name".into(), n.clone()));
        }
        out
    }

    /// Is the model's product tamper-resistant? (yes / no / not said, and where).
    fn tamper_resistant(&self) -> (Option<bool>, String) {
        if let Some(r) = self.resolved() {
            match r.get("tamper_resistant") {
                Some(Value::Bool(b)) => return (Some(*b), "its product record (resolved tamper_resistant)".into()),
                Some(Value::String(s)) if flag(s).is_some() => return (flag(s), "its product record (resolved tamper_resistant)".into()),
                _ => {}
            }
        }
        if let Some(s) = self.prop(&["tamperresistant"])
            && let Some(b) = flag(&s.value).or_else(|| super::validate_tr::says_tr(&s.value, true))
        {
            return (Some(b), s.from);
        }
        if let Some(list) = list_of(self.resolved().and_then(|r| r.get("features"))) {
            let said: Vec<Option<bool>> = list.iter().map(|f| super::validate_tr::says_tr(f, true)).collect();
            let tr = !said.contains(&Some(false)) && said.contains(&Some(true));
            return (Some(tr), format!("its product record's features ({})", cite(&list.join(", "))));
        }
        for (from, text) in self.words() {
            if let Some(b) = super::validate_tr::says_tr(&text, true) {
                return (Some(b), format!("{from}: \"{}\"", cite(&text)));
            }
        }
        (None, "nothing in the model says whether it is tamper-resistant".into())
    }

    /// Is the model's receptacle a GFCI device?
    fn gfci(&self, tags: &[String]) -> (Option<bool>, String) {
        let tagged = |text: &str| -> bool {
            let w = tokens(text);
            tags.iter().any(|t| w.iter().any(|x| x.eq_ignore_ascii_case(t))) || feature_keys(text).contains("gfci")
        };
        if let Some(s) = self.prop(&["receptacletype"]) {
            let gf = tagged(&s.value);
            let standard = tokens(&s.value).iter().any(|w| w == "standard" || w == "duplex" || w == "normal");
            if gf || standard {
                return (Some(gf), format!("{}: \"{}\"", s.from, cite(&s.value)));
            }
        }
        if validate_kinds::in_family(&self.kind, "gfci_receptacle") {
            return (Some(true), format!("its kind ({})", self.kind));
        }
        if let Some(r) = self.resolved() {
            let config = text_of(r.get("configuration"));
            let feats = list_of(r.get("features"));
            if config.is_some() || feats.is_some() {
                let text = format!("{} {}", config.unwrap_or_default(), feats.unwrap_or_default().join(", "));
                return (Some(tagged(&text)), format!("its product record (resolved configuration and features: \"{}\")", cite(text.trim())));
            }
        }
        let words = self.words();
        if let Some((from, text)) = words.iter().find(|(_, t)| tagged(t)) {
            return (Some(true), format!("{from}: \"{}\"", cite(text)));
        }
        if let Some((from, text)) = words.first() {
            return (Some(false), format!("{from} (\"{}\") names no GFCI", cite(text)));
        }
        (None, "nothing in the model says whether it is a GFCI receptacle".into())
    }

    /// Is the model's receptacle weather-resistant?
    fn weather_resistant(&self) -> (Option<bool>, String) {
        if let Some(s) = self.prop(&["weatherresistant"])
            && let Some(b) = flag(&s.value)
        {
            return (Some(b), s.from);
        }
        if let Some((keys, from)) = self.features() {
            if keys.contains("weather_resistant") {
                return (Some(true), from);
            }
            if list_of(self.resolved().and_then(|r| r.get("features"))).is_some() || self.prop(&["features"]).is_some() {
                return (Some(false), format!("{from} (no weather-resistance)"));
            }
        }
        for (from, text) in self.words() {
            if feature_keys(&text).contains("weather_resistant") {
                return (Some(true), format!("{from}: \"{}\"", cite(&text)));
            }
        }
        (None, "nothing in the model says whether it is weather-resistant".into())
    }

    /// A property's text (its own, then its source object's).
    fn text(&self, key: &str) -> Option<String> {
        self.prop(&[key_of(key).as_str()]).map(|s| s.value)
    }
}

// ---------------------------------------------------------------- what the documents require

/// What the documents require of one field: any of `any_of` (all of them for features), none of
/// `not`, and where it says so.
#[derive(Clone, Debug, PartialEq)]
pub struct Want {
    pub any_of: Vec<String>,
    pub not: Vec<String>,
    /// `submittal`, `spec` or `spec_rule`.
    pub from: &'static str,
    pub source: String,
    pub note: Option<String>,
}

impl Want {
    fn words(&self, field: &str) -> String {
        let list = if field == "features" { self.any_of.join(" + ") } else { self.any_of.join(" or ") };
        let mut s = if list.is_empty() { String::new() } else { list };
        if !self.not.is_empty() {
            if !s.is_empty() {
                s.push_str("; ");
            }
            s.push_str(&format!("not {}", self.not.join(" or ")));
        }
        s
    }
    fn to_json(&self) -> Value {
        json!({"values": self.any_of, "not": self.not, "from": self.from, "source": self.source, "note": self.note})
    }
}

/// The field a document's key is, if one compared.
fn doc_field(key: &str) -> Option<&'static str> {
    Some(match key_of(key).as_str() {
        "manufacturer" | "maker" | "mfr" | "mfg" | "brand" | "approvedmakers" | "manufacturers" => "maker",
        "catalognumber" | "catalog" | "catno" | "catalogno" | "catalognum" | "modelnumber" => "catalog",
        "color" | "colour" | "finish" | "devicecolor" => "color",
        "configuration" | "config" => "configuration",
        "rating" | "amps" | "amperage" | "nema" => "rating",
        "features" | "feature" | "requiredfeatures" => "features",
        _ => return None,
    })
}

/// Is a BIM type field's source a submittal page (`p.32`, `submittal …`)?
fn submittal_source(s: &str) -> bool {
    let t = s.trim().to_ascii_lowercase();
    t.starts_with("p.") || t.starts_with("p ") || t.contains("submittal") || t.contains("cut sheet")
}

/// What the job's documents say of products, by type mark: the submittals' products and the
/// prints' spec rules.
pub(crate) struct Docs<'a> {
    /// mark → field → (value, document).
    submittal: HashMap<String, BTreeMap<&'static str, (String, String)>>,
    spec_rules: Vec<&'a SpecRule>,
}

impl<'a> Docs<'a> {
    pub(crate) fn of(d: &Drawing, prints: Option<&'a Prints>) -> Docs<'a> {
        let mut submittal: HashMap<String, BTreeMap<&'static str, (String, String)>> = HashMap::new();
        for s in prints.map_or(&[][..], |p| p.submittals.as_slice()).iter().take(MAX_ROWS) {
            let Some(m) = mark_key(&s.mark) else { continue };
            let doc = if s.section.trim().is_empty() {
                format!("submittal, type {}", s.mark.trim())
            } else {
                format!("submittal (spec {}), type {}", cite(&s.section), s.mark.trim())
            };
            let p = submittal.entry(m).or_default();
            for (k, v) in s.fields.iter().take(60) {
                if let Some(f) = doc_field(k).filter(|_| !v.trim().is_empty() && v.len() <= 400) {
                    p.entry(f).or_insert_with(|| (v.trim().to_string(), doc.clone()));
                }
            }
        }
        for t in d.bim_types.types.iter().take(MAX_ROWS) {
            let Some(m) = mark_key(&t.mark) else { continue };
            for (k, f) in t.fields.iter().take(60) {
                let Some(field) = doc_field(k).filter(|_| submittal_source(&f.source) && !f.value.trim().is_empty() && f.value.len() <= 400) else {
                    continue;
                };
                let doc = format!("the type catalog's {} {k} (from {})", t.mark.trim(), cite(&f.source));
                submittal.entry(m.clone()).or_default().entry(field).or_insert_with(|| (f.value.trim().to_string(), doc));
            }
        }
        submittal.retain(|_, p| !p.is_empty());
        let spec_rules =
            prints.map_or(&[][..], |p| p.rules.spec.as_slice()).iter().take(MAX_ROWS).filter(|r| doc_field(&r.field).is_some()).collect();
        Docs { submittal, spec_rules }
    }

    /// What the documents require of `field` for an element (its kind, its mark, its record), and
    /// whether it is outdoors.
    pub(crate) fn want(
        &self,
        field: &str,
        kind: &str,
        category: Option<&str>,
        mark: Option<&str>,
        record: Option<&Value>,
        outdoors: bool,
    ) -> Option<Want> {
        let sub = mark.and_then(mark_key).and_then(|m| self.submittal.get(&m));
        let from_sub = |f: &str| {
            sub.and_then(|s| s.get(f)).map(|(v, doc)| Want {
                any_of: vec![v.clone()],
                not: Vec::new(),
                from: "submittal",
                source: doc.clone(),
                note: None,
            })
        };
        let approved = record.and_then(|r| r.get("approved")).filter(|a| a.get("none").is_none());
        let from_approved = |key: &str| {
            let v = text_of(approved.and_then(|a| a.get(key)))?;
            let which = text_of(approved.and_then(|a| a.get("submittal"))).map(|s| format!(" ({})", cite(&s))).unwrap_or_default();
            Some(Want { any_of: vec![v], not: Vec::new(), from: "submittal", source: format!("the approved submittal{which}"), note: None })
        };
        let required = record.and_then(|r| r.get("required")).filter(|r| r.is_object());
        let spec_cite = || {
            let r = required?;
            let sec = text_of(r.get("section"))?;
            let page = text_of(r.get("page")).map(|p| format!(" p.{p}")).unwrap_or_default();
            let product = text_of(r.get("product")).map(|p| format!(" ({})", cite(&p))).unwrap_or_default();
            Some(format!("spec {sec}{page}{product}"))
        };
        let from_spec = |key: &str| -> Option<Want> {
            let r = required?;
            let any_of = if key == "makers" || key == "features" { list_of(r.get(key))? } else { vec![text_of(r.get(key))?] };
            let note = match key {
                "makers" => text_of(r.get("substitution")),
                "color" => text_of(r.get("color_rule")),
                _ => None,
            };
            Some(Want { any_of, not: Vec::new(), from: "spec", source: spec_cite().unwrap_or_else(|| "the specification".into()), note })
        };
        let from_rules = || -> Option<Want> {
            let r = self.spec_rules.iter().find(|r| {
                doc_field(&r.field) == Some(field)
                    && (!r.outdoors || outdoors)
                    && (r.kind.as_deref().is_some_and(|k| validate_kinds::kind_matches(k, kind))
                        || r.category.as_deref().zip(category).is_some_and(|(a, b)| a.eq_ignore_ascii_case(b.trim())))
            })?;
            let any_of: Vec<String> = r.allowed.iter().take(40).map(|x| x.trim().to_string()).filter(|x| !x.is_empty()).collect();
            let not: Vec<String> = r.forbidden.iter().take(40).map(|x| x.trim().to_string()).filter(|x| !x.is_empty()).collect();
            if any_of.is_empty() && not.is_empty() {
                return None;
            }
            let text = if r.text.trim().is_empty() { String::new() } else { format!(": {}", cite(&r.text)) };
            Some(Want { any_of, not, from: "spec_rule", source: format!("spec {}{text}", cite(&r.section)), note: None })
        };
        match field {
            // The approved list: the type's submittal first, else the specification's makers.
            "maker" => from_sub("maker").or_else(|| from_approved("maker")).or_else(|| from_spec("makers")).or_else(from_rules),
            "catalog" => from_sub("catalog").or_else(|| from_approved("catalog")).or_else(from_rules),
            "color" => from_spec("color").or_else(from_rules).or_else(|| from_sub("color")),
            "configuration" => from_spec("configuration").or_else(from_rules).or_else(|| from_sub("configuration")),
            "rating" => from_spec("rating").or_else(from_rules).or_else(|| from_sub("rating")),
            "features" => from_spec("features").or_else(from_rules),
            _ => None,
        }
    }
}

/// The verdict of a model value against a requirement: pass, fail, or unsure (can't compare).
fn judge(field: &str, model: &str, want: &Want) -> Option<bool> {
    if want.not.iter().any(|n| same(field, model, n) == Some(true)) {
        return Some(false);
    }
    if want.any_of.is_empty() {
        return Some(true);
    }
    let said: Vec<Option<bool>> = want.any_of.iter().map(|w| same(field, model, w)).collect();
    if said.contains(&Some(true)) {
        Some(true)
    } else if said.iter().all(|s| *s == Some(false)) {
        Some(false)
    } else {
        None
    }
}

/// The required features the model's features leave out.
fn missing_features(model: &BTreeSet<String>, want: &Want) -> Vec<String> {
    want.any_of.iter().filter(|f| !feature_keys(f).iter().all(|k| model.contains(k))).cloned().collect()
}

// ---------------------------------------------------------------- the counts

/// Verdict counts of one field.
#[derive(Clone, Debug, Default)]
struct Counts {
    pass: u64,
    fail: u64,
    unsure: u64,
}

impl Counts {
    fn add(&mut self, verdict: &str) {
        match verdict {
            "pass" => self.pass += 1,
            "fail" => self.fail += 1,
            _ => self.unsure += 1,
        }
    }
    fn to_json(&self) -> Value {
        let judged = self.pass + self.fail;
        let accuracy = if judged == 0 { Value::Null } else { json!(((self.pass as f64 / judged as f64) * 1000.0).round() / 1000.0) };
        json!({"pass": self.pass, "fail": self.fail, "unsure": self.unsure, "accuracy": accuracy})
    }
}

/// One kind's elements and their field verdicts.
#[derive(Clone, Debug, Default)]
struct KindCounts {
    elements: u64,
    with_data: u64,
    no_data: u64,
    fields: BTreeMap<String, Counts>,
}

/// The product counts: per kind and per field (G-PRODUCT's fields and G-PRODUCT-CODE's features).
#[derive(Clone, Debug, Default)]
pub(crate) struct Tally {
    kinds: BTreeMap<String, KindCounts>,
    fields: BTreeMap<String, Counts>,
}

impl Tally {
    fn element(&mut self, kind: &str, has_data: bool) {
        let k = self.kinds.entry(kind.to_string()).or_default();
        k.elements += 1;
        if has_data {
            k.with_data += 1;
        } else {
            k.no_data += 1;
        }
    }
    fn add(&mut self, kind: &str, field: &str, verdict: &str) {
        self.kinds.entry(kind.to_string()).or_default().fields.entry(field.to_string()).or_default().add(verdict);
        self.fields.entry(field.to_string()).or_default().add(verdict);
    }
    pub(crate) fn is_empty(&self) -> bool {
        self.kinds.is_empty()
    }
    pub(crate) fn to_json(&self) -> Value {
        let mut kinds: Vec<(&String, &KindCounts)> = self.kinds.iter().collect();
        kinds.sort_by(|a, b| b.1.elements.cmp(&a.1.elements).then_with(|| a.0.cmp(b.0)));
        let by_kind: serde_json::Map<String, Value> = kinds
            .into_iter()
            .take(MAX_KINDS)
            .map(|(k, c)| {
                let mut all = Counts::default();
                for f in c.fields.values() {
                    all.pass += f.pass;
                    all.fail += f.fail;
                    all.unsure += f.unsure;
                }
                let fields: serde_json::Map<String, Value> = c.fields.iter().map(|(f, n)| (f.clone(), n.to_json())).collect();
                let a = all.to_json();
                (
                    k.clone(),
                    json!({"elements": c.elements, "with_product_data": c.with_data, "no_product_data": c.no_data, "by_field": fields, "accuracy": a["accuracy"]}),
                )
            })
            .collect();
        let by_field: serde_json::Map<String, Value> = self.fields.iter().map(|(f, n)| (f.clone(), n.to_json())).collect();
        json!({
            "about": "G-PRODUCT field by field and G-PRODUCT-CODE feature by feature: per kind and per field, pass / fail / unsure (unsure: no product data, or nothing to compare) and accuracy = pass / (pass + fail)",
            "by_kind": by_kind,
            "by_field": by_field,
        })
    }
}

// ---------------------------------------------------------------- the code rules

/// The code rules G-PRODUCT-CODE reads, from the shared data.
pub(crate) struct Code {
    tr: Option<CodeRule>,
    gf: Option<CodeRule>,
    wr: Option<CodeRule>,
    /// The ratings the tamper-resistance rule covers, at most (amps, volts).
    tr_max: (Option<f64>, Option<f64>),
}

impl Code {
    pub(crate) fn load() -> Code {
        let all = cr::rules();
        let by = |sets: &str| all.iter().find(|r| r.sets == sets).cloned();
        let raw: Value = serde_json::from_str(CODERULES).unwrap_or(Value::Null);
        let tr_raw = raw
            .get("rules")
            .and_then(Value::as_array)
            .and_then(|a| a.iter().find(|r| r.get("sets").and_then(Value::as_str) == Some("tamper_resistant")));
        let num = |k: &str| tr_raw.and_then(|r| r.get(k)).and_then(Value::as_f64).filter(|x| x.is_finite() && *x > 0.0);
        Code { tr: by("tamper_resistant"), gf: by("gfci"), wr: by("weather_resistant"), tr_max: (num("max_amps"), num("max_volts")) }
    }

    /// The rule's text with its section, cited in a finding.
    fn cited(r: &CodeRule) -> Value {
        json!({"id": r.id, "cite": r.cite, "rule": r.rule})
    }
}

/// Does a rule list `kind` among its kinds (or a family that holds it)?
fn rule_kind(r: &CodeRule, kind: &str) -> bool {
    r.kinds.iter().any(|k| validate_kinds::kind_matches(k, kind) || k.eq_ignore_ascii_case(kind))
}

/// Why the tamper-resistance rule covers a room: a word of its name, or a typical unit.
fn tr_room(r: &CodeRule, room: &Area, typical: Option<&str>) -> Option<String> {
    let name = room.name.as_deref().unwrap_or_default().to_ascii_uppercase();
    let words: Vec<&str> = name.split(|c: char| !c.is_ascii_alphanumeric()).filter(|w| !w.is_empty()).collect();
    let label = room_label(room);
    if let Some(w) = r.rooms.name_words.iter().find(|w| words.iter().any(|x| x.starts_with(w.as_str()))) {
        return Some(format!("{label} is a guest room or dwelling by its name ({w})"));
    }
    if r.rooms.unit_rooms
        && let Some(t) = typical
    {
        return Some(format!("{label} is a guest room (typical unit {})", cite(t)));
    }
    None
}

fn room_label(a: &Area) -> String {
    match (&a.number, &a.name) {
        (Some(n), Some(name)) => format!("{name} {n}"),
        (Some(n), None) => format!("room {n}"),
        (None, Some(name)) => name.clone(),
        (None, None) => a.id.clone(),
    }
}

/// A circuit's breaker as the panel schedule gives it.
#[derive(Clone, Debug)]
enum Breaker {
    /// GFCI or not, and the schedule row cited.
    Known(bool, String),
    /// Why it isn't known.
    Unknown(String),
}

/// The schedule row of a circuit: its breaker, and its description.
fn breaker_of(sp: &BuildingSpec, panel: Option<&str>, circuit: Option<&str>) -> (Breaker, Option<String>) {
    let (Some(panel), Some(circuit)) = (panel.map(str::trim).filter(|p| !p.is_empty()), circuit.map(str::trim).filter(|c| !c.is_empty())) else {
        return (Breaker::Unknown("the model gives it no panel and circuit".into()), None);
    };
    // `G-12`, `G/12`, `12`: the number after the panel.
    let number = circuit.rsplit_once(['-', '/', '.']).map_or(circuit, |(_, n)| n).trim();
    let Some(ps) = sp.mep.panel_schedules.iter().take(10_000).find(|p| p.panel.trim().eq_ignore_ascii_case(panel)) else {
        return (Breaker::Unknown(format!("panel {panel} has no schedule in the documents read")), None);
    };
    let Some(row) = ps.circuits.iter().take(2_000).find(|c| c.number.trim() == number) else {
        return (Breaker::Unknown(format!("circuit {number} is not on panel {panel}'s schedule")), None);
    };
    let desc = Some(row.description.clone()).filter(|d| !d.trim().is_empty());
    // A schedule that marks breaker types and none on this row says a standard breaker; one that
    // marks none says nothing.
    if !ps.circuits.iter().take(2_000).any(|c| !c.protection.is_empty()) {
        return (Breaker::Unknown(format!("panel {panel}'s schedule marks no breaker types")), desc);
    }
    let (gf, _) = super::panelsched::protection(row, &ps.legend);
    let codes = if row.protection.is_empty() { "no type code".to_string() } else { row.protection.join("/") };
    let on = if ps.source.trim().is_empty() { String::new() } else { format!(" on {}", cite(&ps.source)) };
    let what = if gf { "a GFCI breaker" } else { "not a GFCI breaker" };
    (Breaker::Known(gf, format!("panel {panel} schedule{on}, circuit {} ({codes}: {what})", row.number.trim())), desc)
}

// ---------------------------------------------------------------- the screen

/// What the product rules found.
#[derive(Default)]
pub(crate) struct Found {
    /// (rank, finding).
    pub findings: Vec<(u8, Value)>,
    pub checked: BTreeMap<String, BTreeMap<String, u64>>,
    pub unmeasured: BTreeMap<String, BTreeMap<String, u64>>,
    pub tally: Tally,
}

impl Found {
    fn tally(&mut self, rule: &str, verdict: &str) {
        *self.checked.entry(rule.to_string()).or_default().entry(verdict.to_string()).or_default() += 1;
    }
    fn skip(&mut self, rule: &str, why: &str) {
        *self.unmeasured.entry(rule.to_string()).or_default().entry(why.to_string()).or_default() += 1;
    }
}

/// The inputs of the product screen.
pub(crate) struct Input<'a> {
    pub d: &'a Drawing,
    pub sp: &'a BuildingSpec,
    pub prints: Option<&'a Prints>,
    pub rules: &'a [Effective],
    /// Only these rules (empty: all).
    pub only: &'a [String],
    pub units: &'a Units,
}

fn rule_on<'r>(inp: &'r Input, id: &str) -> Option<&'r Effective> {
    inp.rules.iter().find(|r| r.rule.id == id && r.enabled).filter(|_| inp.only.is_empty() || inp.only.iter().any(|o| o == id))
}

fn rank(verdict: &str, severity: &str) -> u8 {
    match verdict {
        "pass" => 9,
        "unsure" => 5,
        _ => match severity {
            "high" => 0,
            "medium" => 1,
            _ => 2,
        },
    }
}

#[allow(clippy::too_many_arguments)]
fn finding(r: &Effective, it: &Lit, check: &str, verdict: &str, issue: String, expected: String, found: String, measured: Value) -> (u8, Value) {
    let id = format!("PS-{}", super::validate_run::hash8(&format!("{}|{}|{check}|{verdict}", r.rule.id, it.handle)));
    let f = json!({
        "id": id,
        "rule": r.rule.id,
        "check": check,
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

/// A receptacle's GFCI facts, read before the circuits are judged together.
struct Gf<'a> {
    it: &'a Lit<'a>,
    /// Its device is GFCI (yes / no / not said) and where that was read.
    device: (Option<bool>, String),
    breaker: Breaker,
    /// Why its location needs GFCI, if it does.
    needs: Option<String>,
    /// Its circuit: the panel element (else the panel tag and its room or level) and the number.
    circuit: Option<(String, String)>,
    /// Its panel's place on the plan, when the model has the panel.
    panel_at: Option<[f64; 2]>,
}

/// Run the product rules over `items` (every element in scope, with its room).
pub(crate) fn screen(inp: &Input, items: &[Lit]) -> Found {
    let mut out = Found::default();
    for (id, verdicts) in VERDICTS {
        if rule_on(inp, id).is_some() {
            out.checked.insert(id.to_string(), verdicts.iter().map(|v| (v.to_string(), 0)).collect());
        }
    }
    let (fields_rule, code_rule) = (rule_on(inp, RULE), rule_on(inp, CODE));
    if fields_rule.is_none() && code_rule.is_none() {
        return out;
    }
    let docs = Docs::of(inp.d, inp.prints);
    let exterior: BTreeSet<(String, String)> = inp
        .prints
        .map_or(&[][..], |p| p.rooms.as_slice())
        .iter()
        .take(MAX_ROWS)
        .filter(|r| r.exterior)
        .map(|r| (r.level.trim().to_ascii_lowercase(), r.number.trim().to_ascii_lowercase()))
        .collect();
    let print_units: HashMap<(String, String), &str> = inp
        .prints
        .map_or(&[][..], |p| p.rooms.as_slice())
        .iter()
        .take(MAX_ROWS)
        .filter_map(|pr| Some(((pr.level.trim().to_ascii_lowercase(), pr.number.trim().to_ascii_lowercase()), pr.unit_type.as_deref()?)))
        .collect();
    let outdoors = |a: Option<&Area>| {
        a.is_some_and(|a| {
            a.number.as_deref().is_some_and(|n| exterior.contains(&(a.level.trim().to_ascii_lowercase(), n.trim().to_ascii_lowercase())))
        })
    };
    // G-PRODUCT: field by field.
    if let Some(r) = fields_rule {
        for it in items.iter().filter(|it| r.rule.applies_to_element(it.kind, it.category, it.mark)) {
            let own = Own::of(inp.d, it);
            let has = own.any();
            out.tally.element(it.kind, has);
            let mark = own.mark.clone();
            let wants: Vec<(&str, Option<Want>)> =
                FIELDS.iter().map(|f| (*f, docs.want(f, it.kind, it.category, mark.as_deref(), own.record.as_ref(), outdoors(it.room)))).collect();
            let kind_words = it.kind.replace('_', " ");
            if !has {
                // Nothing of its product in the model: one finding, every required field unsure.
                let required: Vec<Value> =
                    wants.iter().filter_map(|(f, w)| w.as_ref().map(|w| json!({"field": f, "requires": w.to_json()}))).collect();
                for (f, w) in &wants {
                    if w.is_some() {
                        out.tally.add(it.kind, f, "unsure");
                    }
                }
                out.tally(&r.rule.id, "unsure");
                out.skip(&r.rule.id, &format!("{}: {NO_DATA}", it.kind));
                let expected = if required.is_empty() {
                    "its product as the submittals and the specification give it".to_string()
                } else {
                    wants.iter().filter_map(|(f, w)| w.as_ref().map(|w| format!("{f}: {}", w.words(f)))).collect::<Vec<_>>().join("; ")
                };
                out.findings.push(finding(
                    r,
                    it,
                    "record",
                    "unsure",
                    format!("{NO_DATA}: the model records no maker, catalog number, colour, configuration, rating or features for this {kind_words}"),
                    cite(&expected),
                    NO_DATA.into(),
                    json!({"field": "record", "requires": required}),
                ));
                continue;
            }
            for (field, want) in wants {
                let Some(want) = want else {
                    out.skip(&r.rule.id, &format!("{field}: no requirement in the submittals or the specification"));
                    continue;
                };
                let (verdict, issue, found, model_json) = if field == "features" {
                    match own.features() {
                        None => {
                            out.skip(&r.rule.id, &format!("{}: {NO_DATA} ({field})", it.kind));
                            (
                                "unsure",
                                format!(
                                    "{NO_DATA}: the model gives no features for this {kind_words}; {} requires {}",
                                    want.source,
                                    want.words(field)
                                ),
                                NO_DATA.to_string(),
                                Value::Null,
                            )
                        }
                        Some((keys, from)) => {
                            let missing = missing_features(&keys, &want);
                            let have = keys.iter().cloned().collect::<Vec<_>>().join(", ");
                            if missing.is_empty() {
                                (
                                    "pass",
                                    format!("{kind_words} has the features {} requires ({})", want.source, want.words(field)),
                                    have.clone(),
                                    json!({"value": have, "from": from}),
                                )
                            } else {
                                (
                                    "fail",
                                    format!("{kind_words} lacks {} that {} requires ({from})", missing.join(", "), want.source),
                                    if have.is_empty() { "none".into() } else { have.clone() },
                                    json!({"value": have, "from": from, "missing": missing}),
                                )
                            }
                        }
                    }
                } else {
                    match own.value(field) {
                        None => {
                            out.skip(&r.rule.id, &format!("{}: {NO_DATA} ({field})", it.kind));
                            (
                                "unsure",
                                format!("{NO_DATA}: the model has no {field} for this {kind_words}; {} gives {}", want.source, want.words(field)),
                                NO_DATA.to_string(),
                                Value::Null,
                            )
                        }
                        Some(m) => {
                            let v = judge(field, &m.value, &want);
                            let mj = json!({"value": m.value, "from": m.from});
                            match v {
                                Some(true) => {
                                    ("pass", format!("{kind_words} {field} {} is what {} gives", cite(&m.value), want.source), m.value.clone(), mj)
                                }
                                Some(false) => {
                                    let what = if field == "maker" && want.from != "submittal" {
                                        format!("{} is not an approved maker: {} lists {}", cite(&m.value), want.source, want.words(field))
                                    } else {
                                        format!(
                                            "{kind_words} {field} is {} ({}), but {} gives {}",
                                            cite(&m.value),
                                            m.from,
                                            want.source,
                                            want.words(field)
                                        )
                                    };
                                    ("fail", what, m.value.clone(), mj)
                                }
                                None => (
                                    "unsure",
                                    format!("{kind_words} {field} {} can't be compared with {} ({})", cite(&m.value), want.words(field), want.source),
                                    m.value.clone(),
                                    mj,
                                ),
                            }
                        }
                    }
                };
                out.tally(&r.rule.id, verdict);
                out.tally.add(it.kind, field, verdict);
                let mut expected = format!("{} ({})", want.words(field), want.source);
                if let Some(n) = &want.note {
                    expected.push_str(&format!("; {}", cite(n)));
                }
                let measured = json!({"field": field, "model": model_json, "requires": want.to_json()});
                out.findings.push(finding(r, it, field, verdict, issue, cite(&expected), cite(&found), measured));
            }
        }
    }
    // G-PRODUCT-CODE: the features the code requires where each receptacle is.
    if let Some(r) = code_rule {
        let code = Code::load();
        code_checks(inp, r, &code, items, &outdoors, &print_units, &mut out);
    }
    out
}

/// G-PRODUCT-CODE over the receptacles: tamper-resistance, GFCI by circuit, weather-resistance.
fn code_checks(
    inp: &Input,
    r: &Effective,
    code: &Code,
    items: &[Lit],
    outdoors: &dyn Fn(Option<&Area>) -> bool,
    print_units: &HashMap<(String, String), &str>,
    out: &mut Found,
) {
    let receptacle = |it: &Lit| r.rule.applies_to_element(it.kind, it.category, it.mark);
    let typical = |a: &Area| -> Option<String> {
        let n = a.number.as_deref()?;
        inp.units
            .get(&a.level, Some(n))
            .map(|u| u.name.clone())
            .or_else(|| print_units.get(&(a.level.trim().to_ascii_lowercase(), n.trim().to_ascii_lowercase())).map(|s| s.to_string()))
    };
    // Tamper-resistant receptacles where the code wants them.
    if let Some(tr) = &code.tr {
        for it in items.iter().filter(|it| receptacle(it) && rule_kind(tr, it.kind)) {
            let Some(room) = it.room else {
                out.skip(&r.rule.id, "tamper_resistant: not in a room (whether the code wants it is not known)");
                continue;
            };
            let Some(why) = tr_room(tr, room, typical(room).as_deref()) else { continue };
            let own = Own::of(inp.d, it);
            // A receptacle known to be rated above what the rule covers is left out.
            let scope = super::validate_rules::CodeScope {
                cite: tr.cite.clone(),
                says: tr.rule.clone(),
                rooms: Vec::new(),
                typical_units: false,
                max_amps: code.tr_max.0,
                max_volts: code.tr_max.1,
            };
            let subject = super::validate_tr::Subject { mark: it.mark, tag: None, name: it.name, fields: it.fields, entity: it.entity };
            if super::validate_tr::outside_ratings(&scope, &subject).is_some() {
                continue;
            }
            let (tr_is, from) = own.tamper_resistant();
            let verdict = match tr_is {
                Some(true) => "pass",
                Some(false) => "fail",
                None => "unsure",
            };
            if verdict == "unsure" {
                out.skip(&r.rule.id, &format!("{}: {NO_DATA} (tamper_resistant)", it.kind));
            }
            let kind_words = it.kind.replace('_', " ");
            let issue = match verdict {
                "pass" => format!("{kind_words} is tamper-resistant ({from}) as {} wants: {why}", tr.cite),
                "fail" => format!("{kind_words} is not tamper-resistant ({from}), but {} wants it so: {why}", tr.cite),
                _ => format!("{NO_DATA}: {from}; {} wants it tamper-resistant: {why}", tr.cite),
            };
            out.tally(&r.rule.id, verdict);
            out.tally.add(it.kind, "tamper_resistant", verdict);
            let found = match tr_is {
                Some(true) => "tamper-resistant",
                Some(false) => "not tamper-resistant",
                None => NO_DATA,
            };
            let measured =
                json!({"field": "tamper_resistant", "model": {"value": tr_is, "from": from}, "required_because": why, "code": Code::cited(tr)});
            out.findings.push(finding(r, it, "tamper_resistant", verdict, issue, format!("tamper-resistant ({})", tr.cite), found.into(), measured));
        }
    }
    // Weather-resistant receptacles outdoors.
    if let Some(wr) = &code.wr {
        for it in items.iter().filter(|it| receptacle(it) && rule_kind(wr, it.kind)) {
            let own = Own::of(inp.d, it);
            let (_, desc) = breaker_of(inp.sp, own.text("panel").as_deref(), own.text("circuit").as_deref());
            let why = if outdoors(it.room) {
                it.room.map(|a| format!("{} is an exterior room on the prints", room_label(a)))
            } else {
                desc.as_deref().and_then(|d| cr::place_of(&wr.places, d).map(|p| format!("{}: its circuit is {}", p.cite, cite(d)))).or_else(|| {
                    let a = it.room?;
                    let name = a.name.as_deref()?;
                    cr::place_of(&wr.places, name).map(|p| format!("{}: in {}", p.cite, room_label(a)))
                })
            };
            let Some(why) = why else { continue };
            let (is, from) = own.weather_resistant();
            let verdict = match is {
                Some(true) => "pass",
                Some(false) => "fail",
                None => "unsure",
            };
            if verdict == "unsure" {
                out.skip(&r.rule.id, &format!("{}: {NO_DATA} (weather_resistant)", it.kind));
            }
            let kind_words = it.kind.replace('_', " ");
            let issue = match verdict {
                "pass" => format!("{kind_words} is weather-resistant ({from}) as {} wants: {why}", wr.cite),
                "fail" => format!("{kind_words} is not weather-resistant ({from}), but {} wants it so: {why}", wr.cite),
                _ => format!("{NO_DATA}: {from}; {} wants it weather-resistant: {why}", wr.cite),
            };
            out.tally(&r.rule.id, verdict);
            out.tally.add(it.kind, "weather_resistant", verdict);
            let found = match is {
                Some(true) => "weather-resistant",
                Some(false) => "not weather-resistant",
                None => NO_DATA,
            };
            let measured =
                json!({"field": "weather_resistant", "model": {"value": is, "from": from}, "required_because": why, "code": Code::cited(wr)});
            out.findings.push(finding(
                r,
                it,
                "weather_resistant",
                verdict,
                issue,
                format!("weather-resistant ({})", wr.cite),
                found.into(),
                measured,
            ));
        }
    }
    if let Some(gf) = &code.gf {
        gfci_checks(inp, r, gf, items, out);
    }
}

/// GFCI judged on the whole circuit (see the module's words).
fn gfci_checks(inp: &Input, r: &Effective, gf: &CodeRule, items: &[Lit], out: &mut Found) {
    let receptacle = |it: &Lit| r.rule.applies_to_element(it.kind, it.category, it.mark) && rule_kind(gf, it.kind);
    // The sinks the model holds, by level: their words name a sink (a lavatory, a kitchenette).
    let sink_words: Vec<String> = gf.sink_words.iter().map(|w| w.to_ascii_lowercase()).collect();
    let sinks: Vec<(&str, [f64; 2], &str)> = items
        .iter()
        .filter(|it| validate_kinds::in_family(it.kind, "fixture") || it.kind.contains("sink"))
        .filter(|it| {
            let words = tokens(&format!("{} {} {}", it.kind.replace('_', " "), it.mark.unwrap_or_default(), it.name.unwrap_or_default()));
            words.iter().any(|w| sink_words.contains(w))
        })
        .map(|it| (it.level, it.at, it.handle))
        .collect();
    // Where each panel element is (by handle, and by its tag on a level).
    let panels: Vec<&Lit> = items.iter().filter(|it| validate_kinds::in_family(it.kind, "panel") || it.kind == "panel").collect();
    let at_of: HashMap<String, [f64; 2]> = items.iter().map(|it| (it.handle.to_ascii_uppercase(), it.at)).collect();
    let mut facts: Vec<Gf> = Vec::new();
    for it in items.iter().filter(|it| receptacle(it)) {
        let own = Own::of(inp.d, it);
        let device = own.gfci(&gf.tags);
        let panel = own.text("panel");
        let number = own.text("circuit");
        let (breaker, desc) = breaker_of(inp.sp, panel.as_deref(), number.as_deref());
        // Why the location needs GFCI: the plan tags it so, a sink is within reach, its circuit
        // or its room is a place the rules list.
        let tagged = own.words().into_iter().find(|(_, t)| {
            let w = tokens(t);
            gf.tags.iter().any(|g| w.iter().any(|x| x.eq_ignore_ascii_case(g)))
        });
        let near_sink = gf.sink_reach_ft.filter(|r| r.is_finite() && *r > 0.0).and_then(|reach| {
            sinks
                .iter()
                .filter(|(lvl, _, _)| lvl.eq_ignore_ascii_case(it.level))
                .map(|(_, p, h)| ((it.at[0] - p[0]).hypot(it.at[1] - p[1]), *h))
                .filter(|(dd, _)| dd.is_finite() && *dd <= reach + SINK_HALF_FT)
                .min_by(|a, b| a.0.total_cmp(&b.0))
                .map(|(dd, h)| format!("{dd:.1} ft from sink {h} (within {reach} ft of its edge, NEC 210.8)"))
        });
        let place =
            desc.as_deref().and_then(|d| cr::place_of(&gf.places, d).map(|p| format!("{}: its circuit is {}", p.cite, cite(d)))).or_else(|| {
                let a = it.room?;
                let name = a.name.as_deref()?;
                cr::place_of(&gf.places, name).map(|p| format!("{}: in {}", p.cite, room_label(a)))
            });
        let needs = tagged.map(|(from, t)| format!("the plan tags it GFCI ({from}: \"{}\")", cite(&t))).or(near_sink).or(place);
        // Its circuit: the panel element it is fed from (a computed wiring fact), else its panel
        // tag in its room (or on its level).
        let fed = own.text("fed_from_handle").map(|h| h.trim().to_ascii_uppercase()).filter(|h| !h.is_empty());
        let circuit = number.as_ref().filter(|n| !n.trim().is_empty()).map(|n| {
            let key = fed.clone().unwrap_or_else(|| {
                format!(
                    "{}@{}",
                    panel.as_deref().unwrap_or_default().trim().to_ascii_uppercase(),
                    it.room.and_then(|a| a.number.clone()).unwrap_or_else(|| it.level.to_ascii_lowercase())
                )
            });
            (key, n.trim().to_ascii_uppercase())
        });
        let panel_at = fed.as_ref().and_then(|h| at_of.get(h).copied()).or_else(|| {
            let p = panel.as_deref()?.trim();
            panels
                .iter()
                .filter(|x| x.level.eq_ignore_ascii_case(it.level))
                .filter(|x| x.mark.is_some_and(|m| m.trim().eq_ignore_ascii_case(p)) || x.name.is_some_and(|n| n.trim().eq_ignore_ascii_case(p)))
                .map(|x| ((x.at[0] - it.at[0]).hypot(x.at[1] - it.at[1]), x.at))
                .filter(|(dd, _)| dd.is_finite())
                .min_by(|a, b| a.0.total_cmp(&b.0))
                .map(|(_, at)| at)
        });
        facts.push(Gf { it, device, breaker, needs, circuit, panel_at });
    }
    for (i, f) in facts.iter().enumerate() {
        let it = f.it;
        let kind_words = it.kind.replace('_', " ");
        let (dev, dev_from) = (&f.device.0, &f.device.1);
        let mut protector: Option<String> = None;
        let (verdict, issue, expected) = match &f.breaker {
            Breaker::Known(true, row) => match dev {
                Some(true) => (
                    "fail",
                    format!(
                        "double protection: {kind_words} is a GFCI device ({dev_from}) on a GFCI breaker ({row}); a standard receptacle belongs there"
                    ),
                    format!("a standard (non-GFCI) receptacle: its breaker protects it ({row})"),
                ),
                Some(false) => (
                    "pass",
                    format!("{kind_words} is a standard device ({dev_from}), protected by its GFCI breaker ({row})"),
                    format!("a standard receptacle on its GFCI breaker ({row})"),
                ),
                None => (
                    "unsure",
                    format!("{NO_DATA}: {dev_from}; its breaker is GFCI ({row}), so it should be a standard device"),
                    format!("a standard receptacle on its GFCI breaker ({row})"),
                ),
            },
            Breaker::Known(false, row) => {
                let Some(needs) = &f.needs else { continue };
                match dev {
                    Some(true) => (
                        "pass",
                        format!("{kind_words} is a GFCI device ({dev_from}) where GFCI is needed ({needs}); its breaker isn't GFCI ({row})"),
                        format!("GFCI protection ({needs})"),
                    ),
                    Some(false) => {
                        // On the load side of a GFCI receptacle of the same circuit, nearer its panel.
                        let mine = f.panel_at.map(|p| (it.at[0] - p[0]).hypot(it.at[1] - p[1]));
                        let same: Vec<&Gf> = facts
                            .iter()
                            .enumerate()
                            .filter(|(j, g)| *j != i && g.circuit.is_some() && g.circuit == f.circuit && g.device.0 == Some(true))
                            .map(|(_, g)| g)
                            .collect();
                        let up = match (mine, f.panel_at) {
                            (Some(d0), Some(p)) => same
                                .iter()
                                .map(|g| ((g.it.at[0] - p[0]).hypot(g.it.at[1] - p[1]), g))
                                .filter(|(dd, _)| dd.is_finite() && *dd < d0)
                                .min_by(|a, b| a.0.total_cmp(&b.0))
                                .map(|(_, g)| *g),
                            _ => None,
                        };
                        match (up, same.is_empty(), mine) {
                            (Some(g), _, _) => {
                                protector = Some(g.it.handle.to_string());
                                (
                                    "pass",
                                    format!(
                                        "{kind_words} is a standard device ({dev_from}) on the load side of GFCI receptacle {} on the same circuit, nearer its panel; GFCI is needed ({needs}); its breaker isn't GFCI ({row})",
                                        g.it.handle
                                    ),
                                    format!("GFCI protection ({needs})"),
                                )
                            }
                            (None, false, None) => (
                                "unsure",
                                format!(
                                    "{kind_words} is a standard device on a circuit with a GFCI receptacle, but the model has no place for its panel: which is first on the circuit is not known; GFCI is needed ({needs}); {row}"
                                ),
                                format!("GFCI protection ({needs})"),
                            ),
                            _ => (
                                "fail",
                                format!(
                                    "{kind_words} is a standard device ({dev_from}) where GFCI is needed ({needs}), its breaker isn't GFCI ({row}) and no GFCI receptacle upstream on its circuit protects it"
                                ),
                                format!("a GFCI receptacle, or one on the load side of a GFCI receptacle on its circuit ({needs})"),
                            ),
                        }
                    }
                    None => (
                        "unsure",
                        format!("{NO_DATA}: {dev_from}; GFCI is needed ({needs}); its breaker isn't GFCI ({row})"),
                        format!("GFCI protection ({needs})"),
                    ),
                }
            }
            Breaker::Unknown(why) => {
                if f.needs.is_none() && *dev != Some(true) {
                    continue;
                }
                let need = f.needs.as_deref().map(|n| format!("GFCI is needed ({n}); ")).unwrap_or_default();
                (
                    "unsure",
                    format!(
                        "{BREAKER_UNKNOWN}: {why}; {need}{kind_words} is {}",
                        match dev {
                            Some(true) => "a GFCI device",
                            Some(false) => "a standard device",
                            None => "of a type the model doesn't say",
                        }
                    ),
                    f.needs
                        .as_deref()
                        .map_or_else(|| "a standard receptacle if its breaker is GFCI".to_string(), |n| format!("GFCI protection ({n})")),
                )
            }
        };
        if verdict == "unsure" {
            let why = if matches!(f.breaker, Breaker::Unknown(_)) { BREAKER_UNKNOWN.to_string() } else { format!("{}: {NO_DATA} (gfci)", it.kind) };
            out.skip(&r.rule.id, &format!("gfci: {why}"));
        }
        out.tally(&r.rule.id, verdict);
        out.tally.add(it.kind, "gfci", verdict);
        let (breaker_json, breaker_row) = match &f.breaker {
            Breaker::Known(g, row) => (json!(if *g { "gfci" } else { "not gfci" }), json!(row)),
            Breaker::Unknown(why) => (json!("unknown"), json!(why)),
        };
        let found = match dev {
            Some(true) => "a GFCI receptacle",
            Some(false) => "a standard receptacle",
            None => NO_DATA,
        };
        let measured = json!({
            "field": "gfci",
            "device": {"gfci": dev, "from": dev_from},
            "breaker": breaker_json,
            "breaker_row": breaker_row,
            "circuit": f.circuit.as_ref().map(|c| c.1.clone()),
            "needs": f.needs,
            "protected_by": protector,
            "code": Code::cited(gf),
        });
        out.findings.push(finding(r, it, "gfci", verdict, issue, cite(&expected), found.into(), measured));
    }
}

// ---------------------------------------------------------------- packets

/// Each product element of a packet's `objects.json` gets its product record (what the
/// specification requires, what the submittal approved, what it resolves to) and its G-PRODUCT
/// and G-PRODUCT-CODE verdicts from the prescreen; the spec pages the area's records cite are
/// returned (`(section, page, product, kinds)`) for the packet's pictures.
pub(crate) fn annotate(d: &Drawing, elements: &mut [Value], findings: &[Value]) -> Vec<(String, String, String, BTreeSet<String>)> {
    let mut pages: BTreeMap<(String, String), (String, BTreeSet<String>)> = BTreeMap::new();
    for e in elements.iter_mut() {
        let Some(handle) = e.get("handle").and_then(Value::as_str).map(str::to_string) else { continue };
        let ent: Option<&Entity> = cadaddy_doc::Handle::parse_hex(&handle).and_then(|h| d.model.get(h)).map(|x| x.as_ref());
        let kind = e.get("kind").and_then(Value::as_str).unwrap_or_default().to_string();
        if let Some(rec) = super::validate_tr::product_of(ent, None) {
            if let Some(req) = rec.get("required").filter(|r| r.is_object())
                && let (Some(sec), Some(page)) = (text_of(req.get("section")), text_of(req.get("page")))
            {
                let product = text_of(req.get("product")).unwrap_or_default();
                let p = pages.entry((sec, page)).or_insert_with(|| (product, BTreeSet::new()));
                p.1.insert(kind.clone());
            }
            e["product_record"] = rec;
        }
        let checks: Vec<Value> = findings
            .iter()
            .filter(|f| RULES.contains(&f["rule"].as_str().unwrap_or_default()) && f["handle"].as_str().is_some_and(|h| h.eq_ignore_ascii_case(&handle)))
            .take(20)
            .map(|f| json!({"rule": f["rule"], "check": f["check"], "verdict": f["verdict"], "expected": f["expected"], "found": f["found"], "issue": f["issue"]}))
            .collect();
        if !checks.is_empty() {
            e["product_checks"] = json!(checks);
        }
    }
    pages.into_iter().map(|((sec, page), (product, kinds))| (sec, page, product, kinds)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn values_compare_by_field() {
        assert_eq!(same("color", "RED", "white"), Some(false));
        assert_eq!(same("color", "WH", "white unless otherwise indicated"), Some(true));
        assert_eq!(same("maker", "Acme Electric", "ACME"), Some(true));
        assert_eq!(same("maker", "Other Co", "Acme"), Some(false));
        assert_eq!(same("catalog", "XR-100-W", "XR-100"), Some(true));
        assert_eq!(same("catalog", "ZZ-200", "XR-100"), Some(false));
        assert_eq!(same("rating", "20A 125V", "20 AMP, 125 V"), Some(true));
        assert_eq!(same("rating", "15A 125V", "20 A, 125 V"), Some(false));
        assert_eq!(same("rating", "NEMA 5-20R", "20A 125V"), Some(true));
        assert_eq!(same("configuration", "duplex", "DUPLEX RECEPTACLE"), Some(true));
        assert_eq!(same("configuration", "single", "duplex"), Some(false));
    }

    #[test]
    fn features_are_read_canonically() {
        assert!(feature_keys("TAMPER-RESISTANT").contains("tamper_resistant"));
        assert!(feature_keys("TR").contains("tamper_resistant"));
        assert!(feature_keys("WRTR").contains("weather_resistant") && feature_keys("WRTR").contains("tamper_resistant"));
        assert!(feature_keys("Ground fault circuit interrupter").contains("gfci"));
        assert!(feature_keys("USB charging").contains("usb"));
        assert!(feature_keys("self-test").contains("self_test"));
        let want = Want { any_of: vec!["TR".into(), "self test".into()], not: Vec::new(), from: "spec", source: "spec".into(), note: None };
        let have: BTreeSet<String> = ["tamper_resistant".to_string()].into();
        assert_eq!(missing_features(&have, &want), vec!["self test".to_string()]);
    }

    #[test]
    fn the_code_rules_have_tr_gfci_and_wr() {
        let c = Code::load();
        assert!(c.tr.as_ref().is_some_and(|r| r.cite.contains("406.12")));
        assert!(c.gf.as_ref().is_some_and(|r| r.sink_reach_ft.is_some() && !r.places.is_empty()));
        assert!(c.wr.as_ref().is_some_and(|r| r.cite.contains("406.9") && !r.places.is_empty()));
        assert_eq!(c.tr_max, (Some(20.0), Some(250.0)));
    }
}
