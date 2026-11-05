//! Value engineering (Q6154): the wiring-method alternatives a project manager weighs, tiered by
//! what the documents and the code allow, weighed against the design's likely intent, priced
//! with their manpower from the routed takeoff, and modelled as scenarios.
//!
//! - **Tiers, never mixed:** AS DESIGNED (the model, always built this way); ALLOWED NOW (the
//!   spec lists the alternative for the application and the code allows it: the contractor may
//!   choose it); NEEDS RFI (the code allows it, the spec doesn't: an RFI drafted). An option that
//!   can't meet the design's likely intent is not proposed. Each alternative is a separate model
//!   scenario (`ve.scenario` shows it in the model; `conduit.takeoff {scenario}` counts it); it
//!   becomes the design only when its RFI is recorded approved (`ve.decide`, `rfi.set`).
//! - **From the documents:** the code-analysis sheet ([`facts`]: construction type, sprinklers,
//!   occupancies, stories, rated assemblies, the electrical code), the specification's
//!   wiring-methods table (`<application>: <methods>.`, MasterFormat 26 05 19's layout), the
//!   model's rooms ([`scope`]: unit, common or exposed; suspended ceilings, plenums, rated
//!   corridors, below slab). The code is rules data with its cites (`verules.json`).
//! - **Price and manpower** ([`price`]): both ways from the routed takeoff's pieces, through a
//!   cost DB whose labor is in dollars (man-hours = labor $ ÷ the manpower rate), else the
//!   estimate's prices; Δ material, Δ labor $, Δ man-hours, crew-days; unpriced items listed.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::*;
use crate::{Result, Session};

mod facts;
mod price;
mod scenario;
mod scope;
mod sketch;

pub(crate) use price::read_cost_db;
pub(crate) use scenario::Sc as KeptScenario;
pub(crate) use scenario::{VE_ATOM, approved_circuits, approved_feeders, decide, takeoff_view};
pub(crate) use scope::feeder_makeup;

/// The scenarios the drawing keeps (their pieces, the design's pieces each replaces, their RFI
/// and decision): what Validate checks ([`super::validate_value`]).
pub(crate) fn kept_scenarios(d: &cadaddy_doc::Drawing) -> Vec<KeptScenario> {
    scenario::state(d).scenarios
}

const CMD: &str = "ve.study";
/// The alternatives and the methods' words.
const RULES: &str = include_str!("verules.json");
/// Spec files read, at most; characters of one read, at most (hostile input).
const MAX_SPEC_FILES: usize = 60;
const MAX_TEXT: usize = 4_000_000;
/// Applications one table lists, at most; lines one application joins, at most.
const MAX_APPLICATIONS: usize = 60;
const MAX_ITEM_LINES: usize = 40;
/// Items (circuits or feeders) an answer names per scenario, at most.
const MAX_NAMED: usize = 50;

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new(CMD, "Value Engineering", run)
            .menu(&["Draw", "BIM", "Value Engineering"])
            .params(
                "{specs?: [path] (spec PDFs or text files, or folders of them: their wiring-methods tables; default the ones the job read), construction_type?: \"V-A\" (else the code sheet's), code_texts?: [string] (the code-analysis sheet's texts in reading order: construction type, sprinklers, occupancies, stories, rated assemblies, electrical code), cost_db?: path (.csv or .json: item, size, unit, material_unit, labor_unit — labor in DOLLARS; `# price_date:`, `# escalation:`, `# manpower_rate:` lines), manpower_rate?: $/h (default the cost DB's), crew_size?: n, hours_per_day?: h, price_book?: path (.csv: the cost DB rows the job needs, a private book beside the drawing), sketches?: folder (an SVG sketch per RFI), model?: bool (default true: each proposed alternative as a hidden model scenario), force?: bool (replace existing files)} → {facts, construction_type, applications, groups: [{kind, area, as_designed, allowed_now, needs_rfi, not_proposed, not_allowed}], scenarios: [{id, name, tier, why?, application, code, conditions, excluded, intent, checks, quantity, as_designed, alternative, saves, unpriced, rfi?, model}], pricing, price_book?}: the wiring-method alternatives, tiered, priced with their manpower and modelled",
            )
            .noundo(),
        CommandSpec::new("ve.scenario", "Value Engineering Scenario", scenario::run_scenario)
            .menu(&["Draw", "BIM", "Value Engineering Scenario"])
            .params("{id?: scenario id (none: the list), show?: bool} → {scenarios: [{id, name, tier, shown, pieces, replaces, decision, rfi}]} or {id, shown, pieces, replaces}: a scenario shown in the model (its pieces on, the design's pieces it replaces hidden) or hidden again; `conduit.takeoff {scenario}` counts it"),
        CommandSpec::new("ve.decide", "Value Engineering Decision", scenario::run_decide).params(
            "{id: scenario or RFI id, decision: approved|rejected (a NEEDS RFI option, by its RFI's answer)|chosen (an ALLOWED NOW option, the contractor's choice)|undo, by?: text (who answered: the engineer's RFI response), note?: text} → {id, decision, adopted?: {circuits, feeders, pieces}, rfi}: an approved (or chosen) alternative becomes the design: its pieces replace the design's, the circuits keep the method on every rebuild",
        ),
    ]
}

// ---- Rules ---------------------------------------------------------------------------------------

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub(crate) struct Rules {
    methods: BTreeMap<String, Vec<String>>,
    method_names: BTreeMap<String, String>,
    code_sheet: CodeSheetWords,
    areas: BTreeMap<String, AreaRule>,
    crew: Crew,
    ampacity: Ampacity,
    firestop: FirestopItems,
    intents: Vec<Intent>,
    alternatives: Vec<Alternative>,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub(crate) struct CodeSheetWords {
    construction: Vec<String>,
    sprinklered_yes: Vec<String>,
    sprinklered_no: Vec<String>,
    sprinkler_standards: Vec<String>,
    occupancy_words: Vec<String>,
    occupancy_skip: Vec<String>,
    occupancies: Vec<String>,
    stories_words: Vec<String>,
    rated_words: Vec<String>,
    rated_skip: Vec<String>,
    nec_words: Vec<String>,
    source: String,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub(crate) struct AreaRule {
    name: String,
    words: Vec<String>,
    source: String,
    /// What to confirm when an alternative doesn't exclude the area but its exclusions depend on
    /// what the documents don't say.
    confirm: String,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
struct Crew {
    size: f64,
    hours_per_day: f64,
    source: String,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
struct Ampacity {
    source: String,
    cu: BTreeMap<String, BTreeMap<String, f64>>,
    al: BTreeMap<String, BTreeMap<String, f64>>,
    sizes: Vec<String>,
    small_conductor_max_a: BTreeMap<String, BTreeMap<String, f64>>,
    small_conductor_source: String,
    standard_ocpd_a: Vec<f64>,
    next_size_up_source: String,
    bundling: Bundling,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
struct Bundling {
    cables_per_opening: usize,
    factors: Vec<[f64; 3]>,
    source: String,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
struct FirestopItems {
    raceway_item: String,
    cable_item: String,
    source: String,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
struct Intent {
    id: String,
    reason: String,
    detect: String,
    meets: String,
    risk: String,
    defeats: bool,
    defeats_when_stated: Vec<String>,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub(crate) struct Alternative {
    pub id: String,
    pub name: String,
    pub circuit: String,
    pub to: String,
    pub areas: Vec<String>,
    pub application: Vec<String>,
    pub application_not: Vec<String>,
    pub designed: String,
    pub alternative: String,
    pub cable: String,
    pub construction: Vec<String>,
    pub ampacity_column: String,
    pub ampacity_why: String,
    pub bundling: bool,
    pub code: Vec<Cite>,
    pub conditions: Vec<String>,
    pub excluded_areas: BTreeMap<String, String>,
    pub intents: Vec<String>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(default)]
pub(crate) struct Cite {
    cite: String,
    says: String,
}

pub(crate) fn rules() -> Rules {
    serde_json::from_str(RULES).unwrap_or_default()
}

// ---- The specification's wiring-methods table ---------------------------------------------------

/// One application of a specification's wiring-methods table.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Application {
    pub section: String,
    /// Its letter (`C`).
    pub item: String,
    /// What it applies to (`Feeders Concealed in Ceilings, Walls, Partitions, and Crawlspaces`).
    pub application: String,
    /// The methods it lists, by the rules' names (`raceway`, `nm`).
    pub methods: Vec<String>,
    /// What it says after the colon (the paragraph an RFI quotes).
    pub text: String,
}

/// An outline letter line (`C.` or `C. Feeders…`): the letter and the rest.
fn lettered(line: &str) -> Option<(String, String)> {
    let t = line.trim();
    let mut c = t.chars();
    let (l, dot) = (c.next()?, c.next()?);
    (l.is_ascii_uppercase() && dot == '.' && t.get(2..).is_none_or(|r| r.is_empty() || r.starts_with(' ')))
        .then(|| (l.to_string(), t.get(2..).unwrap_or_default().trim().to_string()))
}

/// An article line (`3.3`, `3.3 INSTALLATION…`) or a part (`PART 3 - EXECUTION`).
fn article(line: &str) -> bool {
    let t = line.trim();
    if t.to_ascii_uppercase().starts_with("PART ") {
        return true;
    }
    let head = t.split_whitespace().next().unwrap_or_default();
    let mut parts = head.split('.');
    matches!((parts.next(), parts.next(), parts.next()), (Some(a), Some(b), None) if !a.is_empty() && !b.is_empty() && a.chars().all(|c| c.is_ascii_digit()) && b.chars().all(|c| c.is_ascii_digit()))
}

/// A page footer (`260519 - 5`).
fn footer(line: &str) -> bool {
    let t: String = line.chars().filter(|c| !c.is_whitespace()).collect();
    t.split_once('-')
        .is_some_and(|(a, b)| a.len() == 6 && a.chars().all(|c| c.is_ascii_digit()) && !b.is_empty() && b.chars().all(|c| c.is_ascii_digit()))
}

/// A specification section's wiring-methods table: the article whose heading names WIRING
/// METHODS, each lettered item `<application>: <methods>.` with the methods it lists.
pub(crate) fn wiring_applications(text: &str, section: &str) -> Vec<Application> {
    let rules = rules();
    let lines: Vec<&str> = text.lines().map(str::trim).filter(|l| !l.is_empty() && !footer(l)).collect();
    let Some(start) = lines.iter().position(|l| l.to_ascii_uppercase().contains("WIRING METHODS")) else { return Vec::new() };
    let mut items: Vec<(String, Vec<String>)> = Vec::new();
    for l in lines.iter().skip(start + 1) {
        if article(l) {
            break;
        }
        if let Some((letter, rest)) = lettered(l) {
            if items.len() >= MAX_APPLICATIONS {
                break;
            }
            items.push((letter, if rest.is_empty() { Vec::new() } else { vec![rest] }));
        } else if let Some((_, body)) = items.last_mut()
            && body.len() < MAX_ITEM_LINES
        {
            // A running header repeated at a page break (the section's title in capitals, no
            // punctuation, in an item written in lower case) is not the item's text.
            let header = l.chars().any(char::is_alphabetic)
                && !l.chars().any(char::is_lowercase)
                && !l.contains([':', ',', '.'])
                && body.first().is_some_and(|b| b.chars().any(char::is_lowercase))
                && body.iter().any(|b| b.contains(':'));
            if !header {
                body.push((*l).to_string());
            }
        }
    }
    items
        .into_iter()
        .filter_map(|(item, body)| {
            let all = body.join(" ");
            let (app, said) = all.split_once(':')?;
            let up = said.to_ascii_uppercase();
            let methods = rules.methods.iter().filter(|(_, words)| words.iter().any(|w| up.contains(w.as_str()))).map(|(m, _)| m.clone()).collect();
            Some(Application {
                section: section.to_string(),
                item,
                application: app.split_whitespace().collect::<Vec<_>>().join(" "),
                methods,
                text: said.split_whitespace().collect::<Vec<_>>().join(" ").chars().take(600).collect(),
            })
        })
        .collect()
}

/// The construction type a code sheet gives (`CONSTRUCTION TYPE V-A`, `TYPE OF CONSTRUCTION:
/// TYPE V-A`): `V-A`. `texts` in reading order; the type is in the label's text or the next few.
pub(crate) fn construction_type(texts: &[String]) -> Option<String> {
    facts::construction(texts, &rules()).map(|f| f.value)
}

/// A sheet's texts in reading order: down the sheet in rows of its usual text height, then
/// across. Each text gets one key, so the order is total whatever the texts' heights.
pub(crate) fn reading_order(texts: Vec<cadaddy_mepfix::sheet_facts::TextItem>) -> Vec<String> {
    let mut hs: Vec<f64> = texts.iter().map(|t| t.height).filter(|h| h.is_finite() && *h > 0.0).collect();
    hs.sort_by(f64::total_cmp);
    let row = hs.get(hs.len() / 2).copied().unwrap_or(1.0).max(1e-6);
    let mut keyed: Vec<(i64, f64, String)> = texts
        .into_iter()
        .filter(|t| t.at.x.is_finite() && t.at.y.is_finite())
        .map(|t| (-((t.at.y / row).round().clamp(-1e15, 1e15) as i64), t.at.x, t.value))
        .collect();
    keyed.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.total_cmp(&b.1)));
    keyed.into_iter().map(|k| k.2).collect()
}

/// A construction type's class (`V-A` → `V`).
fn class_of(t: &str) -> &str {
    t.split('-').next().unwrap_or(t)
}

/// The spec files' text, each with its section (`26 05 19`): PDFs and text files, or the
/// Division 26 files in folders.
pub(crate) fn spec_texts(paths: &[String]) -> Vec<(String, String)> {
    fn walk(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>, depth: usize) {
        if depth > 4 || out.len() >= MAX_SPEC_FILES * 20 {
            return;
        }
        let Ok(rd) = std::fs::read_dir(dir) else { return };
        let mut entries: Vec<std::path::PathBuf> = rd.flatten().map(|e| e.path()).collect();
        entries.sort();
        for p in entries {
            if p.is_dir() {
                walk(&p, out, depth + 1);
            } else {
                out.push(p);
            }
        }
    }
    let digits = |name: &str| -> String { name.chars().take(12).filter(char::is_ascii_digit).take(6).collect() };
    let mut files: Vec<std::path::PathBuf> = Vec::new();
    for given in paths {
        let p = std::path::Path::new(given.trim());
        if p.is_dir() {
            let mut found = Vec::new();
            walk(p, &mut found, 0);
            // Division 26's basic materials and methods (26 05 xx): the wiring methods live there.
            files.extend(found.into_iter().filter(|f| {
                let name = f.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
                digits(&name).starts_with("2605") && name.chars().next().is_some_and(|c| c.is_ascii_digit())
            }));
        } else {
            files.push(p.to_path_buf());
        }
    }
    files.truncate(MAX_SPEC_FILES);
    let mut out = Vec::new();
    for f in files {
        let path = f.to_string_lossy().to_string();
        let name = f.file_stem().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
        let d = digits(&name);
        let section = if d.len() == 6 { format!("{} {} {}", &d[..2], &d[2..4], &d[4..6]) } else { name.clone() };
        let Ok(bytes) = crate::readfile::read(&f) else { continue };
        let text = if path.to_ascii_lowercase().ends_with(".pdf") {
            match super::spec_import::pdf_text(&bytes, &path) {
                Ok(t) => t,
                Err(_) => continue,
            }
        } else {
            String::from_utf8_lossy(&bytes).to_string()
        };
        out.push((section, text.chars().take(MAX_TEXT).collect()));
    }
    out
}

// ---- Checks: ampacity, bundling --------------------------------------------------------------------

/// A conductor's ampacity at a column (`60`, `75`, `90`).
fn ampacity(rules: &Rules, al: bool, size: &str, column: &str) -> Option<f64> {
    let t = if al { &rules.ampacity.al } else { &rules.ampacity.cu };
    t.get(column)?.get(size.trim().trim_start_matches('#').split_whitespace().next().unwrap_or_default()).copied()
}

/// The next standard protection size at or above `a` (240.6(A)).
fn next_standard(rules: &Rules, a: f64) -> Option<f64> {
    rules.ampacity.standard_ocpd_a.iter().copied().find(|s| *s + 1e-9 >= a)
}

/// Whether a conductor at a column is protected by `breaker` (310.16 with 240.4(B), (D)); the
/// conductor's ampacity.
fn protected(rules: &Rules, al: bool, size: &str, column: &str, breaker: f64, factor: f64) -> (bool, Option<f64>) {
    let bare = size.trim().trim_start_matches('#').split_whitespace().next().unwrap_or_default().to_string();
    let col = ampacity(rules, al, &bare, column);
    // The bundled ampacity: the 90 °C rating derated, not above the column's.
    let amp = if factor < 1.0 { ampacity(rules, al, &bare, "90").map(|a| a * factor).zip(col).map(|(a, c)| a.min(c)) } else { col };
    let Some(amp) = amp else { return (false, None) };
    let small = rules.ampacity.small_conductor_max_a.get(if al { "al" } else { "cu" }).and_then(|m| m.get(&bare)).copied();
    if small.is_some_and(|m| breaker > m + 1e-9) {
        return (false, Some(amp));
    }
    let ok = amp + 1e-9 >= breaker || (breaker <= 800.0 && next_standard(rules, amp).is_some_and(|n| n + 1e-9 >= breaker));
    (ok, Some(amp))
}

/// The smallest conductor at or above `size` that `breaker` protects.
fn upsize(rules: &Rules, al: bool, size: &str, column: &str, breaker: f64, factor: f64) -> Option<String> {
    let bare = size.trim().trim_start_matches('#').split_whitespace().next().unwrap_or_default().to_string();
    let from = rules.ampacity.sizes.iter().position(|s| *s == bare)?;
    rules.ampacity.sizes.iter().skip(from).find(|s| protected(rules, al, s, column, breaker, factor).0).cloned()
}

/// The bundling factor for `cables` cables of two current-carrying conductors through one
/// opening (334.80 → 310.15(C)(1)); 1 at or under the rule's count.
fn bundling_factor(rules: &Rules, cables: usize) -> f64 {
    let b = &rules.ampacity.bundling;
    if cables <= b.cables_per_opening.max(1) {
        return 1.0;
    }
    let ccc = (cables * 2) as f64;
    b.factors.iter().find(|f| ccc >= f[0] && ccc <= f[1]).map_or(1.0, |f| f[2])
}

// ---- The study ------------------------------------------------------------------------------------

/// What the job (or the caller) gives the study.
#[derive(Clone, Debug, Default)]
pub(crate) struct Inputs {
    pub applications: Vec<Application>,
    pub facts: facts::CodeFacts,
    pub cost_db: Option<price::CostDb>,
    pub manpower_rate: Option<f64>,
    pub crew_size: Option<f64>,
    pub hours_per_day: Option<f64>,
    pub model: bool,
    pub sketches: Option<String>,
    /// Replace existing files (sketches).
    pub force: bool,
}

fn r1(v: f64) -> f64 {
    (v * 10.0).round() / 10.0
}

/// The manpower basis: the caller's rate, the cost DB's, else the estimate's labor rate.
fn manpower(s: &mut Session, rules: &Rules, inp: &Inputs) -> price::Manpower {
    let (rate, rate_source) = match (inp.manpower_rate, inp.cost_db.as_ref().and_then(|d| d.manpower_rate)) {
        (Some(r), _) => (Some(r), "given".to_string()),
        (None, Some(r)) => (
            Some(r),
            inp.cost_db
                .as_ref()
                .map(|d| if d.manpower_rate_source.is_empty() { format!("cost DB {}", d.path) } else { d.manpower_rate_source.clone() })
                .unwrap_or_default(),
        ),
        _ => match s.execute("estimate.settings", &json!({})).ok().and_then(|v| v["labor_rate"].as_f64()).filter(|v| v.is_finite() && *v > 0.0) {
            Some(r) => (Some(r), "the estimate's labor rate (no cost DB manpower rate)".to_string()),
            None => (None, "none: man-hours not computed".to_string()),
        },
    };
    let given_crew = inp.crew_size.is_some() || inp.hours_per_day.is_some();
    price::Manpower {
        rate,
        rate_source,
        crew_size: inp.crew_size.filter(|v| v.is_finite() && *v > 0.0).unwrap_or(rules.crew.size.max(1.0)),
        hours_per_day: inp.hours_per_day.filter(|v| v.is_finite() && *v > 0.0).unwrap_or(if rules.crew.hours_per_day > 0.0 {
            rules.crew.hours_per_day
        } else {
            8.0
        }),
        crew_source: if given_crew { "given".into() } else { rules.crew.source.clone() },
    }
}

/// The application of the spec's table an alternative answers to.
fn application_for<'a>(alt: &Alternative, apps: &'a [Application]) -> Option<&'a Application> {
    apps.iter().find(|ap| {
        let up = ap.application.to_ascii_uppercase();
        alt.application.iter().all(|w| up.contains(w.as_str())) && !alt.application_not.iter().any(|w| up.contains(w.as_str()))
    })
}

/// The alternatives against the documents and the model: each one's tier, scope, checks,
/// intent, prices both ways and (needing an RFI) its draft.
pub(crate) fn study(s: &mut Session, inp: &Inputs) -> Result<Value> {
    let rules = rules();
    let (sp, _) = super::bspec::current(s, CMD)?;
    let construction = inp.facts.construction_type.as_ref().map(|f| f.value.clone());
    if inp.facts.is_empty() {
        s.echo("value engineering: no code-analysis sheet read (give code_texts or construction_type): the code's limits by construction type can't be applied");
    }
    let items = {
        let d = s.doc()?;
        scope::items(d, &sp, &inp.facts, &rules)
    };
    let mp = manpower(s, &rules, inp);
    let mut scenarios: Vec<Value> = Vec::new();
    let mut groups: BTreeMap<(String, String), Value> = BTreeMap::new();
    // As designed, by kind and area: every routed circuit and feeder.
    for it in &items {
        let g = groups.entry((it.kind.to_string(), it.area.base.clone())).or_insert_with(|| {
            json!({"kind": it.kind, "area": it.area.base, "as_designed": {"methods": {}, "items": 0, "feet": 0.0}, "allowed_now": [], "needs_rfi": [], "not_proposed": [], "not_allowed": [], "no_scope": [], "no_application": []})
        });
        let m = format!("{} ({})", if it.method.is_empty() { "raceway" } else { &it.method }, it.method_source);
        let n = g["as_designed"]["methods"][&m].as_u64().unwrap_or(0) + 1;
        g["as_designed"]["methods"][&m] = json!(n);
        g["as_designed"]["items"] = json!(g["as_designed"]["items"].as_u64().unwrap_or(0) + 1);
        g["as_designed"]["feet"] = json!(r1(g["as_designed"]["feet"].as_f64().unwrap_or(0.0) + it.feet));
    }
    let mut built: Vec<scenario::Built> = Vec::new();
    for alt in &rules.alternatives {
        let app = application_for(alt, &inp.applications);
        let code_ok = construction.as_deref().is_some_and(|c| alt.construction.iter().any(|k| k.eq_ignore_ascii_case(class_of(c))));
        // The circuits it would change, and the ones its exclusions keep as designed.
        let mut taken: Vec<&scope::Item> = Vec::new();
        let mut excluded: BTreeMap<String, usize> = BTreeMap::new();
        let mut not_read: Vec<Value> = Vec::new();
        let mut not_read_n = 0usize;
        for it in items.iter().filter(|it| it.kind == alt.circuit) {
            match scope::takes(alt, it) {
                Ok(()) => taken.push(it),
                Err(why) if alt.excluded_areas.contains_key(&why) => *excluded.entry(why).or_default() += 1,
                Err(why) if why == scope::NOT_READ && alt.areas.contains(&it.area.base) => {
                    not_read_n += 1;
                    if not_read.len() < MAX_NAMED {
                        not_read.push(json!({"id": it.id, "label": it.label, "level": it.level, "conductors": it.conductors}));
                    }
                }
                Err(_) => {}
            }
        }
        // Ampacity at the alternative's column; bundling where a homerun's cables share a hole.
        let mut checks: Vec<Value> = Vec::new();
        let mut sized: Vec<scope::Item> = Vec::new();
        for it in &taken {
            let mut it = (*it).clone();
            let al = it.conductors.to_ascii_uppercase().contains(" AL");
            let parts = super::mepatoms::conductor_parts(&it.conductors);
            let Some((_, size, _)) = parts.iter().find(|p| !p.2).cloned() else {
                sized.push(it);
                continue;
            };
            let cables = if it.members.is_empty() { 1 } else { it.members.len() };
            let factor = if alt.bundling { bundling_factor(&rules, cables) } else { 1.0 };
            if let Some(b) = it.breaker_a.or_else(|| (it.kind == "branch").then(|| super::conduitroute::rules().vdrop.breaker_a)) {
                let (ok, amp) = protected(&rules, al, &size, &alt.ampacity_column, b, factor);
                if !ok {
                    let to = upsize(&rules, al, &size, &alt.ampacity_column, b, factor);
                    if let Some(to) = &to {
                        it.conductors = it.conductors.replacen(&format!("#{size}"), &format!("#{to}"), 2);
                        it.members = it.members.iter().map(|(l, c)| (l.clone(), c.replacen(&format!("#{size}"), &format!("#{to}"), 2))).collect();
                    }
                    if checks.len() < MAX_NAMED {
                        checks.push(json!({
                            "item": it.id, "level": it.level, "conductor": format!("#{size}{}", if al { " AL" } else { "" }), "breaker_a": b,
                            "column_c": alt.ampacity_column, "ampacity_a": amp, "bundled_cables": cables, "factor": factor,
                            "upsized_to": to.map(|t| format!("#{t}")), "why": alt.ampacity_why,
                            "cite": format!("{}; {}; {}", rules.ampacity.source, rules.ampacity.next_size_up_source, if factor < 1.0 { rules.ampacity.bundling.source.as_str() } else { "" }),
                        }));
                    }
                }
            }
            sized.push(it);
        }
        let taken_ref: Vec<&scope::Item> = sized.iter().collect();
        let feet: f64 = taken_ref.iter().map(|i| i.feet).sum();
        // What the documents don't say that its exclusions depend on: to confirm, with how many.
        let mut to_confirm: Vec<Value> = Vec::new();
        for (k, a) in rules.areas.iter().filter(|(_, a)| !a.confirm.is_empty()) {
            let n = taken_ref.iter().filter(|i| i.area.flags.contains(k)).count();
            if n > 0 && alt.excluded_areas.contains_key("suspended") {
                to_confirm.push(json!({"area": k, "name": a.name, "items": n, "confirm": a.confirm}));
            }
        }
        let pens: usize = taken_ref.iter().map(|i| i.rated_penetrations).sum();
        // The design's likely intent, from the documents and the model.
        let mut intent: Vec<Value> = Vec::new();
        let mut defeated: Option<String> = None;
        for iid in &alt.intents {
            let Some(i) = rules.intents.iter().find(|x| &x.id == iid) else { continue };
            let (likely, evidence) = match i.detect.as_str() {
                "spec_raceway_only" => match app {
                    Some(ap) => (ap.methods == ["raceway"], format!("{} §{} lists {}", ap.section, ap.item, ap.methods.join(", "))),
                    None => (false, "no wiring-methods table read".into()),
                },
                "rated_penetrations" => (pens > 0, format!("{pens} rated wall penetration(s) on these runs (conduit.route penetrations)")),
                "oversized_raceway" => {
                    let over: Vec<String> = taken_ref
                        .iter()
                        .filter_map(|i| i.oversized.map(|(a, b)| format!("{} {}\" where {}\" fits", i.label, a, b)))
                        .take(5)
                        .collect();
                    let n = taken_ref.iter().filter(|i| i.oversized.is_some()).count();
                    (
                        n > 0,
                        if n > 0 {
                            format!("{n} raceway(s) larger than their conductors need (NEC Chapter 9 fill): {}", over.join("; "))
                        } else {
                            "every raceway sized to its conductors".into()
                        },
                    )
                }
                "exposed" => {
                    let n =
                        items.iter().filter(|it| it.kind == alt.circuit && alt.areas.contains(&it.area.base) && it.area.base == "exposed").count();
                    (n > 0, format!("{n} exposed run(s) in the alternative's areas"))
                }
                _ => (false, String::new()),
            };
            let stated = app.and_then(|ap| i.defeats_when_stated.iter().find(|w| ap.text.to_ascii_uppercase().contains(w.as_str())).cloned());
            if likely && (i.defeats || stated.is_some()) {
                defeated.get_or_insert_with(|| match &stated {
                    Some(w) => format!("{} (the specification says \"{w}\")", i.reason),
                    None => i.reason.clone(),
                });
            }
            intent.push(json!({
                "intent": i.id, "reason": i.reason, "likely": likely, "evidence": evidence,
                "met": if likely { if i.meets.is_empty() { Value::Null } else { json!(i.meets) } } else { Value::Null },
                "risk": if likely && !i.risk.is_empty() { json!(i.risk) } else { Value::Null },
            }));
        }
        let lists_alt = app.is_some_and(|ap| ap.methods.contains(&alt.alternative));
        let (tier, why) = match (app, code_ok) {
            (None, _) => ("no_application", "the specification's wiring-methods table has no application this alternative answers to".to_string()),
            (Some(_), false) => (
                "not_allowed",
                format!(
                    "the code allows it in construction types {} only; the building is {}",
                    alt.construction.join(", "),
                    construction.as_deref().unwrap_or("of a type the code sheet doesn't give")
                ),
            ),
            _ if taken_ref.is_empty() => (
                "no_scope",
                if excluded.values().any(|n| *n > 0) {
                    format!(
                        "every circuit or feeder in its areas stays as designed: {}",
                        excluded
                            .iter()
                            .filter(|(k, n)| **n > 0 && !matches!(k.as_str(), "unit" | "common" | "exposed"))
                            .map(|(k, n)| format!("{}: {n}", rules.areas.get(k).map_or(k.as_str(), |a| a.name.as_str())))
                            .collect::<Vec<_>>()
                            .join(", ")
                    )
                } else {
                    "no routed circuit or feeder of the model is in its areas".to_string()
                },
            ),
            _ if defeated.is_some() => {
                ("not_proposed", format!("it can't meet the design's likely intent: {}", defeated.clone().unwrap_or_default()))
            }
            _ if lists_alt => ("allowed_now", String::new()),
            _ => ("needs_rfi", String::new()),
        };
        // Price both ways from the routed takeoff.
        let fs_raceway = rules.firestop.raceway_item.clone();
        let fs_cable = rules.firestop.cable_item.clone();
        let (design_lines, atoms, alt_lines) = {
            let d = s.doc()?;
            let dl = scope::design_lines(d, &sp, &taken_ref, &fs_raceway);
            let atoms = scope::alt_atoms(&sp, &taken_ref, &alt.cable, &super::conduitroute::cable_paths(d));
            let al = scope::alt_lines(&sp, &taken_ref, &atoms, &alt.cable, &fs_cable);
            (dl, atoms, al)
        };
        let priced = !taken_ref.is_empty();
        let (dv, xv) = if priced {
            (price::price(s, &design_lines, inp.cost_db.as_ref(), &mp), price::price(s, &alt_lines, inp.cost_db.as_ref(), &mp))
        } else {
            (Value::Null, Value::Null)
        };
        let saves = if priced { price::saves(&dv, &xv, &mp) } else { Value::Null };
        let mut unpriced: Vec<Value> = dv["unpriced"].as_array().cloned().unwrap_or_default();
        unpriced.extend(xv["unpriced"].as_array().cloned().unwrap_or_default());
        let named: Vec<Value> = taken_ref
            .iter()
            .take(MAX_NAMED)
            .map(|i| json!({"id": i.id, "label": i.label, "level": i.level, "panel": i.panel, "to": i.to, "feet": r1(i.feet), "rooms": i.area.rooms.iter().take(6).collect::<Vec<_>>()}))
            .collect();
        let mut v = json!({
            "id": alt.id, "name": alt.name, "tier": tier, "kind": alt.circuit,
            "designed": rules.method_names.get(&alt.designed).cloned().unwrap_or_else(|| alt.designed.clone()),
            "alternative_method": rules.method_names.get(&alt.alternative).cloned().unwrap_or_else(|| alt.alternative.clone()),
            "construction_type": construction,
            "application": app.map(|ap| json!({"section": ap.section, "item": ap.item, "application": ap.application, "methods": ap.methods, "text": ap.text})),
            "code": alt.code, "conditions": alt.conditions,
            "excluded": alt.excluded_areas.iter().map(|(k, w)| json!({
                "area": k, "name": rules.areas.get(k).map(|a| a.name.clone()).unwrap_or_default(), "why": w, "kept_as_designed": excluded.get(k).copied().unwrap_or(0),
            })).collect::<Vec<_>>(),
            "intent": intent, "checks": checks,
            "to_confirm": to_confirm,
            "not_weighed": {"count": not_read_n, "why": "their conductors don't read as conductors (a count, # and a size): kept as designed, asked about upstream", "items": not_read},
            "quantity": {
                "items": taken_ref.len(),
                "circuits": taken_ref.iter().filter(|i| i.kind == "branch" && i.members.is_empty()).count(),
                "homeruns": taken_ref.iter().filter(|i| !i.members.is_empty()).count(),
                "feeders": taken_ref.iter().filter(|i| i.kind == "feeder").count(),
                "feet": r1(feet), "rated_penetrations": pens, "named": named,
            },
            "as_designed": dv, "alternative": xv, "saves": saves, "unpriced": unpriced,
        });
        if !why.is_empty() {
            v["why"] = json!(why);
        }
        for a in &alt.areas {
            if let Some(g) = groups.get_mut(&(alt.circuit.clone(), a.clone()))
                && let Some(list) = g[tier].as_array_mut()
                && !list.contains(&json!(alt.id))
            {
                list.push(json!(alt.id));
            }
        }
        if matches!(tier, "allowed_now" | "needs_rfi") {
            built.push(scenario::Built { alt: alt.clone(), tier: tier.to_string(), items: sized.clone(), atoms, value: v.clone() });
        }
        scenarios.push(v);
    }
    // The RFIs, the sketches and the model's scenarios.
    let rfis = scenario::draft_rfis(s, &sp, &rules, inp, &mut built, &mp)?;
    let model = if inp.model { scenario::store(s, &built)? } else { Value::Null };
    for sc in scenarios.iter_mut() {
        let id = sc["id"].as_str().unwrap_or_default().to_string();
        if let Some(r) = rfis.get(&id) {
            sc["rfi"] = r.clone();
        }
        if let Some(m) = model.get(&id) {
            sc["model"] = m.clone();
        }
    }
    let pricing = json!({
        "basis": match &inp.cost_db {
            Some(db) => format!("cost DB {} ({} rows): labor_unit in DOLLARS", db.path, db.rows.len()),
            None => "no cost DB given: the estimate's prices (drawing, price book, starter book), labor in hours per unit".to_string(),
        },
        "price_date": inp.cost_db.as_ref().map(|d| d.price_date.clone()).filter(|t| !t.is_empty()),
        "escalation": inp.cost_db.as_ref().map(|d| d.escalation.clone()).filter(|t| !t.is_empty()),
        "manpower": mp.json(),
    });
    Ok(json!({
        "facts": inp.facts, "construction_type": construction,
        "applications": inp.applications.iter().map(|a| json!({"section": a.section, "item": a.item, "application": a.application, "methods": a.methods})).collect::<Vec<_>>(),
        "groups": groups.into_values().collect::<Vec<_>>(),
        "scenarios": scenarios,
        "pricing": pricing,
    }))
}

/// The lines the job needs priced (both ways of every proposed scenario), for the price book.
fn all_lines(v: &Value) -> Vec<price::Line> {
    let mut out = Vec::new();
    for sc in v["scenarios"].as_array().into_iter().flatten() {
        for side in ["as_designed", "alternative"] {
            for l in sc[side]["lines"].as_array().into_iter().flatten() {
                out.push(price::Line {
                    kind: l["kind"].as_str().unwrap_or_default().into(),
                    item: l["item"].as_str().unwrap_or_default().into(),
                    size: l["size"].as_str().unwrap_or_default().into(),
                    unit: l["unit"].as_str().unwrap_or_default().into(),
                    qty: l["qty"].as_f64().unwrap_or(0.0),
                });
            }
        }
    }
    out
}

fn run(s: &mut Session, p: &Value) -> Result<Value> {
    let rules = rules();
    let mut paths: Vec<String> = match p.get("specs") {
        None | Some(Value::Null) => Vec::new(),
        Some(Value::String(t)) => vec![t.clone()],
        Some(Value::Array(a)) => a.iter().filter_map(Value::as_str).map(str::to_string).take(MAX_SPEC_FILES).collect(),
        Some(_) => return Err(bad(CMD, "`specs` takes a path or a list of paths")),
    };
    paths.retain(|x| !x.trim().is_empty());
    if let Some(path) = str_param(p, "price_book").filter(|x| !x.trim().is_empty()) {
        super::exchange::guard_output(s, CMD, p, path)?;
    }
    let mut applications = Vec::new();
    for (section, text) in spec_texts(&paths) {
        applications.extend(wiring_applications(&text, &section));
    }
    // None given: what the job read for the router.
    if paths.is_empty() {
        let m = super::conduitroute::Methods::stored(s.doc()?);
        applications.extend(m.applications.into_iter().map(|a| Application {
            section: a.section,
            item: a.item,
            application: a.application,
            methods: a.methods,
            text: String::new(),
        }));
    }
    let code_texts: Vec<String> = p
        .get("code_texts")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(|t| t.chars().take(2_000).collect())
        .take(facts::MAX_TEXTS)
        .collect();
    let mut f = facts::read(&code_texts, &rules);
    if let Some(given) = p.get("construction_type").and_then(Value::as_str).map(str::trim).filter(|t| !t.is_empty()) {
        f.construction_type = Some(facts::Fact { value: given.to_ascii_uppercase(), source: "given".into() });
    }
    if let Some(sheet) = p.get("code_sheet").and_then(Value::as_str) {
        f.sheet = sheet.chars().take(40).collect();
    }
    let cost_db = match str_param(p, "cost_db").filter(|x| !x.trim().is_empty()) {
        Some(path) => Some(price::read_cost_db(path, CMD)?),
        None => None,
    };
    let num = |k: &str| p.get(k).and_then(Value::as_f64).filter(|v| v.is_finite() && *v > 0.0 && *v < 1e6);
    let inp = Inputs {
        applications,
        facts: f,
        cost_db,
        manpower_rate: num("manpower_rate"),
        crew_size: num("crew_size"),
        hours_per_day: num("hours_per_day").map(|h| h.min(24.0)),
        model: bool_or(p, "model", true),
        sketches: str_param(p, "sketches").filter(|x| !x.trim().is_empty()).map(str::to_string),
        force: bool_or(p, "force", false),
    };
    let mut v = study(s, &inp)?;
    if let (Some(path), Some(db)) = (str_param(p, "price_book").filter(|x| !x.trim().is_empty()), inp.cost_db.as_ref()) {
        let (csv, rows) = price::book_csv(db, &all_lines(&v));
        crate::writefile::write(path, csv).map_err(|e| bad(CMD, format!("could not write {path}: {e}")))?;
        v["price_book"] =
            json!({"file": path, "rows": rows, "private": "the cost DB's own prices: keep it beside the drawing, never in a public place"});
    }
    Ok(v)
}

#[cfg(test)]
#[path = "valueeng_tests.rs"]
mod tests;
