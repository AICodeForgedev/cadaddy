//! Value engineering (Q6154): the wiring-method alternatives a project manager weighs, priced
//! from the model and tiered by what the documents and the code allow.
//!
//! - **The alternatives** are data (`verules.json`): a method swap (SER cable for the feeders to
//!   the units' panels, NM for the units' branch circuits) with the code's cites, its conditions,
//!   the places it is never used and how the design intent is still met.
//! - **The specification's wiring-methods table** (`<application>: <methods>.`, as MasterFormat
//!   section 26 05 19 lays it out) says what each application may use; **the code sheet** gives
//!   the building's construction type.
//! - **Tiers, never mixed:** as designed (the model, always); `allowed_now` (the spec lists the
//!   alternative for that application and the code allows it); `needs_rfi` (the code allows it,
//!   the spec doesn't: an RFI drafted). An alternative becomes the design only on an approved RFI.
//! - **Price and manpower:** the model's quantities (the feeders' routed lengths, their
//!   conductors) priced both ways through the estimate's prices (`estimate.change`: the drawing's
//!   own prices, the price book, the starter book), material, labor hours and cost.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::*;
use crate::{Result, Session};

const CMD: &str = "ve.study";
/// The alternatives and the methods' words.
const RULES: &str = include_str!("verules.json");
/// Spec files read, at most; characters of one read, at most (hostile input).
const MAX_SPEC_FILES: usize = 60;
const MAX_TEXT: usize = 4_000_000;
/// Applications one table lists, at most; lines one application joins, at most.
const MAX_APPLICATIONS: usize = 60;
const MAX_ITEM_LINES: usize = 40;
/// Feeders and runs looked at, at most.
const MAX_FEEDERS: usize = 2_000;
const MAX_RUNS: usize = 200_000;

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new(CMD, "Value Engineering", run)
            .menu(&["Draw", "BIM", "Value Engineering"])
            .params(
                "{specs?: [path] (spec PDFs or text files, or folders of them: their wiring-methods tables), construction_type?: \"V-A\" (else the code sheet's, given as code_texts), code_texts?: [string]} → {construction_type, applications: [{section, item, application, methods}], scenarios: [{id, name, tier: allowed_now|needs_rfi|not_allowed|no_application, application?, code: [{cite, says}], conditions, excluded, intent, quantity: {feeders, feet, not_routed}, as_designed: {lines, material, labor_hours, cost}, alternative: {…}, saves: {material, labor_hours, cost}, unpriced, rfi?: {issue, proposed}}]}: the wiring-method alternatives, tiered and priced from the model",
            )
            .noundo(),
    ]
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
struct Rules {
    methods: BTreeMap<String, Vec<String>>,
    alternatives: Vec<Alternative>,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
struct Alternative {
    id: String,
    name: String,
    circuit: String,
    to: String,
    application: Vec<String>,
    application_not: Vec<String>,
    designed: String,
    alternative: String,
    construction: Vec<String>,
    code: Vec<Cite>,
    conditions: Vec<String>,
    excluded: Vec<String>,
    intent: String,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(default)]
struct Cite {
    cite: String,
    says: String,
}

fn rules() -> Rules {
    serde_json::from_str(RULES).unwrap_or_default()
}

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
            body.push((*l).to_string());
        }
    }
    items
        .into_iter()
        .filter_map(|(item, body)| {
            let all = body.join(" ");
            let (app, said) = all.split_once(':')?;
            let up = said.to_ascii_uppercase();
            let methods = rules.methods.iter().filter(|(_, words)| words.iter().any(|w| up.contains(w.as_str()))).map(|(m, _)| m.clone()).collect();
            Some(Application { section: section.to_string(), item, application: app.split_whitespace().collect::<Vec<_>>().join(" "), methods })
        })
        .collect()
}

/// The construction type a code sheet gives (`CONSTRUCTION TYPE V-A`, `TYPE OF CONSTRUCTION:
/// TYPE V-A`): `V-A`. `texts` in reading order; the type is in the label's text or the next few.
pub(crate) fn construction_type(texts: &[String]) -> Option<String> {
    let typ = |w: &str| -> Option<String> {
        let w = w.trim_matches(|c: char| !c.is_ascii_alphanumeric() && c != '-').to_ascii_uppercase();
        let (num, letter) = match w.split_once('-') {
            Some((n, l)) => (n.to_string(), Some(l.to_string())),
            None => (w.clone(), None),
        };
        ["I", "II", "III", "IV", "V"].contains(&num.as_str()).then_some(())?;
        match letter {
            Some(l) if ["A", "B", "HT"].contains(&l.as_str()) => Some(format!("{num}-{l}")),
            None => Some(num),
            _ => None,
        }
    };
    for (i, t) in texts.iter().enumerate() {
        let up = t.to_ascii_uppercase();
        let at = up.find("CONSTRUCTION TYPE").map(|p| p + "CONSTRUCTION TYPE".len()).or_else(|| up.find("TYPE OF CONSTRUCTION").map(|p| p + 20));
        let Some(at) = at else { continue };
        let rest = up.get(at..).unwrap_or_default().to_string();
        let found = std::iter::once(rest).chain(texts.iter().skip(i + 1).take(3).map(|x| x.to_ascii_uppercase())).find_map(|x| {
            let words: Vec<&str> = x.split_whitespace().collect();
            // `PER TABLE 601`'s number isn't a type; a type is its own word.
            words.iter().find_map(|w| typ(w))
        });
        if found.is_some() {
            return found;
        }
    }
    None
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

/// A feeder to a unit's panel as the model routes it.
struct UnitFeeder {
    feeder: cadaddy_walk3d::Feeder,
    feet: f64,
}

/// The feeders to the panels of the units (a typical panel's copies), each routed one with its
/// length (the conduit's own: its runs over the conduits it is), and how many aren't routed.
fn unit_feeders(sp: &cadaddy_walk3d::BuildingSpec) -> (Vec<UnitFeeder>, usize) {
    let upf = if sp.units_per_foot.is_finite() && sp.units_per_foot > 0.0 { sp.units_per_foot } else { 12.0 };
    let typical: Vec<String> = sp.mep.panel_schedules.iter().filter(|p| p.typical_of.is_some()).map(|p| p.panel.to_ascii_uppercase()).collect();
    let mut length: BTreeMap<&str, f64> = BTreeMap::new();
    for r in sp.mep.runs.iter().take(MAX_RUNS).filter(|r| r.system == super::gearbuild::FEEDER_SYSTEM) {
        let flat: f64 = r.points.windows(2).map(|w| ((w[1][0] - w[0][0]).powi(2) + (w[1][1] - w[0][1]).powi(2)).sqrt() / upf).sum();
        let up = r.top_ft.map_or(0.0, |t| (t - r.bottoms_ft.first().copied().unwrap_or(t)).abs());
        let v = flat + up;
        if v.is_finite() {
            *length.entry(r.source_sheet.as_str()).or_default() += v;
        }
    }
    let mut out = Vec::new();
    let mut not_routed = 0usize;
    let mut used: Vec<&str> = Vec::new();
    for f in sp.mep.feeders.iter().take(MAX_FEEDERS).filter(|f| typical.contains(&f.to.to_ascii_uppercase())) {
        let head = format!("feeder {} {}→", f.tag, f.from);
        let fam = format!("{}-", f.to.to_ascii_uppercase());
        // Its route: one not taken yet, to its panel or one of its copies.
        let route = length.iter().find(|(src, _)| {
            !used.contains(src)
                && src.strip_prefix(head.as_str()).is_some_and(|rest| {
                    let first = rest.split('→').next().unwrap_or_default().to_ascii_uppercase();
                    first == f.to.to_ascii_uppercase() || first.starts_with(&fam)
                })
        });
        match route {
            Some((src, feet)) => {
                used.push(src);
                out.push(UnitFeeder { feeder: f.clone(), feet: feet / f64::from(f.conduits.max(1)) });
            }
            None => not_routed += 1,
        }
    }
    (out, not_routed)
}

/// The lines (item, size, feet) each way: as designed (conduit and its conductors) and the
/// alternative (the cable).
fn feeder_lines(feeders: &[UnitFeeder], cable: &str) -> (Vec<(String, String, f64)>, Vec<(String, String, f64)>) {
    let mut designed: BTreeMap<(String, String), f64> = BTreeMap::new();
    let mut alt: BTreeMap<(String, String), f64> = BTreeMap::new();
    for uf in feeders {
        let f = &uf.feeder;
        let l = uf.feet;
        let al = f.material.trim().eq_ignore_ascii_case("al");
        let wire = if al { "XHHW aluminum" } else { "THHN copper" };
        let sets = f64::from(f.sets.max(1));
        let phase = super::ugsheets::wire_size(&f.size);
        let neutral = super::ugsheets::wire_size(if f.neutral_size.is_empty() { &f.size } else { &f.neutral_size });
        let ground = super::ugsheets::wire_size(&f.ground_size);
        let size = if f.conduit_in.is_finite() && f.conduit_in > 0.0 { f.conduit_in } else { 1.0 };
        let add = |m: &mut BTreeMap<(String, String), f64>, item: &str, sz: &str, q: f64| {
            if q > 0.0 && q.is_finite() {
                *m.entry((item.to_string(), sz.to_string())).or_default() += q;
            }
        };
        add(&mut designed, "EMT conduit", &super::ugsheets::trade_size(size), l * f64::from(f.conduits.max(1)));
        add(&mut designed, wire, &phase, l * sets * f64::from(f.phases));
        add(&mut designed, wire, &neutral, l * sets * f64::from(f.neutrals));
        add(&mut designed, wire, &ground, l * sets * f64::from(f.grounds));
        // The cable as sold: each conductor's size, the ground last (`2/0-2/0-2/0-4`).
        let bare = |v: &str| v.trim_start_matches('#').to_string();
        let mut sizes: Vec<String> = std::iter::repeat_n(bare(&phase), f.phases as usize).collect();
        sizes.extend(std::iter::repeat_n(bare(&neutral), f.neutrals as usize));
        sizes.extend(std::iter::repeat_n(bare(&ground), f.grounds as usize));
        let item = format!("{cable} {}", if al { "aluminum" } else { "copper" });
        add(&mut alt, &item, &sizes.join("-"), l * sets);
    }
    let flat = |m: BTreeMap<(String, String), f64>| m.into_iter().map(|((i, s), q)| (i, s, (q * 10.0).round() / 10.0)).collect();
    (flat(designed), flat(alt))
}

/// The lines priced (`estimate.change`, each line as the estimate prices it).
fn price(s: &mut Session, lines: &[(String, String, f64)]) -> Value {
    if lines.is_empty() {
        return json!({"lines": [], "material": 0.0, "labor_hours": 0.0, "cost": 0.0, "unpriced": []});
    }
    let ls: Vec<Value> = lines.iter().map(|(i, sz, q)| json!({"item": i, "size": sz, "unit": "ft", "qty": q})).collect();
    match s.execute("estimate.change", &json!({"lines": ls})) {
        Ok(v) => json!({
            "lines": lines.iter().map(|(i, sz, q)| json!({"item": i, "size": sz, "ft": q})).collect::<Vec<_>>(),
            "material": v["totals"]["material"], "labor_hours": v["totals"]["labor_hours"], "cost": v["totals"]["cost"],
            "unpriced": v["unpriced"],
        }),
        Err(e) => {
            json!({"lines": lines.iter().map(|(i, sz, q)| json!({"item": i, "size": sz, "ft": q})).collect::<Vec<_>>(), "error": e.to_string()})
        }
    }
}

/// The spec files' text, each with its section (`26 05 19`): PDFs and text files, or the
/// Division 26 files in folders.
fn spec_texts(paths: &[String]) -> Vec<(String, String)> {
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

/// The alternatives against the documents and the model: each one's tier, quantities, prices
/// both ways and (needing an RFI) its draft.
pub(crate) fn study(s: &mut Session, applications: &[Application], construction: Option<&str>) -> Result<Vec<Value>> {
    let rules = rules();
    let (sp, _) = super::bspec::current(s, CMD)?;
    let (feeders, not_routed) = unit_feeders(&sp);
    let mut out = Vec::new();
    for a in &rules.alternatives {
        let app = applications.iter().find(|ap| {
            let up = ap.application.to_ascii_uppercase();
            a.application.iter().all(|w| up.contains(w.as_str())) && !a.application_not.iter().any(|w| up.contains(w.as_str()))
        });
        let code_ok = construction.is_some_and(|c| a.construction.iter().any(|k| k.eq_ignore_ascii_case(class_of(c))));
        let tier = match (app, code_ok) {
            (None, _) => "no_application",
            (Some(_), false) => "not_allowed",
            (Some(ap), true) if ap.methods.contains(&a.alternative) => "allowed_now",
            (Some(_), true) => "needs_rfi",
        };
        let mut v = json!({
            "id": a.id, "name": a.name, "tier": tier, "code": a.code, "conditions": a.conditions, "excluded": a.excluded, "intent": a.intent,
            "construction_type": construction,
            "application": app.map(|ap| json!({"section": ap.section, "item": ap.item, "application": ap.application, "methods": ap.methods})),
        });
        if a.circuit == "feeder" && a.to == "unit_panel" && !feeders.is_empty() {
            let cable = match a.alternative.as_str() {
                "se" => "SER cable",
                "mc" => "MC cable",
                other => other,
            };
            let (designed, alt) = feeder_lines(&feeders, cable);
            let d = price(s, &designed);
            let x = price(s, &alt);
            let n = |v: &Value, k: &str| v[k].as_f64().unwrap_or(0.0);
            let feet: f64 = feeders.iter().map(|f| f.feet).sum();
            v["quantity"] = json!({"feeders": feeders.len(), "feet": (feet * 10.0).round() / 10.0, "not_routed": not_routed});
            let mut unpriced: Vec<Value> = d["unpriced"].as_array().cloned().unwrap_or_default();
            unpriced.extend(x["unpriced"].as_array().cloned().unwrap_or_default());
            // Saves only when every line both ways has a price (else each way's priced part).
            v["saves"] = if unpriced.is_empty() {
                json!({
                    "material": n(&d, "material") - n(&x, "material"),
                    "labor_hours": n(&d, "labor_hours") - n(&x, "labor_hours"),
                    "cost": n(&d, "cost") - n(&x, "cost"),
                })
            } else {
                Value::Null
            };
            v["unpriced"] = json!(unpriced);
            v["as_designed"] = d;
            v["alternative"] = x;
        } else {
            v["quantity"] = json!({"measured": false, "why": if a.circuit == "branch" { "the model's branch wiring lengths aren't measured yet" } else { "no routed feeders to the units' panels" }});
        }
        if tier == "needs_rfi" {
            let ap = app.map(|ap| format!("{} §{} ({})", ap.section, ap.item, ap.application)).unwrap_or_default();
            let cites = a.code.iter().map(|c| c.cite.as_str()).collect::<Vec<_>>().join(", ");
            let saves = v["saves"]["cost"]
                .as_f64()
                .map(|c| format!(" It saves about ${c:.0} and {:.0} labor hours by our prices.", v["saves"]["labor_hours"].as_f64().unwrap_or(0.0)))
                .unwrap_or_default();
            v["rfi"] = json!({
                "issue": format!(
                    "{ap} lists {} only. The building is Type {} construction, where {cites} permit {}. We propose it, under these conditions: {}; never in {}. {}{saves}",
                    a.designed, construction.unwrap_or("?"), a.name.to_lowercase(), a.conditions.join("; "), a.excluded.join(", "), a.intent
                ),
                "proposed": format!("Approve {} as an alternative to the specified method where the conditions hold", a.name.to_lowercase()),
            });
        }
        out.push(v);
    }
    Ok(out)
}

fn run(s: &mut Session, p: &Value) -> Result<Value> {
    let paths: Vec<String> = match p.get("specs") {
        None | Some(Value::Null) => Vec::new(),
        Some(Value::String(t)) => vec![t.clone()],
        Some(Value::Array(a)) => a.iter().filter_map(Value::as_str).map(str::to_string).take(MAX_SPEC_FILES).collect(),
        Some(_) => return Err(bad(CMD, "`specs` takes a path or a list of paths")),
    };
    let mut applications = Vec::new();
    for (section, text) in spec_texts(&paths) {
        applications.extend(wiring_applications(&text, &section));
    }
    let given = p.get("construction_type").and_then(Value::as_str).map(str::trim).filter(|t| !t.is_empty()).map(str::to_ascii_uppercase);
    let code_texts: Vec<String> =
        p.get("code_texts").and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_str).map(str::to_string).take(20_000).collect();
    let construction = given.or_else(|| construction_type(&code_texts));
    let scenarios = study(s, &applications, construction.as_deref())?;
    Ok(json!({
        "construction_type": construction,
        "applications": applications.iter().map(|a| json!({"section": a.section, "item": a.item, "application": a.application, "methods": a.methods})).collect::<Vec<_>>(),
        "scenarios": scenarios,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Section 26 05 19's wiring-methods article as a PDF's text reads (an item across a page
    /// break, its footer in the middle).
    const SPEC: &str = "PART 3 - EXECUTION\n3.1\nCONDUCTOR MATERIAL APPLICATIONS\nA.\nFeeders: Copper.\n3.2\nCONDUCTOR INSULATION AND MULTICONDUCTOR CABLE APPLICATIONS AND\nWIRING METHODS\nA.\nService Entrance: Type THHN/THWN-2, single conductors in raceway.\nB.\nExposed Feeders: Type THHN/THWN-2, single conductors in raceway.\nC.\nFeeders \nConcealed \nin \nCeilings, \nWalls, \nPartitions, \nand \nCrawlspaces: \nType THHN/THWN-2, single conductors in raceway.\nD.\nFeeders Concealed in Concrete, below Slabs-on-Grade, and Underground: \nType THHN/THWN-2, single conductors in raceway.\nF.\nBranch Circuits Concealed in Ceilings, Walls, and Partitions: Type THHN/THWN-2, \nsingle conductors in raceway, Armored cable, Type AC, Metal-clad cable, Type MC, \nLOW-VOLTAGE ELECTRICAL POWER CONDUCTORS AND CABLES\n260519 - 5\nNonmetallic-sheathed cable, Type NM. Type NM is preferred.\n3.3\nINSTALLATION OF CONDUCTORS AND CABLES\nA.\nConceal cables.";

    #[test]
    fn a_specs_wiring_methods_table_says_each_applications_methods() {
        let apps = wiring_applications(SPEC, "26 05 19");
        assert_eq!(apps.len(), 5, "{apps:#?}");
        let c = &apps[2];
        assert_eq!((c.item.as_str(), c.application.as_str()), ("C", "Feeders Concealed in Ceilings, Walls, Partitions, and Crawlspaces"));
        assert_eq!(c.methods, ["raceway"]);
        let f = &apps[4];
        assert_eq!(f.methods, ["ac", "mc", "nm", "raceway"]);
        assert!(wiring_applications("3.2\nSOMETHING ELSE\nA.\nX: Type NM.", "26 05 19").is_empty());
    }

    #[test]
    fn texts_of_any_heights_sort_into_reading_order_without_panicking() {
        // Texts of many heights (a comparator keyed on one text's height isn't a total order and
        // made the sort panic on a real code sheet).
        let item = |x: f64, y: f64, h: f64, v: &str| cadaddy_mepfix::sheet_facts::TextItem {
            value: v.to_string(),
            at: cadaddy_geom::Vec2::new(x, y),
            height: h,
            rotation: 0.0,
        };
        let mut texts: Vec<cadaddy_mepfix::sheet_facts::TextItem> =
            (0..400).map(|i| item(f64::from(i * 37 % 101), f64::from(i * 53 % 97), [0.5, 3.0, 9.0, 1e-9, 40.0][i as usize % 5], "x")).collect();
        texts.push(item(f64::NAN, 1.0, 1.0, "nan"));
        texts.push(item(0.0, 500.0, 2.0, "CONSTRUCTION TYPE"));
        texts.push(item(30.0, 500.2, 2.0, "V-A"));
        let words = reading_order(texts);
        assert_eq!(words.len(), 402);
        assert_eq!(&words[..2], ["CONSTRUCTION TYPE", "V-A"]);
        assert_eq!(construction_type(&words).as_deref(), Some("V-A"));
    }

    #[test]
    fn the_code_sheet_gives_the_construction_type() {
        let t = |v: &[&str]| v.iter().map(|x| x.to_string()).collect::<Vec<_>>();
        assert_eq!(construction_type(&t(&["CONSTRUCTION TYPE                 V-A"])).as_deref(), Some("V-A"));
        assert_eq!(construction_type(&t(&["CONSTRUCTION TYPE PER TABLE 601", "TYPE V-A"])).as_deref(), Some("V-A"));
        assert_eq!(construction_type(&t(&["TYPE OF CONSTRUCTION: II-B"])).as_deref(), Some("II-B"));
        assert_eq!(construction_type(&t(&["OCCUPANCY R-1"])), None);
    }

    #[test]
    fn ser_for_the_units_feeders_needs_an_rfi_and_nm_is_allowed_now_priced_from_the_routes() {
        let mut s = Session::new();
        let spec = json!({
            "version": 1, "units_per_foot": 12,
            "levels": [{"name": "Level 1", "elevation_ft": 0, "height_ft": 10, "ceiling_ft": 9}],
            "mep": {
                "systems": [{"name": "E-FEEDER", "layer": "E-FEEDER", "kind": "conduit", "size_in": [2, 2], "bottom_ft": 10}],
                "panel_schedules": [{"panel": "G", "typical_of": 2}],
                "feeders": [
                    {"tag": "123", "from": "DP1", "to": "G", "phases": 2, "neutrals": 1, "grounds": 1, "size": "2/0", "ground_size": "4", "material": "al", "conduits": 1, "conduit_in": 2, "sets": 1},
                    {"tag": "123", "from": "DP1", "to": "G", "phases": 2, "neutrals": 1, "grounds": 1, "size": "2/0", "ground_size": "4", "material": "al", "conduits": 1, "conduit_in": 2, "sets": 1}
                ],
                "runs": [
                    {"system": "E-FEEDER", "level": "Level 1", "source_sheet": "feeder 123 DP1→G-101→G-201", "points": [[0, 0], [1200, 0]], "size_in": [2, 2], "bottoms_ft": [10]},
                    {"system": "E-FEEDER", "level": "Level 1", "source_sheet": "feeder 123 DP1→G-101→G-201", "points": [[1200, 0]], "size_in": [2, 2], "bottoms_ft": [0], "top_ft": 20}
                ]
            }
        });
        s.execute("bspec.set", &json!({"replace": true, "spec": spec})).unwrap();
        let apps = wiring_applications(SPEC, "26 05 19");
        let out = study(&mut s, &apps, Some("V-A")).unwrap();
        let ser = out.iter().find(|v| v["id"] == "se_unit_feeders").unwrap();
        assert_eq!(ser["tier"], "needs_rfi", "{ser}");
        assert_eq!(ser["application"]["item"], "C");
        // One feeder routed (100 ft across and 20 up), the other not.
        assert_eq!(ser["quantity"], json!({"feeders": 1, "feet": 120.0, "not_routed": 1}));
        let alt = ser["alternative"]["lines"].as_array().unwrap();
        assert_eq!(alt, &vec![json!({"item": "SER cable aluminum", "size": "2/0-2/0-2/0-4", "ft": 120.0})]);
        let designed = ser["as_designed"]["lines"].as_array().unwrap();
        assert!(designed.contains(&json!({"item": "EMT conduit", "size": "2\"", "ft": 120.0})), "{designed:?}");
        assert!(designed.contains(&json!({"item": "XHHW aluminum", "size": "#2/0", "ft": 360.0})), "{designed:?}");
        assert!(ser["rfi"]["issue"].as_str().unwrap().contains("NEC 338.10(B)(4)(a)"));
        // Both ways priced from the starter book: SER saves the conduit and most of the pulling.
        assert_eq!(ser["unpriced"], json!([]), "{ser}");
        let saves = |k: &str| ser["saves"][k].as_f64().unwrap_or(f64::NAN);
        assert!((saves("material") - 144.24).abs() < 0.01, "{ser}");
        assert!((saves("labor_hours") - 18.0).abs() < 0.01, "{ser}");
        assert!(ser["rfi"]["issue"].as_str().unwrap().contains("saves"), "{ser}");
        let nm = out.iter().find(|v| v["id"] == "nm_unit_branch").unwrap();
        assert_eq!(nm["tier"], "allowed_now", "{nm}");
        assert!(nm.get("rfi").is_none());
        // Type I construction: the code allows neither.
        let out = study(&mut s, &apps, Some("I-A")).unwrap();
        assert!(out.iter().all(|v| v["tier"] == "not_allowed"), "{out:?}");
    }
}
