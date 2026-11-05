//! Low-voltage pathways (Q6203): each low-voltage outlet's pathway (data, telephone, TV, card
//! reader, camera, access point, audio-visual) routed and measured like a branch circuit's leg.
//!
//! - **What the documents say** ([`read`]): the job reads every note, legend note and
//!   specification paragraph that names a pathway for low-voltage outlets — its method (ENT, EMT,
//!   flex, J-hooks), size (a minimum), where it ends (a stub into the accessible ceiling, into the
//!   corridor's ceiling space, a home run to the MDF/IDF or the unit's enclosure, a sleeve through
//!   a rated wall), one per room, a bracket or mud ring at the outlet, who firestops — and stores
//!   them ([`DOCS_FILE`]). The low-voltage system's own documents (its sheets, Division 27/28)
//!   govern over a general electrical note; a statement for a kind of outlet or a type of room
//!   over a general one; where none says, the rules data's default for the area (`lvpath.json`).
//! - **Route**: from the outlet's box up the wall into the ceiling space (the branch router's
//!   rise, tiers and clearances), across to where the pathway ends; through the rated walls it
//!   crosses (each a penetration with its firestop and the wall type's rating), a riser through
//!   the floors to a telecom room on another storey.
//! - **Pieces**: the routed runs go into the spec as [`LV_SYSTEM`] with a tag per outlet
//!   ([`TAG_KIND`]: its method, size, ends, sources), so the build cuts them into sticks, couplings,
//!   connectors, straps and the bushing at a stub's open end; the outlet takes a low-voltage
//!   bracket instead of a box where the documents say. Nothing is built when the scope split
//!   gives the pathways to another trade (they are listed as theirs).

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::OnceLock;

use cadaddy_doc::Drawing;
use cadaddy_doc::props::{self, Prop};
use cadaddy_geom::Vec2;
use cadaddy_walk3d::tags::ElementTag;
use cadaddy_walk3d::{BuildingSpec, MepRun};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::{
    BRANCH_SYSTEM, Dev, Field, Leg, ROUTED, Rated, RouteRules, corridor_path, crosses, from_box, inside, manhattan, penetrate, poly_len, r1, r2,
    rise_point, route_leg, run_ft, same_level, to_box,
};
use crate::cmd::scope;

/// The system the low-voltage pathways go into the spec as.
pub(crate) const LV_SYSTEM: &str = "LV-PATHWAY";
/// The kind of the spec tag each outlet's pathway carries.
pub(crate) const TAG_KIND: &str = "lv_pathway";
/// The id prefix of those tags (and of their runs: `lv:<device>#<n>`).
pub(crate) const TAG_PREFIX: &str = "lv:";
/// What the job read of the documents' pathway statements, among the drawing's embedded files.
pub(crate) const DOCS_FILE: &str = "route/lvpath.json";

const RULES: &str = include_str!("lvpath.json");
/// Most statements kept, outlets routed, report rows listed (hostile documents and models).
const MAX_STATEMENTS: usize = 500;
const MAX_OUTLETS: usize = 50_000;
const MAX_ROWS: usize = 5_000;
const MAX_ROOMS: usize = 50_000;

// ─── Rules ───────────────────────────────────────────────────────────────────

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub(crate) struct Kinds {
    pub words: Vec<String>,
    pub not_words: Vec<String>,
    pub enclosure_words: Vec<String>,
    pub source: String,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub(crate) struct Method {
    pub words: Vec<String>,
    pub raceway: bool,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub(crate) struct Words {
    pub pathway_words: Vec<String>,
    pub lv_words: Vec<String>,
    pub power_words: Vec<String>,
    pub unit_words: Vec<String>,
    pub common_words: Vec<String>,
    pub all_words: Vec<String>,
    pub to_corridor: Vec<String>,
    pub to_ceiling: Vec<String>,
    pub to_room: Vec<String>,
    pub home_words: Vec<String>,
    pub to_enclosure: Vec<String>,
    pub to_sleeve: Vec<String>,
    pub one_per_room: Vec<String>,
    pub minimum_words: Vec<String>,
    pub bracket_words: Vec<String>,
    pub box_words: Vec<String>,
    pub firestop_words: Vec<String>,
    pub hard_ceiling_words: Vec<String>,
    /// A sheet whose title names these is the low-voltage trade's, whatever its number's prefix.
    pub sheet_title_words: Vec<String>,
    pub kind_words: BTreeMap<String, Vec<String>>,
    pub spec_application_words: BTreeMap<String, Vec<String>>,
    pub source: String,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub(crate) struct Default1 {
    pub method: String,
    pub size_in: f64,
    pub to: String,
    pub source: String,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub(crate) struct Geometry {
    pub stub_above_ceiling_in: f64,
    pub into_corridor_ft: f64,
    pub at_ceiling_in: f64,
    pub source: String,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub(crate) struct Fittings {
    pub stub_end: String,
    pub stub_end_source: String,
    pub pull_string: String,
    pub pull_string_source: String,
    pub pull_string_extra_ft: f64,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub(crate) struct Bracket {
    pub item: String,
    pub size: String,
    pub default: bool,
    pub source: String,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub(crate) struct JHooks {
    pub item: String,
    pub spacing_ft: f64,
    pub spacing_source: String,
    pub ours: bool,
    pub ours_source: String,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub(crate) struct Sleeve {
    pub item: String,
    pub over_in: f64,
    pub source: String,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub(crate) struct Firestop {
    pub item: String,
    pub floor_unrated: String,
    pub source: String,
    pub ours: bool,
    pub ours_source: String,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub(crate) struct LvRules {
    pub kinds: Kinds,
    pub methods: BTreeMap<String, Value>,
    pub statements: Words,
    pub defaults: BTreeMap<String, Default1>,
    pub geometry: Geometry,
    pub fittings: Fittings,
    pub bracket: Bracket,
    pub jhooks: JHooks,
    pub sleeve: Sleeve,
    pub firestop: Firestop,
}

impl LvRules {
    /// The methods by name (`ENT`, `J-HOOK`), each with its words.
    fn method_list(&self) -> Vec<(String, Method)> {
        self.methods
            .iter()
            .filter_map(|(k, v)| v.is_object().then(|| serde_json::from_value::<Method>(v.clone()).ok().map(|m| (k.clone(), m))).flatten())
            .collect()
    }
    fn prefer(&self) -> Vec<String> {
        self.methods.get("prefer").and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_str).map(str::to_string).collect()
    }
    fn prefer_source(&self) -> String {
        self.methods.get("prefer_source").and_then(Value::as_str).unwrap_or_default().to_string()
    }
    fn generic(&self) -> Vec<String> {
        self.methods.get("generic_words").and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_str).map(str::to_string).collect()
    }
    /// Whether a method is a raceway (J-hooks and open cable are not).
    pub(crate) fn is_raceway(&self, method: &str) -> bool {
        self.method_list().iter().find(|(k, _)| k.eq_ignore_ascii_case(method)).is_none_or(|(_, m)| m.raceway)
    }
}

/// The rules data, read once.
pub(crate) fn rules() -> &'static LvRules {
    static R: OnceLock<LvRules> = OnceLock::new();
    R.get_or_init(|| serde_json::from_str(RULES).unwrap_or_default())
}

// ─── Words ───────────────────────────────────────────────────────────────────

/// Text as words for phrase matching: upper case, letters and digits (a hyphen inside a word
/// kept), each word between single spaces, with a space at each end.
fn norm(t: &str) -> String {
    let cs: Vec<char> = t.chars().take(20_000).map(|c| c.to_ascii_uppercase()).collect();
    let mut out = String::with_capacity(cs.len() + 2);
    out.push(' ');
    for (i, c) in cs.iter().enumerate() {
        let word = |j: Option<usize>| j.and_then(|j| cs.get(j)).is_some_and(char::is_ascii_alphanumeric);
        if c.is_ascii_alphanumeric() {
            out.push(*c);
        } else if *c == '-' && word(i.checked_sub(1)) && word(Some(i + 1)) {
            out.push('-');
        } else if !out.ends_with(' ') {
            out.push(' ');
        }
    }
    if !out.ends_with(' ') {
        out.push(' ');
    }
    out
}

/// Where a phrase is in normalised text (its first word's place), if it is.
fn find(hay: &str, phrase: &str) -> Option<usize> {
    let p = norm(phrase);
    if p.trim().is_empty() {
        return None;
    }
    hay.find(&p)
}

/// Whether normalised text has any of the phrases (a hyphenated word also as its parts: `IDF-1`
/// names an IDF).
fn has_any(hay: &str, phrases: &[String]) -> bool {
    let parts = hay.contains('-').then(|| hay.replace('-', " "));
    phrases.iter().any(|p| find(hay, p).is_some() || parts.as_deref().is_some_and(|x| find(x, p).is_some()))
}

/// Whether a sheet's title names the low-voltage systems (`LOW VOLTAGE PLANS`, `TELECOM`).
pub(crate) fn lv_sheet_title(title: &str) -> bool {
    has_any(&norm(title), &rules().statements.sheet_title_words)
}

/// The set's low-voltage sheets (indices into the sheet index): each titled for the low-voltage
/// systems, and the other sheets of its series (its number's letters, in its file: `V-1` beside
/// `V-2 LOW VOLTAGE PLANS`; a title block can be misread, and another file's `V-9` is another
/// series).
pub(crate) fn lv_sheets(classified: &[Value]) -> Vec<usize> {
    let text = |c: &Value, k: &str| c.get(k).and_then(Value::as_str).unwrap_or("").trim().to_string();
    let series = |c: &Value| {
        let letters: String = text(c, "number").chars().take_while(char::is_ascii_alphabetic).collect::<String>().to_ascii_uppercase();
        (letters, text(c, "file"))
    };
    let titled = |c: &Value| lv_sheet_title(&text(c, "title"));
    let lv_series: BTreeSet<(String, String)> =
        classified.iter().take(MAX_ROWS).filter(|c| titled(c)).map(series).filter(|(l, f)| !l.is_empty() && !f.is_empty()).collect();
    classified.iter().enumerate().take(MAX_ROWS).filter(|(_, c)| titled(c) || lv_series.contains(&series(c))).map(|(i, _)| i).collect()
}

/// The trade sizes a text names (`3/4"`, `1-inch`, `1-1/4 IN`, `3/4-INCH (21-MM)`), with where
/// each is (in the normalised text's words: the index of the size's last word). 0.5–4 in only.
fn sizes(raw: &str) -> Vec<(f64, usize)> {
    let up = raw.to_ascii_uppercase();
    let cs: Vec<char> = up.chars().take(20_000).collect();
    let mut out = Vec::new();
    let mut i = 0usize;
    while i < cs.len() {
        let starts = cs.get(i).is_some_and(char::is_ascii_digit)
            && !i.checked_sub(1).and_then(|j| cs.get(j)).is_some_and(|c| c.is_ascii_alphanumeric() || *c == '/');
        if !starts {
            i += 1;
            continue;
        }
        // The number: digits, a fraction, a whole and a fraction (`1-1/4`, `1 1/4`), a decimal.
        let mut j = i;
        let mut text = String::new();
        while let Some(c) = cs.get(j) {
            let more = c.is_ascii_digit()
                || *c == '/'
                || *c == '.' && cs.get(j + 1).is_some_and(char::is_ascii_digit)
                || (*c == '-' || *c == ' ')
                    && cs.get(j + 1).is_some_and(char::is_ascii_digit)
                    && cs.get(j + 2..j + 4).is_some_and(|w| w.contains(&'/'));
            if !more || text.len() > 8 {
                break;
            }
            text.push(*c);
            j += 1;
        }
        let rest: String = cs.get(j..(j + 8).min(cs.len())).map(|w| w.iter().collect()).unwrap_or_default();
        let inch = rest.starts_with('"')
            || rest.starts_with("''")
            || rest.starts_with("\u{201d}")
            || rest.trim_start_matches(['-', ' ']).starts_with("INCH")
            || rest.trim_start_matches(['-', ' ']).starts_with("IN.")
            || rest.trim_start_matches(['-', ' ']).starts_with("IN ");
        if inch && let Some(v) = parse_size(&text).filter(|v| (0.5..=4.0).contains(v)) {
            let words_before = norm(cs.get(..i).map(|w| w.iter().collect::<String>()).unwrap_or_default().as_str()).split_whitespace().count();
            out.push((v, words_before));
        }
        i = j.max(i + 1);
    }
    out
}

/// `3/4` → 0.75, `1-1/4` or `1 1/4` → 1.25, `1.5` → 1.5.
fn parse_size(t: &str) -> Option<f64> {
    let t = t.trim();
    let frac = |f: &str| -> Option<f64> {
        let (n, d) = f.split_once('/')?;
        let (n, d) = (n.trim().parse::<f64>().ok()?, d.trim().parse::<f64>().ok()?);
        (d > 0.0).then(|| n / d)
    };
    if let Some((whole, f)) = t.split_once(['-', ' ']).filter(|(_, f)| f.contains('/')) {
        return Some(whole.trim().parse::<f64>().ok()? + frac(f)?);
    }
    if t.contains('/') {
        return frac(t);
    }
    t.parse::<f64>().ok().filter(|v| v.is_finite())
}

/// The sentences of a paragraph (a full stop, semicolon or colon followed by a space ends one).
fn sentences(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let cs: Vec<char> = text.chars().take(20_000).collect();
    // Abbreviations a full stop doesn't end a sentence after.
    const ABBREV: [&str; 12] = ["MIN", "MAX", "TYP", "NO", "IN", "FT", "APPROX", "EA", "AFF", "O.C", "U.N.O", "MIN.)"];
    for (i, c) in cs.iter().enumerate() {
        cur.push(*c);
        let next_space = cs.get(i + 1).is_none_or(|n| n.is_whitespace());
        let decimal = *c == '.' && cs.get(i + 1).is_some_and(char::is_ascii_digit);
        let word: String = cur.trim_end_matches('.').rsplit(|ch: char| ch.is_whitespace() || ch == '(').next().unwrap_or("").to_ascii_uppercase();
        let abbrev = *c == '.' && ABBREV.contains(&word.as_str());
        if matches!(c, '.' | ';') && next_space && !decimal && !abbrev {
            out.push(std::mem::take(&mut cur));
        }
    }
    if !cur.trim().is_empty() {
        out.push(cur);
    }
    out.into_iter().map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect()
}

// ─── What the documents say ──────────────────────────────────────────────────

/// One document statement of a low-voltage pathway, as read.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct Stmt {
    pub doc: String,
    pub at: String,
    pub text: String,
    /// From the low-voltage system's own documents (its sheets, Division 27/28).
    pub lv_doc: bool,
    pub spec: bool,
    /// The kinds of outlet it is for (`camera`, `tv`); none: any.
    pub kinds: Vec<String>,
    /// The areas it is for (`unit`, `common`); none: any.
    pub areas: Vec<String>,
    /// A specification's application (`concealed`, `exposed`): the method there.
    pub application: String,
    /// The methods it names (rules keys: `ENT`, `EMT`, `J-HOOK`).
    pub methods: Vec<String>,
    /// It names a conduit or raceway with no material.
    pub generic: bool,
    pub size_in: Option<f64>,
    pub minimum: bool,
    /// Where the pathway ends: `corridor`, `ceiling`, `room`, `enclosure`, `sleeve`.
    pub to: String,
    pub one_per_room: bool,
    pub bracket: Option<bool>,
    pub hard_ceiling_only: bool,
}

impl Stmt {
    pub(crate) fn cite(&self) -> String {
        let at = if self.at.is_empty() || self.at == self.doc { String::new() } else { format!(" {}", self.at) };
        let text: String = self.text.chars().take(240).collect();
        format!("{}{at}: \"{text}\"", self.doc)
    }
    fn gives(&self) -> bool {
        !self.methods.is_empty() || self.generic || self.size_in.is_some() || !self.to.is_empty() || self.bracket.is_some() || self.one_per_room
    }
}

/// What the job read of the documents' low-voltage pathways.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct LvDocs {
    pub statements: Vec<Stmt>,
    /// The specification's minimum pathway size (inches) and where it says so.
    pub min_size_in: Option<f64>,
    pub min_size_source: String,
    /// The documents' J-hook spacing (feet) and where.
    pub jhook_spacing_ft: Option<f64>,
    pub jhook_spacing_source: String,
    /// Who the documents give the firestopping of the low-voltage penetrations to (a scope
    /// party: `low voltage`, `electrical`) and where.
    pub firestop_by: String,
    pub firestop_source: String,
    /// Who the documents give the J-hooks to, and where.
    pub jhooks_by: String,
    pub jhooks_source: String,
    pub docs_read: usize,
}

impl LvDocs {
    pub(crate) fn stored(d: &Drawing) -> Self {
        d.embedded.get(DOCS_FILE).and_then(|b| serde_json::from_slice(b).ok()).unwrap_or_default()
    }
    pub(crate) fn store(&self, d: &mut Drawing) {
        if let Ok(b) = serde_json::to_vec(self) {
            d.embedded.insert(DOCS_FILE.into(), std::sync::Arc::new(b));
        }
    }
}

/// The scope party a sentence gives a thing to (`... FIRESTOPPING OF CABLE PENETRATIONS BY THE
/// CABLING CONTRACTOR`): the party named after `BY`, else the one named first.
fn party_in(sentence: &str) -> Option<String> {
    let sr = scope::rules();
    let n = norm(sentence);
    let after_by = find(&n, "BY").map_or(n.as_str(), |i| n.get(i..).unwrap_or(""));
    let named = |hay: &str| -> Option<String> {
        sr.parties
            .iter()
            .flat_map(|(k, p)| p.words.iter().filter_map(move |w| find(hay, w).map(|at| (at, std::cmp::Reverse(w.len()), k.clone()))))
            .min()
            .map(|(_, _, k)| k)
    };
    named(after_by).or_else(|| named(&n))
}

/// Read the documents (the scope reading's: sheets' notes as paragraphs, specification sections,
/// submittals) for their low-voltage pathway statements.
pub(crate) fn read(docs: &[scope::Doc]) -> LvDocs {
    let lr = rules();
    let w = &lr.statements;
    let methods = lr.method_list();
    let generic = lr.generic();
    let mut out = LvDocs::default();
    for doc in docs {
        let lv_doc = doc.trade.as_deref() == Some("low voltage");
        let spec = doc.tier == cadaddy_doc::projrules::SourceTier::Spec;
        // A product's data sheet says nothing of how its pathway is installed; a specification
        // section of another division's power raceways nothing of the low-voltage outlets'.
        if doc.tier == cadaddy_doc::projrules::SourceTier::Submittal || (spec && !lv_doc) {
            continue;
        }
        let mut any = false;
        for para in &doc.paras {
            if out.statements.len() >= MAX_STATEMENTS {
                break;
            }
            let whole = norm(&format!("{} {}", para.heading, para.text));
            let lv = lv_doc || has_any(&whole, &w.lv_words);
            if !lv {
                continue;
            }
            let ss = sentences(&para.text);
            // Who firestops the penetrations; who hangs the J-hooks.
            for s in &ss {
                let n = norm(s);
                if has_any(&n, &w.firestop_words)
                    && out.firestop_by.is_empty()
                    && let Some(p) = party_in(s)
                {
                    out.firestop_by = p;
                    out.firestop_source = format!("{} {}: \"{}\"", doc.name, para.at, s.chars().take(200).collect::<String>());
                }
                if find(&n, "HOOK").or_else(|| find(&n, "HOOKS")).is_some() {
                    // `Hang cable hooks 5 feet apart at most.`
                    let ws: Vec<&str> = n.split_whitespace().collect();
                    let ft = ws.windows(2).find_map(|p| match p {
                        [v, u] if matches!(*u, "FEET" | "FT" | "FOOT") => v.parse::<f64>().ok().filter(|x| (1.0..=20.0).contains(x)),
                        _ => None,
                    });
                    if let Some(ft) = ft.filter(|_| out.jhook_spacing_ft.is_none()) {
                        out.jhook_spacing_ft = Some(ft);
                        out.jhook_spacing_source = format!("{} {}: \"{}\"", doc.name, para.at, s.chars().take(200).collect::<String>());
                    }
                    if out.jhooks_by.is_empty()
                        && has_any(&n, &w.home_words.iter().chain(&w.pathway_words).cloned().collect::<Vec<_>>())
                        && let Some(p) = party_in(s)
                    {
                        out.jhooks_by = p;
                        out.jhooks_source = format!("{} {}: \"{}\"", doc.name, para.at, s.chars().take(200).collect::<String>());
                    }
                }
                // The minimum pathway size (`Minimum raceway size is 3/4-inch`).
                if has_any(&n, &w.minimum_words)
                    && (find(&n, "PATHWAY SIZE").is_some() || find(&n, "RACEWAY SIZE").is_some() || find(&n, "CONDUIT SIZE").is_some())
                    && let Some((v, _)) = sizes(s).first()
                    && out.min_size_in.is_none_or(|m| *v > m)
                {
                    out.min_size_in = Some(*v);
                    out.min_size_source = format!("{} {}: \"{}\"", doc.name, para.at, s.chars().take(200).collect::<String>());
                }
            }
            // The sentences that name a pathway, a method or a bracket: what the statement gives.
            let path_sentences: Vec<&String> = ss
                .iter()
                .filter(|s| {
                    let n = norm(s);
                    has_any(&n, &w.pathway_words) || methods.iter().any(|(_, m)| has_any(&n, &m.words)) || has_any(&n, &w.bracket_words)
                })
                .collect();
            if path_sentences.is_empty() {
                continue;
            }
            // A power note isn't about the low-voltage outlets.
            if has_any(&whole, &w.power_words) && !has_any(&whole, &w.lv_words) {
                continue;
            }
            // Its subject, from the sentences that name the pathway (and the heading they are
            // under): the kinds of outlet, the areas it names (an area named only as where the
            // pathway goes, `GUEST ROOM CORRIDOR`, isn't one), all outlets.
            // The kinds from those sentences; the areas from the whole note (`OFFICE OUTLETS: ...
            // CONDUIT FROM EACH BOX ...`).
            let said: String = path_sentences.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(" ");
            let mut subject = norm(&format!("{} {said}", para.heading));
            let mut area_text = whole.clone();
            for a in w.unit_words.iter().chain(&w.common_words) {
                for c in &w.to_corridor {
                    let both = norm(&format!("{a} {c}"));
                    subject = subject.replace(both.as_str(), " ");
                    area_text = area_text.replace(both.as_str(), " ");
                }
            }
            let kinds: Vec<String> = w.kind_words.iter().filter(|(_, ws)| has_any(&subject, ws)).map(|(k, _)| k.clone()).collect();
            let mut areas = Vec::new();
            if has_any(&area_text, &w.unit_words) {
                areas.push("unit".to_string());
            }
            if has_any(&area_text, &w.common_words) {
                areas.push("common".to_string());
            }
            let all = has_any(&area_text, &w.all_words);
            let application = if spec {
                w.spec_application_words.iter().find(|(_, ws)| has_any(&whole, ws)).map(|(k, _)| k.clone()).unwrap_or_default()
            } else {
                String::new()
            };
            // A specification gives the method by application (and the minimum size, the hooks'
            // spacing, read above); where each outlet's pathway goes is the drawings'.
            if spec && application.is_empty() {
                continue;
            }
            if kinds.is_empty() && areas.is_empty() && !all && application.is_empty() {
                // Not said of the outlets (a riser sleeve, a service conduit).
                continue;
            }
            // A keynote is said of the places it is marked at: one that names no area (a
            // typical room's) is not the outlets' rule.
            if norm(&para.heading).contains(" KEYNOTE") && areas.is_empty() && !all {
                continue;
            }
            // A backbone: conduit from the corridor or a telecom room on to another, or two inches
            // and up (the cabling's trunk, not an outlet's pathway).
            let said_n = norm(&said);
            let words: Vec<&str> = said_n.split_whitespace().collect();
            let from_trunk = words.iter().enumerate().any(|(i, x)| {
                *x == "FROM"
                    && words.get(i + 1..(i + 6).min(words.len())).is_some_and(|after| {
                        let tail = format!(" {} ", after.join(" "));
                        has_any(&tail, &w.to_corridor) || has_any(&tail, &w.to_room)
                    })
            });
            let trunk_size = sizes(&said).iter().any(|(v, _)| *v >= 2.0 - 1e-9);
            if from_trunk || trunk_size {
                continue;
            }
            let text: String = path_sentences.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(" ");
            let n = norm(&text);
            let mut st = Stmt {
                doc: doc.name.clone(),
                at: para.at.clone(),
                text: text.chars().take(600).collect(),
                lv_doc,
                spec,
                kinds,
                areas,
                application,
                ..Stmt::default()
            };
            st.methods = methods.iter().filter(|(_, m)| has_any(&n, &m.words)).map(|(k, _)| k.clone()).collect();
            st.generic = has_any(&n, &generic);
            // The size nearest a pathway word.
            let words: Vec<&str> = n.split_whitespace().collect();
            let path_at: Vec<usize> = words
                .iter()
                .enumerate()
                .filter(|(_, x)| {
                    let one = format!(" {x} ");
                    has_any(&one, &w.pathway_words) || methods.iter().any(|(_, m)| has_any(&one, &m.words))
                })
                .map(|(i, _)| i)
                .collect();
            let found = sizes(&text);
            if let Some((v, at)) = found.iter().min_by_key(|(_, at)| path_at.iter().map(|p| p.abs_diff(*at)).min().unwrap_or(usize::MAX)) {
                st.size_in = Some(*v);
                st.minimum = words
                    .get(at.saturating_sub(1)..(at + 4).min(words.len()))
                    .is_some_and(|ws| ws.iter().any(|x| w.minimum_words.iter().any(|m| m.trim_end_matches('.').eq_ignore_ascii_case(x))));
            }
            // A print's text can run a word into the next (`CORRIDORLOCATION`): a long place word
            // at a word's start is that place.
            let glued = |places: &[String]| {
                n.split_whitespace()
                    .any(|t| places.iter().any(|c| c.len() >= 6 && !c.contains(' ') && t.len() > c.len() && t.starts_with(c.as_str())))
            };
            st.to = if has_any(&n, &w.to_room) && (has_any(&n, &w.home_words) || has_any(&n, &w.pathway_words)) {
                "room"
            } else if has_any(&n, &w.to_enclosure) && !has_any(&n, &w.box_words) {
                "enclosure"
            } else if has_any(&n, &w.to_corridor) || glued(&w.to_corridor) {
                "corridor"
            } else if has_any(&n, &w.to_sleeve) && !has_any(&n, &w.to_ceiling) {
                "sleeve"
            } else if has_any(&n, &w.to_ceiling) {
                "ceiling"
            } else {
                ""
            }
            .to_string();
            st.one_per_room = has_any(&whole, &w.one_per_room);
            if has_any(&n, &w.bracket_words) {
                st.bracket = Some(true);
                st.hard_ceiling_only = has_any(&n, &w.hard_ceiling_words);
            }
            if !st.gives() {
                continue;
            }
            any = true;
            if !out.statements.iter().any(|x| x.text == st.text && x.doc == st.doc) {
                out.statements.push(st);
            }
        }
        out.docs_read += usize::from(any);
    }
    out
}

// ─── Each outlet's pathway ───────────────────────────────────────────────────

/// What one outlet's pathway is, each choice with its source.
#[derive(Clone, Debug, Default)]
pub(crate) struct Choice {
    pub method: String,
    pub method_source: String,
    pub size_in: f64,
    pub size_source: String,
    pub to: String,
    pub to_source: String,
    pub one_per_room: bool,
    pub one_source: String,
    pub bracket: bool,
    pub bracket_source: String,
    /// Statements that disagree with what was chosen (lower ranked), as words.
    pub conflicts: Vec<String>,
}

/// The pathway the documents (else the rules) give an outlet of these `kinds` in `area`
/// (`unit`, `common`, `exposed`), `at_ceiling` (its box in the ceiling).
pub(crate) fn choose(docs: &LvDocs, kinds: &[String], area: &str, at_ceiling: bool) -> Choice {
    let lr = rules();
    let applies = |st: &Stmt| -> Option<(u8, bool, usize)> {
        if !st.kinds.is_empty() && !st.kinds.iter().any(|k| kinds.contains(k)) {
            return None;
        }
        let area_ok = st.areas.is_empty() || st.areas.iter().any(|a| a == area);
        if !area_ok {
            return None;
        }
        if !st.application.is_empty() {
            let app_ok = match st.application.as_str() {
                "exposed" => area == "exposed",
                _ => area != "exposed",
            };
            if !app_ok {
                return None;
            }
        }
        let spec = 2 * u8::from(!st.kinds.is_empty()) + u8::from(!st.areas.is_empty());
        Some((spec, st.lv_doc, 0))
    };
    let mut ranked: Vec<(u8, bool, usize, &Stmt)> =
        docs.statements.iter().enumerate().filter_map(|(i, s)| applies(s).map(|(a, b, _)| (a, b, i, s))).collect();
    // Most specific first; the low-voltage system's own documents first; then as read.
    ranked.sort_by(|a, b| b.0.cmp(&a.0).then(b.1.cmp(&a.1)).then(a.2.cmp(&b.2)));
    let mut c = Choice::default();
    let prefer = lr.prefer();
    let pick_method = |st: &Stmt| -> Option<String> {
        if st.methods.is_empty() {
            return None;
        }
        prefer.iter().find(|p| st.methods.iter().any(|m| m.eq_ignore_ascii_case(p))).cloned().or_else(|| st.methods.first().cloned())
    };
    // Each choice from the highest ranked statement that makes it.
    let mut method_at: Option<usize> = None;
    for (i, (_, _, _, st)) in ranked.iter().enumerate() {
        if let Some(m) = pick_method(st) {
            let several = st.methods.len() > 1 || st.generic;
            c.method = m;
            c.method_source = if several { format!("{} ({})", st.cite(), lr.prefer_source()) } else { st.cite() };
            method_at = Some(i);
            break;
        }
    }
    let size_at = ranked.iter().position(|(_, _, _, st)| st.size_in.is_some());
    if let Some((_, _, _, st)) = size_at.and_then(|i| ranked.get(i)) {
        c.size_in = st.size_in.unwrap_or(0.0);
        c.size_source = st.cite();
    }
    let to_at = ranked.iter().position(|(_, _, _, st)| !st.to.is_empty());
    if let Some((_, _, _, st)) = to_at.and_then(|i| ranked.get(i)) {
        c.to = st.to.clone();
        c.to_source = st.cite();
    }
    if let Some((_, _, _, st)) = ranked.iter().find(|(_, _, _, st)| st.one_per_room) {
        c.one_per_room = true;
        c.one_source = st.cite();
    }
    if let Some((_, _, _, st)) = ranked.iter().find(|(_, _, _, st)| st.bracket.is_some() && (!st.hard_ceiling_only || at_ceiling)) {
        c.bracket = st.bracket.unwrap_or(false);
        c.bracket_source = st.cite();
    }
    // Lower ranked statements that say otherwise.
    for (i, (_, _, _, st)) in ranked.iter().enumerate() {
        if c.conflicts.len() >= 4 {
            break;
        }
        if method_at.is_some_and(|m| i > m)
            && pick_method(st).is_some_and(|m| !m.eq_ignore_ascii_case(&c.method))
            && !st.methods.iter().any(|m| m == &c.method)
        {
            c.conflicts.push(format!("method: {} says {}, {} was taken", st.cite(), st.methods.join("/"), c.method));
        }
        if size_at.is_some_and(|s| i > s) && st.size_in.is_some_and(|v| (v - c.size_in).abs() > 1e-6) {
            c.conflicts.push(format!(
                "size: {} says {}, {} was taken",
                st.cite(),
                super::super::mepatoms::trade_size(st.size_in.unwrap_or(0.0)),
                super::super::mepatoms::trade_size(c.size_in)
            ));
        }
    }
    // The defaults for what no statement settles; the specification's minimum size.
    let def = lr.defaults.get(area).or_else(|| lr.defaults.get("common"));
    if c.method.is_empty() {
        let generic = ranked.iter().find(|(_, _, _, st)| st.generic);
        c.method = def.map_or_else(|| "EMT".to_string(), |d| d.method.clone());
        c.method_source = match generic {
            Some((_, _, _, st)) => format!("{} names a conduit with no material: {}", st.cite(), def.map(|d| d.source.clone()).unwrap_or_default()),
            None => def.map(|d| d.source.clone()).unwrap_or_default(),
        };
    }
    if c.size_in <= 0.0 {
        c.size_in = def.map_or(0.75, |d| d.size_in);
        c.size_source = def.map(|d| d.source.clone()).unwrap_or_default();
    }
    if let Some(m) = docs.min_size_in.filter(|m| *m > c.size_in + 1e-9) {
        c.size_source = format!("{} raised to the specification's minimum: {}", c.size_source, docs.min_size_source);
        c.size_in = m;
    }
    if c.to.is_empty() {
        c.to = def.map_or_else(|| "ceiling".to_string(), |d| d.to.clone());
        c.to_source = def.map(|d| d.source.clone()).unwrap_or_default();
    }
    if c.bracket_source.is_empty() {
        c.bracket = lr.bracket.default;
        c.bracket_source = lr.bracket.source.clone();
    }
    c
}

// ─── Routing ─────────────────────────────────────────────────────────────────

/// What the router gives the pathways: the model, its levels' obstacles and ceilings, its
/// corridors, the rated walls.
pub(super) struct Ctx<'a> {
    pub d: &'a Drawing,
    pub sp: &'a BuildingSpec,
    pub upf: f64,
    pub rl: &'a RouteRules,
    pub fields: &'a BTreeMap<String, Field>,
    pub empty: &'a Field,
    pub rated: &'a [Rated],
    pub devs: &'a [Dev],
    pub tiers_of: &'a dyn Fn(&str, Option<f64>) -> (f64, Vec<f64>),
    pub box_ceiling: &'a dyn Fn(&str, Vec2, Option<Vec2>) -> Option<f64>,
    pub ceiling_at: &'a dyn Fn(&str, Vec2) -> Option<f64>,
    pub corridors: &'a dyn Fn(&str) -> Vec<[[f64; 2]; 2]>,
    pub carry: &'a dyn Fn(Vec2, &str, &str) -> Vec2,
}

/// What routing the pathways made.
#[derive(Default)]
pub(super) struct Out {
    pub runs: Vec<MepRun>,
    pub tags: Vec<ElementTag>,
    pub report: Value,
    /// The devices (by index among the router's) that are low-voltage outlets.
    pub outlets: BTreeSet<usize>,
}

/// Whether a device is a low-voltage outlet: its kind, type and description name one (and not a
/// fire alarm device or a power device), or the scope rules class it with them.
pub(crate) fn is_outlet(words: &str) -> bool {
    let lr = rules();
    let n = norm(&words.replace('_', " "));
    if has_any(&n, &lr.kinds.not_words) {
        return false;
    }
    has_any(&n, &lr.kinds.words)
}

/// A device's words: its kind, type, device type, description, name.
fn device_words(d: &Drawing, dv: &Dev) -> String {
    let ps = d.model.get(dv.handle).map(|e| props::read(&e.common.xdata)).unwrap_or_default();
    props_words(&ps, &dv.kind)
}

fn props_words(ps: &props::Props, kind: &str) -> String {
    let g = |k: &str| ps.get(k).map(Prop::text).unwrap_or_default();
    format!("{kind} {} {} {} {}", g("type"), g("device_type"), g("description"), g("name"))
}

/// Whether a device of `kind` with these words is a low-voltage outlet (its words, or the scope
/// rules' low-voltage class).
fn outlet_words(kind: &str, words: &str) -> bool {
    is_outlet(words) || scope::rules().class_of(kind, words).is_some_and(|c| c.class == "low voltage")
}

/// The router's devices that are low-voltage outlets (their indices).
pub(super) fn outlets(d: &Drawing, devs: &[Dev]) -> BTreeSet<usize> {
    devs.iter().enumerate().filter(|(_, dv)| outlet_words(&dv.kind, &device_words(d, dv))).map(|(i, _)| i).take(MAX_OUTLETS).collect()
}

/// Whether the built model has a low-voltage outlet (a device element).
pub(crate) fn any_outlet(d: &Drawing) -> bool {
    d.model.iter().filter(|e| super::super::wallgen::made_as(e).as_deref() == Some("device")).take(MAX_OUTLETS).any(|e| {
        let ps = props::read(&e.common.xdata);
        let kind = ps.get("kind").map(Prop::text).unwrap_or_default();
        outlet_words(&kind, &props_words(&ps, &kind))
    })
}

/// A telecom room as built: its level, name, outline and centre on its level's own plan.
struct TelecomRoom {
    level: String,
    name: String,
    centre: Vec2,
}

/// One outlet's routed pathway.
struct Path {
    dev: usize,
    level: String,
    room: String,
    area: &'static str,
    kinds: Vec<String>,
    choice: Choice,
    /// What it ends at, as routed (`corridor`, `ceiling`, `in ceiling`, `room MDF`, `joined`).
    ends: String,
    runs: Vec<MepRun>,
    ft: f64,
    /// Horizontal feet (J-hooks are along it) and its turns.
    flat_ft: f64,
    turns: usize,
    open_ends: usize,
    note: String,
}

pub(super) fn route(cx: &Ctx) -> Out {
    let lr = rules();
    let (d, sp, upf, rl) = (cx.d, cx.sp, cx.upf, cx.rl);
    let docs = LvDocs::stored(d);
    // -- The outlets. --
    let mut out = Out { outlets: outlets(d, cx.devs), ..Out::default() };
    let list: Vec<(usize, String)> = out.outlets.iter().filter_map(|i| cx.devs.get(*i).map(|dv| (*i, device_words(d, dv)))).collect();
    if list.is_empty() {
        out.report = json!({"outlets": 0, "note": "no low-voltage outlet in the model"});
        return out;
    }
    // -- Rooms: names by number, the telecom rooms, the units. --
    let mut names: HashMap<(String, String), String> = HashMap::new();
    let mut telecom: Vec<TelecomRoom> = Vec::new();
    let w = &lr.statements;
    for r in super::super::bspec::room_elements(d).into_iter().take(MAX_ROOMS) {
        let off = d.model.get(r.handle).map_or(Vec2::new(0.0, 0.0), |e| super::super::bspec::stacked_by(e));
        let name = r.name.trim().to_ascii_uppercase();
        if let Some(n) = r.number.as_ref().map(|n| n.trim().to_ascii_uppercase()).filter(|n| !n.is_empty()) {
            names.insert((r.level.to_ascii_lowercase(), n), name.clone());
        }
        names.entry((r.level.to_ascii_lowercase(), name.clone())).or_insert_with(|| name.clone());
        if has_any(&norm(&name), &w.to_room) && r.outline.len() >= 3 {
            let outline: Vec<Vec2> = r.outline.iter().map(|p| *p - off).collect();
            let centre = outline.iter().fold(Vec2::new(0.0, 0.0), |s, p| s + *p) * (1.0 / outline.len() as f64);
            telecom.push(TelecomRoom { level: r.level.clone(), name, centre });
        }
    }
    // A telecom room's equipment (an MDF or IDF rack, a backboard) the spec places.
    for e in sp.mep.equipment.iter().take(MAX_ROOMS) {
        let words = norm(&format!("{} {} {}", e.tag, e.kind, e.title));
        if has_any(&words, &w.to_room) && e.min.iter().chain(e.max.iter()).all(|v| v.is_finite()) {
            let centre = Vec2::new((e.min[0] + e.max[0]) / 2.0, (e.min[1] + e.max[1]) / 2.0);
            telecom.push(TelecomRoom { level: e.level.clone(), name: e.tag.trim().to_ascii_uppercase(), centre });
        }
    }
    let text = |v: &Value, k: &str| v.get(k).and_then(Value::as_str).map(str::trim).unwrap_or_default().to_string();
    let units: Vec<(String, String, String)> = sp
        .unit_instances
        .as_array()
        .into_iter()
        .flatten()
        .take(MAX_ROOMS)
        .map(|u| {
            let ty = text(u, "unit_type");
            (text(u, "number").to_ascii_uppercase(), text(u, "level").to_ascii_lowercase(), if ty.is_empty() { text(u, "type") } else { ty })
        })
        .filter(|(n, _, t)| !n.is_empty() && !t.is_empty())
        .collect();
    let unit_of = |level: &str, room: &str| -> Option<String> {
        let (n, l) = (room.trim().to_ascii_uppercase(), level.trim().to_ascii_lowercase());
        let all: Vec<&(String, String, String)> = units.iter().filter(|(un, _, _)| *un == n).collect();
        let only = if all.len() == 1 { all.first() } else { None };
        all.iter().find(|(_, ul, _)| *ul == l).or_else(|| all.iter().find(|(_, ul, _)| ul.is_empty())).or(only).map(|x| x.2.clone())
    };
    let has = |text: &str, words: &[String]| words.iter().any(|w| text.contains(w.as_str()));
    let area_of = |dv: &Dev| -> &'static str {
        let room = dv.room.trim().to_ascii_uppercase();
        let name = names.get(&(dv.level.to_ascii_lowercase(), room.clone())).cloned().unwrap_or_else(|| room.clone());
        if unit_of(&dv.level, &room).is_some() || has(&name, &rl.methods.unit_words) {
            "unit"
        } else if has(&name, &rl.methods.exposed_rooms) {
            "exposed"
        } else {
            "common"
        }
    };
    // -- Whose the pathways are (the scope split): ours unless another trade's conduit. --
    let decisions = scope::decisions(d);
    let lv_dec = decisions.iter().find(|x| x.class == "low voltage");
    let (raceway_ours, raceway_scope) = match lv_dec {
        Some(dec) if dec.by_others() => {
            let ours = dec.ours_for("low voltage outlet", "").iter().any(|i| i.item == "conduit");
            (ours, format!("{} ({}); our part: {}", dec.label, dec.party_label(), dec.ours_source()))
        }
        Some(dec) => (true, format!("{} ({})", dec.label, dec.party_label())),
        None => (true, "no scope reading: the electrical trade's (rules default)".to_string()),
    };
    let ours_party = scope::rules().ours.clone();
    let firestop_ours = if docs.firestop_by.is_empty() { lr.firestop.ours } else { docs.firestop_by == ours_party };
    let firestop_scope = if docs.firestop_by.is_empty() {
        lr.firestop.ours_source.clone()
    } else {
        format!("{} ({})", scope::rules().label(&docs.firestop_by), docs.firestop_source)
    };
    let jhooks_ours = if docs.jhooks_by.is_empty() { lr.jhooks.ours } else { docs.jhooks_by == ours_party };
    let jhook_spacing = docs.jhook_spacing_ft.unwrap_or(lr.jhooks.spacing_ft).max(1.0);
    let jhook_src = if docs.jhook_spacing_ft.is_some() { docs.jhook_spacing_source.clone() } else { lr.jhooks.spacing_source.clone() };

    // -- Each outlet's choice, then its route. --
    let kinds_of = |words: &str| -> Vec<String> {
        let n = norm(&words.replace('_', " "));
        w.kind_words.iter().filter(|(_, ws)| has_any(&n, ws)).map(|(k, _)| k.clone()).collect()
    };
    let level_ceiling = |level: &str, floor: f64| -> f64 {
        let c = sp.level(level).map_or(0.0, |l| l.ceiling_ft);
        floor + if c > 0.0 { c } else { rl.route.default_ceiling_ft } * upf
    };
    let mut paths: Vec<Path> = Vec::new();
    let mut penetrations: Vec<Value> = Vec::new();
    let mut sleeves: Vec<Value> = Vec::new();
    let mut clashes = 0usize;
    let mut unrouted: Vec<Value> = Vec::new();
    for (di, words) in &list {
        let Some(dv) = cx.devs.get(*di) else { continue };
        let kinds = kinds_of(words);
        let area = area_of(dv);
        let ceil = (cx.box_ceiling)(&dv.level, dv.at, dv.face);
        let (floor, _) = (cx.tiers_of)(&dv.level, ceil);
        let ceiling_z = ceil.unwrap_or_else(|| level_ceiling(&dv.level, floor));
        let at_ceiling = ceiling_z - dv.top <= lr.geometry.at_ceiling_in.max(0.0) / 12.0 * upf;
        let choice = choose(&docs, &kinds, area, at_ceiling);
        paths.push(Path {
            dev: *di,
            level: dv.level.clone(),
            room: dv.room.trim().to_string(),
            area,
            kinds,
            choice,
            ends: String::new(),
            runs: Vec::new(),
            ft: 0.0,
            flat_ft: 0.0,
            turns: 0,
            open_ends: 0,
            note: String::new(),
        });
    }
    // One pathway per room where the documents say: the room's other outlets join the one
    // nearest where it goes (box to box, over the ceiling).
    let mut lead_of: HashMap<usize, usize> = HashMap::new();
    {
        let mut groups: BTreeMap<(String, String), Vec<usize>> = BTreeMap::new();
        for (pi, p) in paths.iter().enumerate() {
            if p.choice.one_per_room && !p.room.is_empty() && matches!(p.choice.to.as_str(), "corridor" | "room" | "sleeve" | "enclosure") {
                groups.entry((p.level.to_ascii_lowercase(), p.room.to_ascii_uppercase())).or_default().push(pi);
            }
        }
        for (_, g) in groups.into_iter().filter(|(_, g)| g.len() > 1) {
            let reach = |pi: &usize| -> f64 {
                let Some(dv) = paths.get(*pi).and_then(|p| cx.devs.get(p.dev)) else { return f64::INFINITY };
                (cx.corridors)(&dv.level).iter().map(|r| manhattan(dv.at, clamp_in(dv.at, r, 0.0))).fold(f64::INFINITY, f64::min)
            };
            let Some(lead) = g.iter().copied().min_by(|a, b| reach(a).total_cmp(&reach(b))) else { continue };
            for pi in g.into_iter().filter(|p| *p != lead) {
                lead_of.insert(pi, lead);
            }
        }
    }
    for pi in 0..paths.len() {
        let Some(p) = paths.get(pi) else { continue };
        let Some(dv) = cx.devs.get(p.dev) else { continue };
        let field = cx.fields.get(&dv.level.to_ascii_lowercase()).unwrap_or(cx.empty);
        let size = p.choice.size_in.clamp(0.5, 4.0);
        let r = size / 24.0 * upf;
        let raceway = lr.is_raceway(&p.choice.method);
        let id = format!("{TAG_PREFIX}{}", dv.handle.hex());
        let label = if p.room.is_empty() { format!("LV {}", dv.handle.hex()) } else { format!("LV {}", p.room) };
        let ceil = (cx.box_ceiling)(&dv.level, dv.at, dv.face);
        let (floor, tiers) = (cx.tiers_of)(&dv.level, ceil);
        let ceiling_z = ceil.unwrap_or_else(|| level_ceiling(&dv.level, floor));
        let at_ceiling = ceiling_z - dv.top <= lr.geometry.at_ceiling_in.max(0.0) / 12.0 * upf;
        let mut runs: Vec<MepRun> = Vec::new();
        let ends: String;
        let mut note = String::new();
        let mut open = 0usize;
        let mut legs: Vec<Leg> = Vec::new();
        let mk = |pts: Vec<[f64; 2]>, bot: Vec<f64>, n: usize| -> MepRun {
            let mut run = super::super::printcond::run(LV_SYSTEM, &dv.level, "lv pathway", pts, size, 0.0);
            run.bottoms_ft = bot;
            run.id = format!("{id}#{n}");
            run.size_source = p.choice.size_source.chars().take(300).collect();
            run.height_source = ROUTED.into();
            run
        };
        // A stub straight up from the box into the ceiling space.
        let stub = |runs: &mut Vec<MepRun>| {
            let z = ceiling_z + lr.geometry.stub_above_ceiling_in.max(0.0) / 12.0 * upf;
            let mut run = mk(vec![[dv.at.x, dv.at.y]], vec![(dv.top - floor) / upf], runs.len() + 1);
            run.top_ft = Some((z - floor) / upf);
            runs.push(run);
        };
        // From the box over the ceiling to `target` (plan point), seated at the tiers; by the
        // corridors' lanes when `by_corridor`. A leg's heights are its segments' (one fewer than
        // its points), so legs join end to end.
        let over_to = |target: Vec2, by_corridor: bool, legs: &mut Vec<Leg>| -> (Vec<[f64; 2]>, Vec<f64>, f64) {
            let over =
                [ceil, (cx.ceiling_at)(&dv.level, target)].into_iter().flatten().fold(None, |a: Option<f64>, z| Some(a.map_or(z, |a| a.max(z))));
            let (floor, tiers) = (cx.tiers_of)(&dv.level, over);
            let z0 = tiers.first().copied().unwrap_or(ceiling_z);
            let (qa, ja, _) = rise_point(dv.at, dv.top, dv.face, z0, r, field, upf, &[], rl);
            let rects = if by_corridor { (cx.corridors)(&dv.level) } else { Vec::new() };
            let way = (!rects.is_empty()).then(|| corridor_path(&rects, qa, target, upf)).flatten().unwrap_or_else(|| vec![qa, target]);
            let mut leg = Leg::default();
            let mut prev = qa;
            for q in way.iter().skip(1).take(400) {
                let mut l = route_leg(prev, *q, &tiers, r, field, upf, &[], rl);
                if l.pts.len() < 2 || l.z.len() + 1 != l.pts.len() {
                    l = Leg { pts: vec![prev, *q], z: vec![z0], ..Leg::default() };
                }
                if leg.pts.is_empty() {
                    leg.pts = l.pts;
                } else {
                    leg.pts.extend(l.pts.into_iter().skip(1));
                }
                leg.z.extend(l.z);
                leg.blocked.extend(l.blocked);
                leg.holes += l.holes;
                prev = *q;
            }
            let (pts, bot) = from_box(dv.at, dv.top, (qa, ja), &leg, floor, upf);
            legs.push(leg);
            (pts, bot, floor)
        };
        if let Some(lead) = lead_of.get(&pi).and_then(|l| paths.get(*l)).and_then(|l| cx.devs.get(l.dev)) {
            // Joined to the room's pathway: box to box over the ceiling.
            let z0 = tiers.first().copied().unwrap_or(ceiling_z);
            let (qa, ja, _) = rise_point(dv.at, dv.top, dv.face, z0, r, field, upf, &[], rl);
            let (qb, jb, _) = rise_point(lead.at, lead.top, lead.face, z0, r, field, upf, &[], rl);
            let leg = route_leg(qa, qb, &tiers, r, field, upf, &[], rl);
            let (mut pts, mut bot) = from_box(dv.at, dv.top, (qa, ja), &leg, floor, upf);
            to_box(&mut pts, &mut bot, lead.at, lead.top, (qb, jb), floor, upf);
            runs.push(mk(pts, bot, 1));
            legs.push(leg);
            ends = "joined to the room's pathway".into();
        } else {
            match p.choice.to.as_str() {
                "corridor" | "sleeve" => {
                    let rects = (cx.corridors)(&dv.level);
                    let into = lr.geometry.into_corridor_ft.max(0.0) * upf;
                    let target =
                        rects.iter().map(|rc| clamp_in(dv.at, rc, into)).min_by(|a, b| manhattan(dv.at, *a).total_cmp(&manhattan(dv.at, *b)));
                    let in_corridor = rects.iter().any(|rc| dv.at.x >= rc[0][0] && dv.at.x <= rc[1][0] && dv.at.y >= rc[0][1] && dv.at.y <= rc[1][1]);
                    match target.filter(|_| !in_corridor) {
                        Some(t) => {
                            let (mut pts, mut bot, _) = over_to(t, false, &mut legs);
                            // A sleeve end: the raceway stops at the first rated wall it meets.
                            let cut =
                                if p.choice.to == "sleeve" { legs.last().and_then(|leg| to_first_rated(leg, cx.rated, &dv.level)) } else { None };
                            if let Some((cut_pts, wall)) = cut
                                && let Some(leg) = legs.last_mut()
                            {
                                let keep = pts.len().saturating_sub(leg.pts.len());
                                pts.truncate(keep);
                                bot.truncate(keep);
                                pts.extend(cut_pts.iter().map(|q| [q.x, q.y]));
                                bot.extend(leg.z.iter().take(cut_pts.len().saturating_sub(1)).map(|z| (z - floor) / upf));
                                leg.z.truncate(cut_pts.len().saturating_sub(1));
                                leg.pts = cut_pts.clone();
                                let at = cut_pts.last().copied().unwrap_or(t);
                                if sleeves.len() < MAX_ROWS {
                                    sleeves.push(json!({
                                        "outlet": dv.handle.hex(), "level": dv.level, "room": p.room, "wall": wall.handle.hex(), "wall_type": wall.wall_type, "rating": wall.rating,
                                        "size": super::super::mepatoms::trade_size(size + lr.sleeve.over_in.max(0.0)), "at": [r2(at.x), r2(at.y)],
                                        "item": lr.sleeve.item, "source": format!("{}; {}", p.choice.to_source, lr.sleeve.source),
                                    }));
                                }
                                ends = "sleeve through the rated wall".into();
                            } else {
                                ends = "corridor ceiling".into();
                            }
                            runs.push(mk(pts, bot, 1));
                            open += 1;
                        }
                        None => {
                            note = if in_corridor {
                                "in the corridor already: stubbed to its ceiling".into()
                            } else {
                                "no corridor on its level: stubbed to the ceiling above".into()
                            };
                            if at_ceiling {
                                ends = "in the ceiling space".into();
                            } else {
                                stub(&mut runs);
                                open += 1;
                                ends = "accessible ceiling".into();
                            }
                        }
                    }
                }
                "room" => {
                    let here = telecom
                        .iter()
                        .filter(|t| same_level(&t.level, &dv.level))
                        .min_by(|a, b| manhattan(dv.at, a.centre).total_cmp(&manhattan(dv.at, b.centre)));
                    let elev = |l: &str| sp.level(l).map_or(0.0, |x| x.elevation_ft);
                    let other = || {
                        let e0 = elev(&dv.level);
                        telecom
                            .iter()
                            .filter(|t| !same_level(&t.level, &dv.level))
                            .min_by(|a, b| (elev(&a.level) - e0).abs().total_cmp(&(elev(&b.level) - e0).abs()))
                    };
                    match here.or_else(other) {
                        Some(tr) => {
                            let target = (cx.carry)(tr.centre, &tr.level, &dv.level);
                            let far = manhattan(dv.at, target) >= rl.route.corridor_min_ft * upf;
                            let (pts, bot, floor) = over_to(target, far, &mut legs);
                            let last_z = bot.last().copied().unwrap_or(0.0);
                            runs.push(mk(pts, bot, 1));
                            if !same_level(&tr.level, &dv.level) {
                                // A riser at the room's place through the floors between, up or down to
                                // the other storey's ceiling space.
                                let other_floor = elev(&tr.level) * upf;
                                let (_, otiers) = (cx.tiers_of)(&tr.level, None);
                                let oz = otiers.first().copied().unwrap_or(other_floor + rl.route.default_ceiling_ft * upf);
                                let (lo, hi) = (last_z.min((oz - floor) / upf), last_z.max((oz - floor) / upf));
                                let mut riser = mk(vec![[target.x, target.y]], vec![lo], 2);
                                riser.top_ft = Some(hi);
                                runs.push(riser);
                                let (z_lo, z_hi) = (floor / upf + lo, floor / upf + hi);
                                let mut levels: Vec<_> = sp.levels.iter().filter(|l| l.sheet.is_none()).collect();
                                levels.sort_by(|a, b| a.elevation_ft.total_cmp(&b.elevation_ft));
                                for l in levels.iter().filter(|l| l.elevation_ft > z_lo && l.elevation_ft <= z_hi) {
                                    if penetrations.len() >= MAX_ROWS {
                                        break;
                                    }
                                    penetrations.push(json!({
                                        "floor": l.name, "rating": "not given", "level": dv.level, "circuit": label, "raceway_in": size,
                                        "at": [r2(target.x), r2(target.y)],
                                        "firestop": format!("{} for {} pathway through the {} floor: rating {}", lr.firestop.item, super::super::mepatoms::trade_size(size), l.name, lr.firestop.floor_unrated),
                                        "source": lr.firestop.source,
                                    }));
                                }
                            }
                            open += 1;
                            ends = format!("{} ({})", tr.name.to_ascii_lowercase(), tr.level);
                        }
                        None => {
                            note = "no telecom room (MDF, IDF) in the model: stubbed to the ceiling above".into();
                            if at_ceiling {
                                ends = "in the ceiling space".into();
                            } else {
                                stub(&mut runs);
                                open += 1;
                                ends = "accessible ceiling".into();
                            }
                        }
                    }
                }
                "enclosure" => {
                    let encl = cx
                        .devs
                        .iter()
                        .filter(|o| o.handle != dv.handle && same_level(&o.level, &dv.level) && o.room.trim().eq_ignore_ascii_case(dv.room.trim()))
                        .find(|o| {
                            let n = norm(&device_words(d, o).replace('_', " "));
                            has_any(&n, &lr.kinds.enclosure_words)
                        });
                    match encl {
                        Some(e) => {
                            let z0 = tiers.first().copied().unwrap_or(ceiling_z);
                            let (qa, ja, _) = rise_point(dv.at, dv.top, dv.face, z0, r, field, upf, &[], rl);
                            let (qb, jb, _) = rise_point(e.at, e.top, e.face, z0, r, field, upf, &[], rl);
                            let leg = route_leg(qa, qb, &tiers, r, field, upf, &[], rl);
                            let (mut pts, mut bot) = from_box(dv.at, dv.top, (qa, ja), &leg, floor, upf);
                            to_box(&mut pts, &mut bot, e.at, e.top, (qb, jb), floor, upf);
                            runs.push(mk(pts, bot, 1));
                            legs.push(leg);
                            ends = "the unit's low-voltage enclosure".into();
                        }
                        None => {
                            note = "no low-voltage enclosure in its room: stubbed to the ceiling above".into();
                            if at_ceiling {
                                ends = "in the ceiling space".into();
                            } else {
                                stub(&mut runs);
                                open += 1;
                                ends = "accessible ceiling".into();
                            }
                        }
                    }
                }
                _ => {
                    if at_ceiling {
                        ends = "in the ceiling space".into();
                    } else {
                        stub(&mut runs);
                        open += 1;
                        ends = "accessible ceiling".into();
                    }
                }
            }
        }
        // The rated walls the pathway crosses: a raceway's penetration (firestopped), or the
        // open cable's sleeve.
        for leg in &legs {
            clashes += leg.blocked.len();
            if raceway {
                penetrate(&mut penetrations, cx.rated, &dv.level, leg, &label, size, upf, floor, rl);
            } else {
                for wseg in leg.pts.windows(2) {
                    let [a, b] = wseg else { continue };
                    for wall in cx.rated.iter().filter(|x| same_level(&x.level, &dv.level)) {
                        if sleeves.len() < MAX_ROWS && crosses(*a, *b, &wall.hull) {
                            sleeves.push(json!({
                                "outlet": dv.handle.hex(), "level": dv.level, "room": p.room, "wall": wall.handle.hex(), "wall_type": wall.wall_type, "rating": wall.rating,
                                "size": super::super::mepatoms::trade_size(size + lr.sleeve.over_in.max(0.0)), "at": [r2((a.x + b.x) / 2.0), r2((a.y + b.y) / 2.0)],
                                "item": lr.sleeve.item, "source": lr.sleeve.source,
                            }));
                        }
                    }
                }
            }
        }
        let ft: f64 = runs.iter().map(|x| run_ft(x, upf)).sum();
        let flat_ft: f64 = legs.iter().map(|l| poly_len(&l.pts) / upf).sum();
        let turns: usize = legs.iter().map(|l| l.pts.len().saturating_sub(2)).sum();
        if let Some(p) = paths.get_mut(pi) {
            p.ends = ends;
            p.note = note;
            p.ft = ft;
            p.flat_ft = flat_ft;
            p.turns = turns;
            p.open_ends = open;
            p.runs = runs;
        }
    }

    // -- The runs and tags into the spec (ours, raceway); the report. --
    let mut by_method: BTreeMap<String, f64> = BTreeMap::new();
    let mut by_level: BTreeMap<String, (usize, f64, BTreeMap<String, f64>)> = BTreeMap::new();
    let mut by_room: BTreeMap<(String, String), (usize, f64, BTreeMap<String, f64>)> = BTreeMap::new();
    let mut ends_n: BTreeMap<String, usize> = BTreeMap::new();
    let (mut brackets, mut stub_ends, mut jhooks, mut pull_ft, mut built, mut theirs) = (0usize, 0usize, 0usize, 0.0_f64, 0usize, 0usize);
    let mut rows: Vec<Value> = Vec::new();
    let mut conflicts: Vec<Value> = Vec::new();
    let mut notes: BTreeMap<String, usize> = BTreeMap::new();
    for p in &paths {
        let Some(dv) = cx.devs.get(p.dev) else { continue };
        let raceway = lr.is_raceway(&p.choice.method);
        let key = format!("{} {}", p.choice.method, super::super::mepatoms::trade_size(p.choice.size_in));
        let ends_key = if p.ends.starts_with("joined") {
            "joined"
        } else if p.ends.starts_with("corridor") {
            "corridor"
        } else if p.ends.starts_with("sleeve") {
            "sleeve"
        } else if p.ends.starts_with("accessible") {
            "ceiling"
        } else if p.ends.starts_with("in the ceiling") {
            "in ceiling"
        } else if p.ends.starts_with("the unit") {
            "enclosure"
        } else {
            "telecom room"
        };
        *ends_n.entry(ends_key.into()).or_default() += 1;
        if !p.note.is_empty() {
            *notes.entry(p.note.clone()).or_default() += 1;
        }
        if raceway {
            *by_method.entry(key.clone()).or_default() += p.ft;
        } else {
            *by_method.entry(p.choice.method.clone()).or_default() += p.ft;
            jhooks += if p.flat_ft > 0.0 { (p.flat_ft / jhook_spacing).ceil() as usize + p.turns } else { 0 };
        }
        let lv = by_level.entry(p.level.clone()).or_default();
        lv.0 += 1;
        lv.1 += p.ft;
        *lv.2.entry(key.clone()).or_default() += p.ft;
        let rm = by_room.entry((p.level.clone(), p.room.clone())).or_default();
        rm.0 += 1;
        rm.1 += p.ft;
        *rm.2.entry(key.clone()).or_default() += p.ft;
        brackets += usize::from(p.choice.bracket);
        if raceway {
            stub_ends += p.open_ends;
            if p.ft > 0.0 {
                pull_ft += p.ft + 2.0 * lr.fittings.pull_string_extra_ft.max(0.0);
            }
        }
        for c in p.choice.conflicts.iter().take(2) {
            if conflicts.len() < 50 && !conflicts.iter().any(|x| x["what"] == json!(c)) {
                conflicts.push(json!({"what": c, "outlet": dv.handle.hex(), "level": p.level, "room": p.room}));
            }
        }
        let devid = d.model.get(dv.handle).and_then(|e| super::super::bimlook::made_tag(e, "devid")).unwrap_or_default().to_string();
        if rows.len() < MAX_ROWS {
            rows.push(json!({
                "outlet": dv.handle.hex(), "devid": devid, "kind": dv.kind, "kinds": p.kinds, "level": p.level, "room": p.room, "area": p.area,
                "method": p.choice.method, "method_source": p.choice.method_source, "size_in": p.choice.size_in, "size_source": p.choice.size_source,
                "to": p.choice.to, "to_source": p.choice.to_source, "ends": p.ends, "ft": r1(p.ft), "bracket": p.choice.bracket, "bracket_source": p.choice.bracket_source,
                "one_per_room": p.choice.one_per_room, "note": p.note,
            }));
        }
        // The outlet's tag: what the pieces are made of (its method), where it ends.
        let mut t = ElementTag {
            id: format!("{TAG_PREFIX}{}", dv.handle.hex()),
            kind: TAG_KIND.into(),
            level: p.level.clone(),
            at_ft: [dv.at.x / upf, dv.at.y / upf],
            ..ElementTag::default()
        };
        let src_kind = |s: &str| if s.starts_with("rules default") || s.starts_with("company default") { "default" } else { "note" };
        t.set("conduit", p.choice.method.clone(), src_kind(&p.choice.method_source));
        t.set("method_source", p.choice.method_source.chars().take(400).collect::<String>(), "computed");
        t.set("size", super::super::mepatoms::trade_size(p.choice.size_in), src_kind(&p.choice.size_source));
        t.set("to", p.choice.to.clone(), src_kind(&p.choice.to_source));
        t.set("ends", p.ends.clone(), "computed");
        t.set("device", dv.handle.hex(), "computed");
        t.set("devid", devid, "computed");
        t.set("room", p.room.clone(), "computed");
        t.set("area", p.area, "computed");
        t.set("bracket", if p.choice.bracket { "yes" } else { "no" }, src_kind(&p.choice.bracket_source));
        t.set("bracket_source", p.choice.bracket_source.chars().take(400).collect::<String>(), "computed");
        t.set("route", ROUTED, "computed");
        let ours = raceway_ours && raceway;
        t.set("scope", if ours { "ours" } else { "by others" }, "computed");
        t.set("built", if ours && !p.runs.is_empty() { "yes" } else { "no" }, "computed");
        if ours && !p.runs.is_empty() {
            built += 1;
            out.runs.extend(p.runs.iter().cloned());
        } else if !p.runs.is_empty() || !raceway {
            theirs += 1;
        }
        if p.runs.is_empty() && !p.ends.starts_with("in the ceiling") && unrouted.len() < MAX_ROWS {
            unrouted.push(json!({"outlet": dv.handle.hex(), "level": p.level, "room": p.room, "reason": if p.note.is_empty() { "no pathway routed".to_string() } else { p.note.clone() }}));
        }
        out.tags.push(t);
    }
    // By unit type (the spec's unit instances), each typical room, each common area.
    let mut types: BTreeMap<String, (usize, usize, f64, BTreeMap<String, f64>)> = BTreeMap::new();
    let mut typical: Vec<Value> = Vec::new();
    let mut common: Vec<Value> = Vec::new();
    for ((level, room), (n, ft, m)) in &by_room {
        let mut v = json!({"level": level, "room": if room.is_empty() { "(no room)" } else { room.as_str() }, "outlets": n, "ft": r1(*ft),
            "by_method_size": m.iter().map(|(k, x)| (k.clone(), json!(r1(*x)))).collect::<serde_json::Map<_, _>>()});
        match (!room.is_empty()).then(|| unit_of(level, room)).flatten() {
            Some(ty) => {
                let e = types.entry(ty.clone()).or_default();
                e.0 += 1;
                e.1 += n;
                e.2 += ft;
                for (k, x) in m {
                    *e.3.entry(k.clone()).or_default() += x;
                }
                v["unit_type"] = json!(ty);
                if typical.len() < MAX_ROWS {
                    typical.push(v);
                }
            }
            None if common.len() < MAX_ROWS => common.push(v),
            None => {}
        }
    }
    let unit_types: Vec<Value> = types
        .iter()
        .map(|(ty, (rooms, n, ft, m))| {
            let per = (*rooms).max(1) as f64;
            json!({"unit_type": ty, "rooms": rooms, "outlets_per_room": r1(*n as f64 / per), "ft_per_room": r1(ft / per), "ft": r1(*ft),
                "by_method_size_per_room": m.iter().map(|(k, x)| (k.clone(), json!(r1(x / per)))).collect::<serde_json::Map<_, _>>()})
        })
        .collect();
    let levels: Vec<Value> = by_level
        .iter()
        .map(|(l, (n, ft, m))| {
            json!({"level": l, "outlets": n, "ft": r1(*ft), "by_method_size": m.iter().map(|(k, x)| (k.clone(), json!(r1(*x)))).collect::<serde_json::Map<_, _>>()})
        })
        .collect();
    let total_ft: f64 = paths.iter().map(|p| p.ft).sum();
    let firestops = penetrations.len() + sleeves.len();
    out.report = json!({
        "outlets": paths.len(),
        "built": built,
        "by_others": theirs,
        "ft": r1(total_ft),
        "by_method_size": by_method.iter().map(|(k, v)| (k.clone(), json!(r1(*v)))).collect::<serde_json::Map<_, _>>(),
        "ends": ends_n,
        "levels": levels,
        "unit_types": unit_types,
        "typical_rooms": typical,
        "common_areas": common,
        "fittings": {
            "stub_end_bushings": stub_ends, "stub_end_item": lr.fittings.stub_end, "stub_end_source": lr.fittings.stub_end_source,
            "pull_string_ft": r1(pull_ft), "pull_string_item": lr.fittings.pull_string, "pull_string_source": lr.fittings.pull_string_source,
            "note": "sticks, couplings, connectors and straps are the pathway's pieces in the model (bim.count {system: LV-PATHWAY}, conduit.takeoff)",
        },
        "brackets": {"count": brackets, "item": lr.bracket.item, "size": lr.bracket.size},
        "boxes": paths.len().saturating_sub(brackets),
        "sleeves": sleeves,
        "penetrations": penetrations,
        "firestop": {"count": firestops, "ours": firestop_ours, "scope": firestop_scope, "item": lr.firestop.item, "source": lr.firestop.source},
        "jhooks": {"count": jhooks, "spacing_ft": jhook_spacing, "spacing_source": jhook_src, "ours": jhooks_ours,
                   "scope": if docs.jhooks_by.is_empty() { lr.jhooks.ours_source.clone() } else { docs.jhooks_source.clone() }, "item": lr.jhooks.item},
        "raceway_scope": {"ours": raceway_ours, "source": raceway_scope},
        "clashes": clashes,
        "unrouted": unrouted,
        "notes": notes,
        "conflicts": conflicts,
        "documents": {
            "statements": docs.statements.len(), "documents": docs.docs_read,
            "minimum_size_in": docs.min_size_in, "minimum_source": docs.min_size_source,
            "read": docs.statements.iter().take(60).map(|s| json!({"cite": s.cite(), "kinds": s.kinds, "areas": s.areas, "application": s.application,
                "methods": s.methods, "generic": s.generic, "size_in": s.size_in, "minimum": s.minimum, "to": s.to, "one_per_room": s.one_per_room,
                "bracket": s.bracket, "lv_doc": s.lv_doc})).collect::<Vec<_>>(),
        },
        "outlet_list": rows,
        "source": format!("{}; {}", lr.statements.source, lr.geometry.source),
        "system": LV_SYSTEM,
        "branch_system": BRANCH_SYSTEM,
    });
    out
}

/// The point of rectangle `rc` nearest `p`, `inset` into it from the side it is reached by (not
/// past its middle).
fn clamp_in(p: Vec2, rc: &[[f64; 2]; 2], inset: f64) -> Vec2 {
    let (lo, hi) = (Vec2::new(rc[0][0].min(rc[1][0]), rc[0][1].min(rc[1][1])), Vec2::new(rc[0][0].max(rc[1][0]), rc[0][1].max(rc[1][1])));
    let mid = (lo + hi) * 0.5;
    let x = p.x.clamp(lo.x, hi.x);
    let y = p.y.clamp(lo.y, hi.y);
    let push = |v: f64, l: f64, h: f64, m: f64| {
        if (v - l).abs() < 1e-9 {
            (v + inset).min(m)
        } else if (v - h).abs() < 1e-9 {
            (v - inset).max(m)
        } else {
            v
        }
    };
    let outside_x = p.x < lo.x || p.x > hi.x;
    let outside_y = p.y < lo.y || p.y > hi.y;
    Vec2::new(if outside_x { push(x, lo.x, hi.x, mid.x) } else { x }, if outside_y { push(y, lo.y, hi.y, mid.y) } else { y })
}

/// A seated leg's points up to where it first meets a rated wall of its level (that wall), if it
/// meets one.
fn to_first_rated<'a>(leg: &Leg, rated: &'a [Rated], level: &str) -> Option<(Vec<Vec2>, &'a Rated)> {
    let mut pts: Vec<Vec2> = leg.pts.first().copied().into_iter().collect();
    for wseg in leg.pts.windows(2) {
        let [a, b] = wseg else { continue };
        if let Some(wall) = rated.iter().filter(|x| same_level(&x.level, level)).find(|x| crosses(*a, *b, &x.hull)) {
            // The first point of the piece inside the wall's outline (64 steps, then halved).
            let steps = 64usize;
            let mut hit = None;
            for k in 1..=steps {
                let q = *a + (*b - *a) * (k as f64 / steps as f64);
                if inside(q, &wall.hull) {
                    hit = Some(k);
                    break;
                }
            }
            let k = hit.unwrap_or(steps / 2);
            let (mut lo, mut hi) = ((k - 1) as f64 / steps as f64, k as f64 / steps as f64);
            for _ in 0..20 {
                let m = (lo + hi) / 2.0;
                if inside(*a + (*b - *a) * m, &wall.hull) {
                    hi = m;
                } else {
                    lo = m;
                }
            }
            pts.push(*a + (*b - *a) * lo);
            return Some((pts, wall));
        }
        pts.push(*b);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cmd::scope::{Doc, Para};
    use cadaddy_doc::projrules::SourceTier;
    use serde_json::json;

    fn doc(name: &str, trade: Option<&str>, tier: SourceTier, paras: &[&str]) -> Doc {
        Doc {
            name: name.into(),
            tier,
            trade: trade.map(str::to_string),
            paras: paras
                .iter()
                .enumerate()
                .map(|(i, t)| Para { at: format!("note {}", i + 1), heading: String::new(), text: (*t).into(), cells: Vec::new() })
                .collect(),
        }
    }

    #[test]
    fn sizes_are_read_in_every_way_the_documents_write_them() {
        let got: Vec<f64> = sizes("PROVIDE 3/4\" CONDUIT, A 1-INCH SLEEVE, 1-1/4 IN. EMT AND 2'' PVC; 12 BOXES").into_iter().map(|x| x.0).collect();
        assert_eq!(got, vec![0.75, 1.0, 1.25, 2.0]);
        assert_eq!(sizes("Minimum raceway size is 3/4-inch (21-mm) for copper.").first().map(|x| x.0), Some(0.75));
        assert!(sizes("A 12\" X 12\" HATCH").is_empty(), "not a trade size");
        // An abbreviation's full stop doesn't end the sentence.
        assert_eq!(sentences("PROVIDE 3/4\" MIN. CONDUIT TO THE CEILING. LABEL BOTH ENDS.").len(), 2);
    }

    #[test]
    fn statements_say_method_size_end_and_subject_and_the_lv_documents_govern() {
        let docs = vec![
            doc(
                "E-901",
                Some("electrical"),
                SourceTier::DrawingRevision,
                &[
                    "LOW VOLTAGE DEVICES ARE ROUGH-IN ONLY: EVERY OUTLET GETS A BACK BOX WITH A 3/4\" CONDUIT MINIMUM STUB-UP TO ABOVE THE ACCESSIBLE CEILING.",
                    "EACH GUEST ROOM: 1\" CONDUIT FOR LOW VOLTAGE, BOX TO THE CORRIDOR, A PULL LINE IN IT.",
                    "RECEPTACLES ON 20A CIRCUITS IN 3/4\" EMT.",
                ],
            ),
            doc(
                "T-901",
                Some("low voltage"),
                SourceTier::DrawingRevision,
                &[
                    "ONE PATHWAY PER GUEST ROOM: 3/4\" MINIMUM CONDUIT OR SMURF TUBE FROM THE DATA OUTLET AT THE DESK, ENDING IN THE CORRIDOR CEILING.",
                    "CAMERAS OUTSIDE THE CORRIDOR NEED A PATHWAY OVER THE CEILING TO THE GUEST ROOM CORRIDOR. USE 3/4\" CONDUIT.",
                    "PROVIDE TWO 3\" CONDUITS FROM THE CORRIDOR TO THE MDF ROOM.",
                    "THE ELECTRICAL CONTRACTOR FURNISHES BOXES AND RACEWAYS FOR DATA. FIRESTOPPING OF THE CABLE PENETRATIONS IS BY THE LOW VOLTAGE CONTRACTOR.",
                ],
            ),
            doc(
                "27 05 28",
                Some("low voltage"),
                SourceTier::Spec,
                &[
                    "Concealed in walls and ceilings: EMT.",
                    "Minimum raceway size is 3/4-inch (21-mm) for copper cabling.",
                    "Hang cable hooks 5 feet apart at most, and one at each turn.",
                ],
            ),
        ];
        let r = read(&docs);
        // The backbone conduit (no outlet subject) and the power note are not statements.
        assert!(r.statements.iter().all(|s| !s.text.contains("3\" CONDUITS") && !s.text.contains("RECEPTACLES")), "{:#?}", r.statements);
        assert_eq!(r.min_size_in, Some(0.75));
        assert_eq!(r.jhook_spacing_ft, Some(5.0));
        assert_eq!(r.firestop_by, "low voltage", "{}", r.firestop_source);
        // A guest room's data outlet: the low-voltage sheet's statement (more specific, its own
        // trade's) over the electrical note's 1".
        let c = choose(&r, &["data".to_string()], "unit", false);
        assert_eq!((c.method.as_str(), c.size_in, c.to.as_str(), c.one_per_room), ("ENT", 0.75, "corridor", true), "{c:?}");
        assert!(c.method_source.contains("T-901"), "{}", c.method_source);
        // A guest room's TV outlet: the room-wide note (1", corridor; no material: the spec's
        // concealed method).
        let tv = choose(&r, &["tv".to_string()], "unit", false);
        assert_eq!((tv.method.as_str(), tv.size_in, tv.to.as_str()), ("EMT", 1.0, "corridor"), "{tv:?}");
        // An office outlet: the legend note's stub into the ceiling, the spec's EMT.
        let o = choose(&r, &[], "common", false);
        assert_eq!((o.method.as_str(), o.size_in, o.to.as_str()), ("EMT", 0.75, "ceiling"), "{o:?}");
        // A camera: to the corridor, its own note.
        let cam = choose(&r, &["camera".to_string()], "common", true);
        assert_eq!(cam.to, "corridor", "{cam:?}");
        // Nothing read: the rules defaults by area, each said as such.
        let none = choose(&LvDocs::default(), &[], "unit", false);
        assert_eq!((none.method.as_str(), none.size_in, none.to.as_str()), ("ENT", 0.75, "corridor"));
        assert!(none.method_source.starts_with("rules default"), "{}", none.method_source);
    }

    #[test]
    fn product_data_other_trades_specs_marked_keynotes_and_backbones_are_not_outlet_rules() {
        let docs = vec![
            // A product's data sheet and a power raceway section.
            doc("Submittal 1", Some("low voltage"), SourceTier::Submittal, &["Data cabling in 3/4\" PVC conduit to each outlet."]),
            doc("26 05 33", Some("electrical"), SourceTier::Spec, &["Concealed in walls: EMT, also for data and telephone outlets."]),
            // A communications section's paragraph that isn't an application.
            doc("27 05 44", Some("low voltage"), SourceTier::Spec, &["Sleeves for pathways through masonry-unit walls: steel pipe sleeves."]),
            doc(
                "T-902",
                Some("low voltage"),
                SourceTier::DrawingRevision,
                &[
                    // The trunk from the corridor to the telecom room.
                    "RUN A HORIZONTAL PATHWAY FROM THE CORRIDOR CEILING TO THE MDF SERVING EVERY GUEST ROOM.",
                    "FOUR 2\" CONDUITS ARE REQUIRED FOR ALL GUEST ROOMS AND OFFICES.",
                ],
            ),
        ];
        let mut marked = doc(
            "E-907",
            Some("electrical"),
            SourceTier::DrawingRevision,
            &["TELEPHONE JACK BY THE CAB CONTROLLER IN 3/4\" CONDUIT WITH A PULL LINE."],
        );
        if let Some(p) = marked.paras.first_mut() {
            p.heading = "ELEVATOR KEYNOTES".into();
        }
        let mut all = docs;
        all.push(marked);
        let r = read(&all);
        assert!(r.statements.is_empty(), "{:#?}", r.statements);
        // A word the print ran into the next still says where the pathway goes.
        let glued = read(&[doc(
            "T-903",
            Some("low voltage"),
            SourceTier::DrawingRevision,
            &["OFFICE OUTLETS: PROVIDE CONDUIT FROM EACH BOX TO ABOVE THE CORRIDORCEILING."],
        )]);
        assert_eq!(glued.statements.first().map(|s| s.to.as_str()), Some("corridor"), "{:#?}", glued.statements);
        // A typical room's keynote naming the room is its outlets' rule.
        let mut typical = doc(
            "E-908",
            Some("electrical"),
            SourceTier::DrawingRevision,
            &["1\" LV CONDUIT WITH PULL LINE FROM THE GUESTROOM DESK BOX TO THE CORRIDOR."],
        );
        if let Some(p) = typical.paras.first_mut() {
            p.heading = "ELECTRICAL KEYNOTES".into();
        }
        let r = read(&[typical]);
        assert_eq!(r.statements.len(), 1);
        assert_eq!((r.statements[0].areas.clone(), r.statements[0].to.clone()), (vec!["unit".to_string()], "corridor".to_string()));
    }

    #[test]
    fn a_misread_title_block_is_still_a_low_voltage_sheet_beside_its_series() {
        let sheets = vec![
            json!({"number": "V-1", "title": "1st Floor", "file": "set.pdf", "page": 20}),
            json!({"number": "V-2", "title": "LOW VOLTAGE PLANS - FIRST FLOOR", "file": "set.pdf", "page": 21}),
            json!({"number": "V-3", "title": "LANDSCAPE PLAN", "file": "civil.pdf", "page": 3}),
            json!({"number": "T-901", "title": "TELECOMMUNICATIONS RISER", "file": "civil.pdf", "page": 9}),
            json!({"number": "E-909", "title": "ELECTRICAL LEGEND", "file": "set.pdf", "page": 2}),
        ];
        assert_eq!(lv_sheets(&sheets), vec![0, 1, 3]);
    }

    #[test]
    fn outlets_are_told_by_their_words_not_fire_alarm_or_power() {
        assert!(is_outlet("data_outlet 2D"));
        assert!(is_outlet("low_voltage_outlet TV"));
        assert!(is_outlet("device CARD READER"));
        assert!(!is_outlet("fire_alarm_device SMOKE"));
        assert!(!is_outlet("receptacle DUPLEX"));
    }

    /// A made-up one-storey building (12' floor to floor, 9' ceiling) with `tags`, `extra` spec
    /// keys, the documents' paragraphs read and stored, built; each device's room set to `rooms`
    /// (by tag id) and the model routed again.
    fn building(tags: Vec<Value>, extra: Value, docs: Vec<Doc>, rooms: &[(&str, &str)]) -> crate::Session {
        let mut s = crate::Session::empty();
        s.open_drawing(Drawing::new_imperial(), "Sample Floor Plan", None);
        // The walls on the wall layer: the outside's two faces round `outline`, each marked
        // wall's two faces (the spec's marks give it its type).
        let walls = extra["walls"].as_array().cloned().unwrap_or_default();
        if !walls.is_empty() {
            if s.execute("layer.new", &json!({"name": "A-WALL", "current": true})).is_err() {
                s.execute("layer.current", &json!({"name": "A-WALL"})).expect("layer");
            }
            let t = 4.875;
            let rect = |s: &mut crate::Session, a: [f64; 2], b: [f64; 2]| {
                s.execute("pline", &json!({"vertices": [a, [b[0], a[1]], b, [a[0], b[1]]], "closed": true})).expect("outside");
            };
            rect(&mut s, [-t, -t], [720.0 + t, 312.0 + t]);
            rect(&mut s, [0.0, 0.0], [720.0, 312.0]);
        }
        for m in &walls {
            let (a, b) = (&m["a"], &m["b"]);
            let (ax, ay, bx, by) =
                (a[0].as_f64().unwrap_or(0.0), a[1].as_f64().unwrap_or(0.0), b[0].as_f64().unwrap_or(0.0), b[1].as_f64().unwrap_or(0.0));
            let h = 4.875 / 2.0;
            s.execute("line", &json!({"points": [[ax, ay - h], [bx, by - h]]})).expect("face");
            s.execute("line", &json!({"points": [[ax, ay + h], [bx, by + h]]})).expect("face");
        }
        let mut spec = json!({
            "version": 1, "units_per_foot": 12,
            "levels": [{"name": "L1", "elevation_ft": 0, "height_ft": 12, "ceiling_ft": 9, "base": [0, 0]}],
            "tags": tags,
        });
        if let (Some(o), Some(x)) = (spec.as_object_mut(), extra.as_object()) {
            o.extend(x.clone());
        }
        s.execute("bspec.set", &json!({"replace": true, "spec": spec})).expect("spec");
        if let Ok(d) = s.doc_mut() {
            read(&docs).store(d);
        }
        s.execute("model.build", &json!({})).expect("build");
        for (id, room) in rooms {
            let h = {
                let d = s.doc().expect("doc");
                d.model.iter().find(|e| super::super::super::bimlook::made_tag(e, "devid") == Some(*id)).map(|e| e.handle.hex())
            };
            let h = h.unwrap_or_else(|| panic!("no device built for {id}"));
            s.execute("bim.props.set", &json!({"handle": h, "key": "room", "value": room})).expect("room");
        }
        s.execute("conduit.reroute", &json!({})).expect("reroute");
        s
    }

    fn outlet(id: &str, kind: &str, x: f64, y: f64) -> Value {
        json!({"id": format!("device:L1:{id}"), "kind": kind, "level": "L1", "at_ft": [x, y], "fields": {
            "type": {"value": "D", "source": "plan"}, "symbol": {"value": format!("{:X}", 0x5A0 + id.len()), "source": "plan"},
            "sheet": {"value": "T-911", "source": "plan"}, "mount_height_in": {"value": "18", "source": "note"}}})
    }

    /// Each built device's plan point and top (sheet inches).
    fn tops(s: &crate::Session) -> Vec<[f64; 3]> {
        let d = s.doc().expect("doc");
        d.model
            .iter()
            .filter(|e| super::super::super::wallgen::made_as(e).as_deref() == Some("device"))
            .filter_map(|e| {
                let cadaddy_doc::EntityKind::Mesh(m) = &e.kind else { return None };
                let (mut lo, mut hi) = ([f64::INFINITY; 3], [f64::NEG_INFINITY; 3]);
                for v in &m.vertices {
                    for (k, x) in [v.x, v.y, v.z].into_iter().enumerate() {
                        lo[k] = lo[k].min(x);
                        hi[k] = hi[k].max(x);
                    }
                }
                Some([(lo[0] + hi[0]) / 2.0, (lo[1] + hi[1]) / 2.0, hi[2]])
            })
            .collect()
    }

    fn lv_count(s: &mut crate::Session, kind: &str) -> Value {
        s.execute("bim.count", &json!({"kind": kind, "system": LV_SYSTEM})).expect("count")
    }

    #[test]
    fn an_outlet_with_a_stub_up_note_gets_its_stub_into_the_ceiling_with_a_bushing() {
        let note = "EVERY LOW VOLTAGE OUTLET: A BACK BOX AND A 3/4\" EMT STUB-UP TO ABOVE THE ACCESSIBLE CEILING.";
        let mut s = building(
            vec![outlet("D1", "data_outlet", 20.0, 10.0)],
            json!({}),
            vec![doc("E-911", Some("electrical"), SourceTier::DrawingRevision, &[note])],
            &[("device:L1:D1", "R12")],
        );
        let r = s.execute("conduit.route", &json!({"section": "lv_pathways"})).expect("report");
        assert_eq!((r["outlets"].clone(), r["built"].clone(), r["ends"]["ceiling"].clone()), (json!(1), json!(1), json!(1)), "{r}");
        let o = &r["outlet_list"][0];
        assert_eq!(
            (o["method"].clone(), o["size_in"].clone(), o["to"].clone(), o["area"].clone()),
            (json!("EMT"), json!(0.75), json!("ceiling"), json!("common")),
            "{o}"
        );
        assert!(o["method_source"].as_str().is_some_and(|m| m.contains("E-911")), "{o}");
        // Up from the box's top to 6" over the 9' ceiling.
        let top = tops(&s).first().map(|t| t[2]).expect("device");
        let want = (9.5 * 12.0 - top) / 12.0;
        assert!(o["ft"].as_f64().is_some_and(|f| (f - want).abs() < 0.06), "{want} vs {o}");
        // Its pieces: EMT to that length, a connector at the box, the stub's insulated bushing.
        let c = lv_count(&mut s, "conduit");
        assert!(c["length_ft"].as_f64().is_some_and(|f| (f - want).abs() < 0.06), "{c}");
        assert_eq!(lv_count(&mut s, "connector")["count"], json!(1));
        let b = s.execute("bim.count", &json!({"kind": "bushing", "system": LV_SYSTEM, "group_by": "item"})).expect("count");
        assert_eq!(b["rows"][0]["item"], json!("Insulated bushing"), "{b}");
        // The takeoff has its pull string; the bill no longer counts a stub per outlet.
        let tk = s.execute("conduit.takeoff", &json!({"price": false})).expect("takeoff");
        assert!(tk["lines"].as_array().into_iter().flatten().any(|l| l["kind"] == json!("pull string")), "{tk}");
        let bom = s.execute("bim.bom", &json!({})).expect("bom");
        let items: Vec<&str> = bom["lines"].as_array().into_iter().flatten().filter_map(|l| l["item"].as_str()).collect();
        assert!(!items.contains(&"Conduit stub to the ceiling space"), "the built pathway replaces the per-outlet stub: {items:?}");
        assert!(items.contains(&"Pull string"), "{items:?}");
    }

    #[test]
    fn an_outlet_home_run_to_the_idf_is_routed_into_it() {
        let note = "ALL DATA OUTLETS: PROVIDE 1\" EMT HOME RUN TO THE IDF.";
        let mut s = building(
            vec![outlet("D2", "data_outlet", 10.0, 10.0)],
            json!({"mep": {"equipment": [{"tag": "IDF-1", "kind": "rack", "level": "L1", "min": [480, -12], "max": [504, 0], "height_ft": 7, "bottom_ft": 0}]}}),
            vec![doc("T-912", Some("low voltage"), SourceTier::DrawingRevision, &[note])],
            &[("device:L1:D2", "R14")],
        );
        let r = s.execute("conduit.route", &json!({"section": "lv_pathways"})).expect("report");
        let o = &r["outlet_list"][0];
        assert_eq!((o["method"].clone(), o["size_in"].clone(), o["to"].clone()), (json!("EMT"), json!(1.0), json!("room")), "{o}");
        assert!(o["ends"].as_str().is_some_and(|e| e.starts_with("idf-1")), "{o}");
        assert_eq!(r["ends"]["telecom room"], json!(1), "{r}");
        // Up into the ceiling space (its first tier, 3" over the 9' ceiling) and square to the
        // rack's centre.
        let t = tops(&s).first().copied().expect("device");
        let want = (111.0 - t[2]) / 12.0 + ((492.0 - t[0]).abs() + (-6.0 - t[1]).abs()) / 12.0;
        assert!(o["ft"].as_f64().is_some_and(|f| (f - want).abs() < 0.06), "{want} vs {o}");
        let c = lv_count(&mut s, "conduit");
        assert!(c["length_ft"].as_f64().is_some_and(|f| (f - want).abs() < 0.1), "{c}");
        // Its two ends: the box's connector, and the rack's (or a bushing over it).
        let ends = lv_count(&mut s, "connector")["count"].as_u64().unwrap_or(0) + lv_count(&mut s, "bushing")["count"].as_u64().unwrap_or(0);
        assert_eq!(ends, 2);
    }

    #[test]
    fn a_guest_rooms_outlet_crosses_the_rated_corridor_wall_with_its_firestop() {
        // No document says: the rules' default for a unit (3/4" ENT to the corridor's ceiling).
        let rated = json!({"tag": "P1", "description": "RATED PARTITION", "fire_rating": "1 HR", "layers": []});
        let mut s = building(
            vec![outlet("D3", "data_outlet", 10.0, 10.0)],
            json!({
                "layers": [{"layer": "A-WALL", "role": "wall", "wall_type": "P1"}],
                "wall_types": [{"name": "P1", "thickness_in": 4.875, "from": "pairs", "assembly": rated}],
                "walls": [{"level": "L1", "a": [0.0, 240.0], "b": [720.0, 240.0], "wall_type": "P1"}],
                "circulation": [{"kind": "corridor", "level": "L1", "name": "CORRIDOR", "at": [360, 276], "rects": [[[0, 240], [720, 312]]]}],
                "unit_instances": [{"number": "U7", "level": "L1", "type": "KING-A"}],
            }),
            Vec::new(),
            &[("device:L1:D3", "U7")],
        );
        let r = s.execute("conduit.route", &json!({"section": "lv_pathways"})).expect("report");
        let o = &r["outlet_list"][0];
        assert_eq!(
            (o["method"].clone(), o["size_in"].clone(), o["to"].clone(), o["area"].clone()),
            (json!("ENT"), json!(0.75), json!("corridor"), json!("unit")),
            "{o}"
        );
        assert!(o["method_source"].as_str().is_some_and(|m| m.starts_with("rules default")), "{o}");
        // Up into the ceiling space and across into the corridor, 1' past its wall.
        let t = tops(&s).first().copied().expect("device");
        let want = (111.0 - t[2]) / 12.0 + (252.0 - t[1]) / 12.0;
        assert!(o["ft"].as_f64().is_some_and(|f| (f - want).abs() < 0.06), "{want} vs {o}");
        // The 1-hour wall it crosses: one penetration, the wall type's rating, firestopped.
        let pens = r["penetrations"].as_array().cloned().unwrap_or_default();
        assert_eq!(pens.len(), 1, "{r}");
        assert_eq!((pens[0]["rating"].clone(), pens[0]["wall_type"].clone()), (json!("1 HR"), json!("P1")));
        assert_eq!(r["firestop"]["count"], json!(1));
        let tk = s.execute("conduit.takeoff", &json!({"price": false})).expect("takeoff");
        let fs: Vec<&Value> =
            tk["lines"].as_array().into_iter().flatten().filter(|l| l["kind"] == json!("firestop") && l["type"] == json!("1 HR")).collect();
        assert_eq!(fs.len(), 1, "{tk}");
        // ENT pieces: its stick, a snap-in connector at the box (no locknut), a bushing at the end.
        let c = s.execute("bim.count", &json!({"kind": "conduit", "system": LV_SYSTEM, "group_by": "type"})).expect("count");
        assert_eq!(c["rows"][0]["type"], json!("ENT"), "{c}");
        assert_eq!(lv_count(&mut s, "locknut")["count"], json!(0));
        assert_eq!(lv_count(&mut s, "connector")["count"], json!(1));
        // By unit type: the room's feet.
        assert_eq!(r["unit_types"][0]["unit_type"], json!("KING-A"), "{r}");
        // Rerouting again is stable.
        let again = s.execute("conduit.reroute", &json!({})).expect("again");
        let r2 = s.execute("conduit.route", &json!({"section": "lv_pathways"})).expect("report");
        assert_eq!(r2["ft"], r["ft"], "{again}");
    }

    #[test]
    fn a_corridor_is_reached_just_inside_its_nearest_side() {
        let rc = [[0.0, 0.0], [120.0, 60.0]];
        let q = clamp_in(Vec2::new(60.0, 200.0), &rc, 12.0);
        assert!((q.x - 60.0).abs() < 1e-9 && (q.y - 48.0).abs() < 1e-9, "{q:?}");
        let q = clamp_in(Vec2::new(-50.0, 30.0), &rc, 12.0);
        assert!((q.x - 12.0).abs() < 1e-9 && (q.y - 30.0).abs() < 1e-9, "{q:?}");
    }
}
