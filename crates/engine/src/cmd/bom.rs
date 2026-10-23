//! The job's bill of materials (Q326): every modelled item and the small parts the rules imply,
//! across the trades, with waste shown apart from the net, each line's status, the required job
//! items (tools, consumables, setup, labeling), a check that what the documents require has a
//! line, and what changed since the last BOM.
//!
//! `bim.bom` (typed `BOM`):
//! - **Electrical** from the element tags (Q299): luminaires by schedule type, devices by type,
//!   each with its type mark, manufacturer, model, finish, colour and options (an approved
//!   submittal's over the tag's); a box, plaster ring (by the wall's assembly) and plate per wall
//!   device; branch conduit by material and size with its couplings, connectors, straps and
//!   elbows; and each homerun or feeder from its conductor note (`3#4 + 1#4N + 1#8G in 1-1/4"
//!   RMC`): its wire over the route plus makeup, and its raceway (material, size, count) with the
//!   same fittings, a bushing at each raceway end where the conductors are large (bang-on on
//!   EMT/ENT, screw-on on RMC/IMC/PVC, grounding bushings on a service or bonded feeder); a
//!   cover/trim per panelboard; the labels (`labels.bom`: one arc flash label per labeled piece of
//!   gear) and the grounding (`grounding.takeoff`).
//! - **The implied-part rules** are the project rules' (Q316, `custom`): `bom.stick_ft[.<mat>]`,
//!   `bom.strap_spacing_ft[.<mat>]`, `bom.makeup_ft`, `bom.bushing_min_awg`, `bom.box[.<kind>]`,
//!   `bom.ring[.<wall assembly>|.<kind>]`, `bom.plate[.<kind>]`, `bom.boxed_devices`,
//!   `bom.conduit_material`, `bom.material.<system>`, `bom.panel_cover`, … with today's values as
//!   the defaults; each line's `rule` names the key it used.
//! - **Mechanical and plumbing** from `mep.qty` (Q269): duct by size and gauge (ft, lb), pipe by
//!   system and size, fittings, terminals, equipment, hangers; flex duct and a volume damper per
//!   air terminal, duct wrap on supply and outside air duct, pipe insulation on hot and chilled
//!   water; plumbing fixtures, valves, cleanouts and water heaters by model.
//! - **Drywall** from `drywall.takeoff` and `rcp.takeoff` by level, wall and ceiling board apart,
//!   with studs, track, corner bead, tape, mud and screws.
//! - **Framing** from the spec's framing takeoff (Q303): members by section, length and status.
//! - **Concrete** from the concrete takeoff (Q328): each element in our scope (the concrete
//!   rules' `our_scope`) gives its concrete (cubic yards, with the takeoff's waste), forms, rebar
//!   by bar size, welded wire, anchor bolts, embeds and stub-ups by size, finish and curing, each
//!   line carrying its element ids; every other element is listed in `by_others` as
//!   `by others (<owner>)` or `unassigned`, at no cost to us.
//! - **Status** in the doctrine's words — `fixed`, `rule`, `zone`, `field_verified`, `override`,
//!   `agent` — from the element's own status, else from where its fields came from (a document
//!   fixes it, a person overrides it, a default is a rule); a part a rule implies is `rule`.
//! - **Waste** per category (defaults below; project rules `custom` `waste.<category>` or the
//!   `waste` param override), shown as its own column.
//! - **Required job items** from the modelled work, the crew, the floors and the duration, each
//!   with the rule that requires it; a line is zeroed only with a reason (`waive`), recorded in
//!   the project rules (`custom` `bom.waive.<item>`). The BOM is `complete` when every required
//!   line is there or waived and every spec check is clean or waived.
//! - **Spec check**: each catalog type (Q297/Q298) whose finish/colour/options the documents fix
//!   is carried only by a line with that mark and that attribute; a line with that mark saying
//!   otherwise contradicts the spec (with the source of that field); separately sold accessories
//!   (plates for devices, covers/trims for panels) must have their lines.
//! - `compare_to`: an earlier BOM's `lines` → added, removed and changed quantities.
//! - `group_by`: `job` (default), `level`, `trade`, `room`, `phase` or `area` (rooms, phases
//!   and areas/unit types where the elements carry them); `file` writes CSV, XLSX or a PDF
//!   summary (every category's total), `columns` renames the columns for another estimating tool.

use cadaddy_doc::projrules::{ProjectRules, RuleDomain, RuleValue};
use cadaddy_walk3d::BuildingSpec;
use cadaddy_walk3d::tags::ElementTag;
use serde_json::{Value, json};
use std::collections::BTreeMap;

use super::bspec::current;
use super::*;
use crate::{Result, Session};

/// Lines in one BOM, at most (hostile specs).
const MAX_LINES: usize = 50_000;
/// A conduit stick, feet (a coupling joins each two), unless the rules say otherwise.
const STICK_FT: f64 = 10.0;
/// Conduit supports: within 3' of each end and every 10' between (NEC 358.30 for EMT).
const STRAP_SPACING_FT: f64 = 10.0;
/// Wire left at each end for makeup, feet.
const MAKEUP_FT: f64 = 1.0;
/// The crew, when neither the call nor the rules say.
const CREW: f64 = 4.0;
/// How high a step ladder works from (feet to the work); above it a lift.
const LADDER_REACH_FT: f64 = 12.0;

/// Default waste, percent, by category.
fn default_waste(category: &str) -> f64 {
    match category {
        "conduit" | "wire" | "duct" | "pipe" | "insulation" => 10.0,
        "conduit fittings" | "fittings" | "boxes" | "hangers" => 5.0,
        "drywall" => 12.0,
        "devices" | "fixtures" => 2.0,
        _ => 0.0,
    }
}

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new("bim.bom", "Bill of Materials", run)
            .alias(&["bom"])
            .params(
                "{group_by?: job|level|trade|room|phase|area, all_elements?: bool (each line's every element, not the first 500), waste?: {category: percent}, waive?: [{item, reason}] (a required item or a spec check's item), crew?: people (default rules bom.crew, 4), floors?: count (default the levels), weeks?: duration (default rules bom.weeks), compare_to?: {lines} (an earlier BOM), file?: path (.csv/.xlsx/.pdf: the PDF is a summary, every category's total), columns?: {trade|category|item|size|unit|net|waste|total|status|rule|level: header}} → {lines: [{trade, category, item, size, unit, net, waste_pct, waste, total, status: fixed|rule|zone|field_verified|override|agent, rule?, mark?, attributes?: {finish|color|options|manufacturer|model: value}, attribute_sources?, level?, room?, phase?, area?, elements}], summary: [{trade, category, unit, total}], by_others: [{trade, element, kind, level, owner, scope: by others (owner)|unassigned, cy, cost_to_us: 0}] (concrete not ours), required: [{item, qty, supply: purchase|rental|owned, rule, weeks?, waived?}], spec_check: [{issue: requirement_without_line|attribute_contradicts_spec|accessory_missing, item, field?, spec?, line?, source?, detail, waived?}], complete, changes?: {added, removed, changed}, csv, path?}",
            ),
    ]
}

/// One BOM line before grouping.
#[derive(Clone, Debug, Default)]
struct Line {
    trade: &'static str,
    category: String,
    item: String,
    size: String,
    unit: &'static str,
    net: f64,
    /// The doctrine's status: `fixed`, `rule`, `zone`, `field_verified`, `override`, `agent`.
    status: &'static str,
    rule: Option<String>,
    level: String,
    elements: Vec<String>,
    /// Waste the takeoff already worked out (concrete: by each element's size), in the line's
    /// unit; `None`: the category's percent.
    waste_qty: Option<f64>,
    /// The catalog type mark (`A`, `WP1`).
    mark: String,
    /// Finish, colour, options, manufacturer, model: (value, where it came from).
    attrs: BTreeMap<String, (String, String)>,
    /// Where the elements carry them: the room, the phase (Q284), the area or unit type.
    room: String,
    phase: String,
    area: String,
}

impl Line {
    /// The line's identity; `dims` the groupings kept apart (level, room, phase, area).
    fn key(&self, dims: &[&str]) -> String {
        let dim = |d: &str, v: &str| if dims.contains(&d) { v.to_string() } else { String::new() };
        let attrs: Vec<String> = self.attrs.iter().map(|(k, (v, _))| format!("{k}={v}")).collect();
        format!(
            "{}|{}|{}|{}|{}|{}|{}|{}|{}|{}",
            self.trade,
            self.category,
            self.item,
            self.size,
            self.mark,
            attrs.join(";"),
            dim("level", &self.level),
            dim("room", &self.room),
            dim("phase", &self.phase),
            dim("area", &self.area)
        )
    }
}

const ALL_DIMS: [&str; 4] = ["level", "room", "phase", "area"];

/// Add `l` into `lines`, merging with an equal line (same trade, category, item, size, identity,
/// place).
fn add(lines: &mut Vec<Line>, l: Line) {
    if l.net <= 0.0 || !l.net.is_finite() || lines.len() >= MAX_LINES {
        return;
    }
    let k = l.key(&ALL_DIMS);
    match lines.iter_mut().find(|x| x.status == l.status && x.rule == l.rule && x.key(&ALL_DIMS) == k) {
        Some(x) => merge(x, l),
        None => lines.push(l),
    }
}

/// `l`'s quantities and elements into `x`.
fn merge(x: &mut Line, l: Line) {
    x.net += l.net;
    x.waste_qty = match (x.waste_qty, l.waste_qty) {
        (None, None) => None,
        (a, b) => Some(a.unwrap_or(0.0) + b.unwrap_or(0.0)),
    };
    x.elements.extend(l.elements);
}

/// A run's plan length, feet.
pub(super) fn run_length_ft(points: &[[f64; 2]], upf: f64) -> f64 {
    points.windows(2).map(|w| if let [a, b] = w { (b[0] - a[0]).hypot(b[1] - a[1]) } else { 0.0 }).sum::<f64>() / upf
}

/// Conductors `2#12 + 1#12G` (or `2#12, 1#12G`) → (count, size, ground?) parts.
fn conductor_parts(makeup: &str) -> Vec<(u32, String, bool)> {
    makeup
        .split(" in ")
        .next()
        .unwrap_or_default()
        .split(['+', ','])
        .filter_map(|p| {
            let (n, rest) = p.trim().split_once('#')?;
            let n = n.trim().parse::<u32>().ok().unwrap_or(1).min(12);
            let size: String = rest.chars().take_while(|c| c.is_ascii_digit() || *c == '/').collect();
            let ground = rest.get(size.len()..).is_some_and(|r| r.starts_with('G'));
            (!size.is_empty()).then_some((n, size, ground))
        })
        .collect()
}

/// An AWG size as an order (smaller is larger wire): `12` → 12, `1/0` → 0, `4/0` → -3, `250`
/// kcmil → -4 and down.
fn awg_order(size: &str) -> Option<i32> {
    let s = size.trim().trim_start_matches('#').trim();
    if let Some((n, "0")) = s.split_once('/') {
        return n.trim().parse::<i32>().ok().filter(|n| (1..=4).contains(n)).map(|n| 1 - n);
    }
    let v = s.split_whitespace().next()?.parse::<i32>().ok()?;
    Some(if v >= 100 { -4 - v / 100 } else { v })
}

/// AWG #4 and larger (`4`, `2`, `1/0`…): conductors that need insulating bushings (NEC 300.4(G)).
fn large_conductor(size: &str) -> bool {
    large_at(size, 4)
}

/// A conductor at least as large as AWG `min` (an order from [`awg_order`]).
fn large_at(size: &str, min: i32) -> bool {
    awg_order(size).is_some_and(|o| o <= min)
}

// ---- The rules ----------------------------------------------------------------------------------

/// The implied-part rules: the project rules' `custom` `bom.<key>` (Q316) over the defaults.
struct Rules<'a>(&'a ProjectRules);

impl Rules<'_> {
    fn text(&self, key: &str) -> Option<String> {
        self.0.resolve(RuleDomain::Custom, key).map(|v| v.value.trim().to_string()).filter(|v| !v.is_empty())
    }
    /// The first of `keys` the rules set to a number in `range`, else `default`; and the key named
    /// on the line (the one set, else the first, with `default` said).
    fn num(&self, keys: &[String], default: f64, range: std::ops::RangeInclusive<f64>) -> (f64, String) {
        for k in keys {
            if let Some(v) =
                self.text(k).and_then(|v| v.trim_end_matches(['\'', '"', '%', ' ']).trim().parse::<f64>().ok()).filter(|v| range.contains(v))
            {
                return (v, k.clone());
            }
        }
        (default, format!("{}, default", keys.first().cloned().unwrap_or_default()))
    }
    /// A part named by the rules (`item; size`, or the size alone), the first of `keys` set; else
    /// the default; and the key.
    fn part(&self, keys: &[String], default: (&str, &str)) -> (String, String, String) {
        for k in keys {
            if let Some(v) = self.text(k) {
                return match v.split_once(';') {
                    Some((item, size)) => (item.trim().to_string(), size.trim().to_string(), k.clone()),
                    None => (default.0.to_string(), v, k.clone()),
                };
            }
        }
        (default.0.to_string(), default.1.to_string(), format!("{}, default", keys.last().cloned().unwrap_or_default()))
    }
    fn yes(&self, key: &str, default: bool) -> bool {
        self.text(key).map_or(default, |v| truthy(&v))
    }
}

fn truthy(v: &str) -> bool {
    matches!(v.trim().to_ascii_lowercase().as_str(), "yes" | "y" | "true" | "1" | "on")
}

// ---- Status --------------------------------------------------------------------------------------

/// The doctrine's status: the element's own (`fixed`, `rule`, `zone`, `field_verified`,
/// `override`, `agent`), else from where its field came from — the documents (plan, schedule,
/// note, spec, submittal) fix it, a person (`user`) overrides it, the field verifies it, and a
/// rule or a default is a rule.
fn status_of(own: &str, source: &str) -> &'static str {
    if let Some(st) = cadaddy_walk3d::validate::Status::parse(own) {
        return st.name();
    }
    match source.trim().to_ascii_lowercase().as_str() {
        "plan" | "schedule" | "note" | "spec" | "submittal" | "detail" | "print" => "fixed",
        "user" | "override" => "override",
        "field" | "field_verified" | "verified" | "as_built" => "field_verified",
        "agent" => "agent",
        "zone" => "zone",
        _ => "rule",
    }
}

// ---- Identity: type mark and attributes ------------------------------------------------------------

/// The attributes a line carries and the field names they are read from.
const ATTRS: [(&str, &[&str]); 5] = [
    ("finish", &["finish"]),
    ("color", &["color", "colour"]),
    ("options", &["options"]),
    ("manufacturer", &["manufacturer", "mfr"]),
    ("model", &["model", "catalog", "catalognumber", "catalog_number"]),
];

/// The attribute a field name is, if it is one.
fn attr_of(field: &str) -> Option<&'static str> {
    let f = field.trim().to_ascii_lowercase().replace([' ', '-'], "_");
    ATTRS.iter().find(|(_, names)| names.iter().any(|n| f == *n || f.replace('_', "") == *n)).map(|(a, _)| *a)
}

/// The approved submittals' fields by type mark (lower case): (attribute, value, source).
fn approved(sp: &BuildingSpec) -> BTreeMap<String, Vec<(&'static str, String, String)>> {
    let mut out: BTreeMap<String, Vec<(&'static str, String, String)>> = BTreeMap::new();
    for s in super::validate::prints_of(sp).map(|p| p.submittals.as_slice()).unwrap_or_default() {
        for (k, v) in &s.fields {
            if let Some(a) = attr_of(k).filter(|_| !v.trim().is_empty()) {
                out.entry(s.mark.trim().to_ascii_lowercase()).or_default().push((
                    a,
                    v.trim().to_string(),
                    format!("approved submittal {}", s.section),
                ));
            }
        }
    }
    out
}

/// A tag's type mark and attributes: its fields, the approved submittal's over them.
fn identity(
    t: &ElementTag,
    mark_field: &[&str],
    subs: &BTreeMap<String, Vec<(&'static str, String, String)>>,
) -> (String, BTreeMap<String, (String, String)>) {
    let mark = mark_field.iter().find_map(|k| t.fields.get(*k)).map(|f| f.value.trim().to_string()).unwrap_or_default();
    let mut attrs: BTreeMap<String, (String, String)> = BTreeMap::new();
    for (k, f) in &t.fields {
        if let Some(a) = attr_of(k).filter(|_| !f.value.trim().is_empty()) {
            attrs.insert(a.to_string(), (f.value.trim().to_string(), f.source.clone()));
        }
    }
    if let Some(list) = subs.get(&mark.to_ascii_lowercase()).filter(|_| !mark.is_empty()) {
        for (a, v, src) in list {
            attrs.insert((*a).to_string(), (v.clone(), src.clone()));
        }
    }
    (mark, attrs)
}

/// Where a tag is: its room, phase and area (or unit type), where it carries them.
fn place(t: &ElementTag) -> (String, String, String) {
    let f = |ks: &[&str]| ks.iter().find_map(|k| t.fields.get(*k)).map(|v| v.value.trim().to_string()).unwrap_or_default();
    (f(&["room"]), f(&["phase"]), f(&["area", "unit_type"]))
}

// ---- Raceway ---------------------------------------------------------------------------------------

/// A raceway as a conductor note names it: `in 1-1/4" RMC`, `in (2) 2" PVC`, `in MC`.
#[derive(Clone, Debug, PartialEq)]
struct Raceway {
    count: u32,
    /// Trade size, inches (0: a cable assembly's, not said).
    size_in: f64,
    /// `EMT`, `ENT`, `RMC`, `IMC`, `PVC`, `FMC`, `LFMC`, `MC`, `AC`, `NM`.
    material: String,
}

/// A trade size: `3/4`, `1`, `1-1/4`, `1 1/4`.
fn parse_trade_size(s: &str) -> Option<f64> {
    let s = s.trim();
    let frac = |f: &str| -> Option<f64> {
        let (n, d) = f.split_once('/')?;
        let (n, d) = (n.trim().parse::<f64>().ok()?, d.trim().parse::<f64>().ok()?);
        (d > 0.0).then_some(n / d)
    };
    let v = if let Some((whole, f)) = s.split_once(['-', ' ']).filter(|(_, f)| f.contains('/')) {
        whole.trim().parse::<f64>().ok()? + frac(f)?
    } else if s.contains('/') {
        frac(s)?
    } else {
        s.parse::<f64>().ok()?
    };
    (v > 0.0 && v <= 8.0).then_some(v)
}

/// A raceway material's name as the BOM writes it (`RGS`/`GRC` are RMC).
fn material(word: &str) -> Option<String> {
    let w = word.trim().trim_matches(|c: char| !c.is_ascii_alphanumeric()).to_ascii_uppercase();
    Some(match w.as_str() {
        "EMT" | "ENT" | "RMC" | "IMC" | "PVC" | "FMC" | "LFMC" | "MC" | "AC" | "NM" => w,
        "RGS" | "GRC" | "RIGID" => "RMC".into(),
        "FLEX" => "FMC".into(),
        "SEALTITE" | "LIQUIDTIGHT" => "LFMC".into(),
        _ => return None,
    })
}

/// The raceway a conductor note names, after ` in `.
fn parse_raceway(note: &str) -> Option<Raceway> {
    let (_, rest) = note.split_once(" in ")?;
    let mut r = rest.trim();
    let mut count = 1;
    if let Some(x) = r.strip_prefix('(') {
        let (n, after) = x.split_once(')')?;
        count = n.trim().parse::<u32>().ok()?.clamp(1, 40);
        r = after.trim();
    }
    match r.split_once('"') {
        Some((size, mat)) => Some(Raceway { count, size_in: parse_trade_size(size)?, material: material(mat.split_whitespace().next()?)? }),
        None => Some(Raceway { count, size_in: 0.0, material: material(r.split_whitespace().next()?)? }),
    }
}

/// A cable assembly (its conductors inside it), not a raceway they are pulled into.
fn cable_assembly(mat: &str) -> bool {
    matches!(mat, "MC" | "AC" | "NM")
}

/// Bent in place: no elbows.
fn flexible(mat: &str) -> bool {
    matches!(mat, "ENT" | "FMC" | "LFMC" | "MC" | "AC" | "NM")
}

/// Sticks, feet (0: a coil, no couplings).
fn default_stick(mat: &str) -> f64 {
    if flexible(mat) { 0.0 } else { STICK_FT }
}

/// Supports, feet apart (NEC 358.30 EMT, 344/342.30 RMC/IMC, 352.30 PVC, 362.30 ENT, 348.30
/// FMC, 330.30 MC, 334.30 NM).
fn default_strap(mat: &str) -> f64 {
    match mat {
        "PVC" | "ENT" => 3.0,
        "FMC" | "LFMC" | "NM" => 4.5,
        "MC" | "AC" => 6.0,
        _ => STRAP_SPACING_FT,
    }
}

/// The bushing a raceway end takes for large conductors: bang-on on EMT/ENT (and flexible),
/// screw-on on threaded RMC/IMC/PVC.
fn bushing(mat: &str) -> &'static str {
    if matches!(mat, "RMC" | "IMC" | "PVC") { "Insulating bushing (screw-on)" } else { "Insulating bushing (bang-on)" }
}

/// One raceway: what it is, how long, where.
struct RacewayRun<'a> {
    mat: &'a str,
    size: String,
    count: f64,
    len: f64,
    turns: f64,
    level: &'a str,
    elements: Vec<String>,
    status: &'static str,
    place: (String, String, String),
    /// The raceway line's rule (where its material came from), if any.
    rule: Option<String>,
}

/// A raceway's lines: itself and its couplings, end fittings, straps and elbows, by the rules.
fn raceway_lines(lines: &mut Vec<Line>, rules: &Rules, r: &RacewayRun) {
    let m = r.mat.to_ascii_lowercase();
    let (room, phase, area) = r.place.clone();
    let line = |category: &str, item: String, unit: &'static str, net: f64, status: &'static str, rule: Option<String>| Line {
        trade: "electrical",
        category: category.into(),
        item,
        size: r.size.clone(),
        unit,
        net,
        status,
        rule,
        level: r.level.to_string(),
        elements: r.elements.clone(),
        room: room.clone(),
        phase: phase.clone(),
        area: area.clone(),
        ..Line::default()
    };
    // A cable assembly is on the BOM as its cable (with the wire); a raceway as itself.
    if !cable_assembly(r.mat) {
        add(lines, line("conduit", format!("{} conduit", r.mat), "ft", r.count * r.len, r.status, r.rule.clone()));
    }
    let (stick, stick_key) = rules.num(&[format!("bom.stick_ft.{m}"), "bom.stick_ft".into()], default_stick(r.mat), 1.0..=100.0);
    let (spacing, strap_key) = rules.num(&[format!("bom.strap_spacing_ft.{m}"), "bom.strap_spacing_ft".into()], default_strap(r.mat), 0.5..=100.0);
    let couplings = if stick > 0.0 { r.count * ((r.len / stick).ceil() - 1.0).max(0.0) } else { 0.0 };
    let straps = r.count * ((r.len / spacing).ceil() + 1.0);
    let rigid = matches!(r.mat, "RMC" | "IMC");
    let (fitting, per_end) = if rigid { ("Locknut".to_string(), 2.0) } else { (format!("{} connector", r.mat), 1.0) };
    let parts: [(String, f64, String); 4] = [
        (format!("{} coupling", r.mat), couplings, format!("one per joint between {stick}' sticks ({stick_key})")),
        (fitting, r.count * 2.0 * per_end, if rigid { "two locknuts at each raceway end".into() } else { "one at each raceway end".into() }),
        (
            if cable_assembly(r.mat) { "Cable strap".into() } else { "Conduit strap".into() },
            straps,
            format!("within 3' of each end and every {spacing}' ({strap_key})"),
        ),
        (format!("{} 90° elbow", r.mat), if flexible(r.mat) { 0.0 } else { r.count * r.turns }, "one at each turn of the routed run".into()),
    ];
    for (item, n, rule) in parts {
        add(lines, line("conduit fittings", item, "each", n, "rule", Some(rule)));
    }
}

/// Whether a homerun or feeder is a service's or bonded: its own field, or it comes from the
/// service gear (the grounding's service, a service entrance's equipment).
fn service_or_bonded(t: &ElementTag, sp: &BuildingSpec) -> bool {
    let field = |k: &str| t.fields.get(k).map(|f| f.value.trim().to_string());
    if field("service").or_else(|| field("bonded")).is_some_and(|v| truthy(&v)) {
        return true;
    }
    let from = field("from").or_else(|| field("panel")).unwrap_or_default();
    !from.is_empty()
        && (sp.mep.grounding.as_ref().is_some_and(|g| g.service.eq_ignore_ascii_case(&from))
            || sp.mep.services.iter().any(|s| s.kind.eq_ignore_ascii_case("power") && s.to.eq_ignore_ascii_case(&from)))
}

// ---- Electrical ------------------------------------------------------------------------------------

fn electrical(s: &Session, lines: &mut Vec<Line>) -> Result<()> {
    let (sp, _) = current(s, "bim.bom")?;
    let d = s.doc()?;
    let rules = Rules(&d.project_rules);
    let upf = if sp.units_per_foot.is_finite() && sp.units_per_foot > 0.0 { sp.units_per_foot } else { 12.0 };
    let subs = approved(&sp);
    // A luminaire type's schedule description, from whichever tag carries it.
    let mut described: BTreeMap<String, String> = BTreeMap::new();
    for t in sp.tags.iter().filter(|t| t.kind == "luminaire") {
        if let (Some(ty), Some(desc)) = (t.fields.get("type"), t.fields.get("luminaire")) {
            described.entry(ty.value.clone()).or_insert_with(|| desc.value.clone());
        }
    }
    let boxed: Vec<String> = rules
        .text("bom.boxed_devices")
        .map(|v| v.split(',').map(|x| x.trim().to_ascii_lowercase()).filter(|x| !x.is_empty()).collect())
        .unwrap_or_else(|| ["receptacle", "switch", "data_outlet"].map(String::from).to_vec());
    for t in sp.tags.iter().take(MAX_LINES) {
        let f = |k: &str| t.fields.get(k).map(|v| (v.value.clone(), v.source.clone()));
        let (room, phase, area) = place(t);
        match t.kind.as_str() {
            "luminaire" => {
                let (ty, src) = f("type").unwrap_or_else(|| ("UNTAGGED".into(), "default".into()));
                let desc = described.get(&ty).cloned().unwrap_or_default();
                let (mark, attrs) = identity(t, &["mark", "type"], &subs);
                add(
                    lines,
                    Line {
                        trade: "electrical",
                        category: "fixtures".into(),
                        item: format!("Luminaire type {ty}"),
                        size: desc,
                        unit: "each",
                        net: 1.0,
                        status: status_of(&t.status, &src),
                        level: t.level.clone(),
                        elements: vec![t.id.clone()],
                        mark,
                        attrs,
                        room,
                        phase,
                        area,
                        ..Line::default()
                    },
                );
            }
            "homerun" | "feeder" | "conduit" => {}
            kind => {
                let (ty, src) = f("device_type").unwrap_or_else(|| (kind.replace('_', " "), "plan".into()));
                let (mark, attrs) = identity(t, &["mark", "type_mark"], &subs);
                add(
                    lines,
                    Line {
                        trade: "electrical",
                        category: "devices".into(),
                        item: ty,
                        unit: "each",
                        net: 1.0,
                        status: status_of(&t.status, &src),
                        level: t.level.clone(),
                        elements: vec![t.id.clone()],
                        mark,
                        attrs,
                        room: room.clone(),
                        phase: phase.clone(),
                        area: area.clone(),
                        ..Line::default()
                    },
                );
                if boxed.iter().any(|b| b == kind) {
                    // The ring by the wall's assembly where the tag names it.
                    let wall = f("wall").or_else(|| f("wall_type")).map(|w| w.0.trim().to_string()).filter(|w| !w.is_empty());
                    let mut ring_keys: Vec<String> = wall.iter().map(|w| format!("bom.ring.{w}")).collect();
                    ring_keys.extend([format!("bom.ring.{kind}"), "bom.ring".into()]);
                    for (keys, default) in [
                        (vec![format!("bom.box.{kind}"), "bom.box".into()], ("Box, 4\" square", "2-1/8\" deep")),
                        (ring_keys, ("Plaster ring", "1 gang")),
                        (vec![format!("bom.plate.{kind}"), "bom.plate".into()], ("Device plate", "1 gang")),
                    ] {
                        let (item, size, key) = rules.part(&keys, default);
                        add(
                            lines,
                            Line {
                                trade: "electrical",
                                category: "boxes".into(),
                                item,
                                size,
                                unit: "each",
                                net: 1.0,
                                status: "rule",
                                rule: Some(format!("one per wall device ({key})")),
                                level: t.level.clone(),
                                elements: vec![t.id.clone()],
                                room: room.clone(),
                                phase: phase.clone(),
                                area: area.clone(),
                                ..Line::default()
                            },
                        );
                    }
                }
            }
        }
    }
    // Branch conduit and its implied fittings; each homerun's and feeder's wire and raceway.
    let kind_of = |sys: &str| sp.mep.systems.iter().find(|x| x.name == sys).map(|x| x.kind);
    let (makeup, makeup_key) = rules.num(&["bom.makeup_ft".into()], MAKEUP_FT, 0.0..=50.0);
    let bushing_min = rules.text("bom.bushing_min_awg").and_then(|v| awg_order(&v)).unwrap_or(4);
    for (i, r) in sp.mep.runs.iter().enumerate().take(MAX_LINES) {
        let len = run_length_ft(&r.points, upf);
        if !(len.is_finite() && len > 0.0) {
            continue;
        }
        let kind = kind_of(&r.system);
        if !matches!(kind, Some(cadaddy_walk3d::MepKind::Conduit | cadaddy_walk3d::MepKind::Cable)) {
            continue;
        }
        let id = if r.id.is_empty() { format!("run:{i}") } else { r.id.clone() };
        let turns = r.points.len().saturating_sub(2) as f64;
        // The homerun or feeder this route is for: its tag starts where the route does.
        let origin = r.points.first().map(|p| [p[0] / upf, p[1] / upf]).unwrap_or([f64::NAN, f64::NAN]);
        // (Routes are on the cable systems; a conduit system's runs are branch conduit.)
        let tag = sp.tags.iter().filter(|_| kind == Some(cadaddy_walk3d::MepKind::Cable)).find(|t| {
            matches!(t.kind.as_str(), "homerun" | "feeder")
                && t.level.eq_ignore_ascii_case(&r.level)
                && (t.at_ft[0] - origin[0]).hypot(t.at_ft[1] - origin[1]) < 1.0
        });
        if kind == Some(cadaddy_walk3d::MepKind::Conduit) {
            // Branch conduit: the material the rules give its system, else the job's.
            let (mat, key) = rules
                .text(&format!("bom.material.{}", r.system))
                .map(|m| (m, format!("bom.material.{}", r.system)))
                .or_else(|| rules.text("bom.conduit_material").map(|m| (m, "bom.conduit_material".to_string())))
                .unwrap_or_else(|| ("EMT".into(), "bom.conduit_material, default".into()));
            let mat = material(&mat).unwrap_or_else(|| mat.to_ascii_uppercase());
            raceway_lines(
                lines,
                &rules,
                &RacewayRun {
                    mat: &mat,
                    size: format!("{}\"", trade_size(r.size_in[0])),
                    count: 1.0,
                    len,
                    turns,
                    level: &r.level,
                    elements: vec![id.clone()],
                    status: status_of(&r.status, "plan"),
                    place: Default::default(),
                    // Where the material came from, when the rules set it.
                    rule: (!key.ends_with("default")).then(|| format!("material from {key}")),
                },
            );
            continue;
        }
        let (note, src) = tag
            .and_then(|t| t.fields.get("conductors"))
            .map_or_else(|| ("2#12 + 1#12G".to_string(), "rule".to_string()), |f| (f.value.clone(), f.source.clone()));
        let own = tag.map_or(r.status.as_str(), |t| t.status.as_str());
        let wire_status = status_of(own, &src);
        let place = tag.map(place).unwrap_or_default();
        let el = tag.map_or_else(|| vec![id.clone()], |t| vec![t.id.clone()]);
        // The raceway: the note's, else the rules' (`bom.homerun_raceway`, as `3/4" EMT`).
        let raceway = parse_raceway(&note).map(|r| (r, "the conductor note".to_string())).or_else(|| {
            rules.text("bom.homerun_raceway").and_then(|v| parse_raceway(&format!("x in {v}"))).map(|r| (r, "bom.homerun_raceway".to_string()))
        });
        let each = len + 2.0 * makeup;
        let count = f64::from(raceway.as_ref().map_or(1, |r| r.0.count));
        let parts = conductor_parts(&note);
        if let Some((rw, _)) = raceway.as_ref().filter(|(rw, _)| cable_assembly(&rw.material)) {
            // A cable assembly: the cable itself (its conductors inside).
            let conductors = note.split(" in ").next().unwrap_or_default().trim().to_string();
            add(
                lines,
                Line {
                    trade: "electrical",
                    category: "wire".into(),
                    item: format!("{} cable", rw.material),
                    size: conductors,
                    unit: "ft",
                    net: count * each,
                    status: wire_status,
                    rule: Some(format!("{note} over the route plus {makeup}' makeup at each end ({makeup_key})")),
                    level: r.level.clone(),
                    elements: el.clone(),
                    room: place.0.clone(),
                    phase: place.1.clone(),
                    area: place.2.clone(),
                    ..Line::default()
                },
            );
        } else {
            for (n, size, ground) in &parts {
                let item = if *ground { "Copper ground, green".to_string() } else { "THHN copper".to_string() };
                add(
                    lines,
                    Line {
                        trade: "electrical",
                        category: "wire".into(),
                        item,
                        size: format!("#{size}"),
                        unit: "ft",
                        net: count * f64::from(*n) * each,
                        status: wire_status,
                        rule: Some(format!("{note} over the route plus {makeup}' makeup at each end ({makeup_key})")),
                        level: r.level.clone(),
                        elements: el.clone(),
                        room: place.0.clone(),
                        phase: place.1.clone(),
                        area: place.2.clone(),
                        ..Line::default()
                    },
                );
            }
        }
        let Some((rw, from)) = raceway else { continue };
        let size = if rw.size_in > 0.0 { format!("{}\"", trade_size(rw.size_in)) } else { String::new() };
        raceway_lines(
            lines,
            &rules,
            &RacewayRun {
                mat: &rw.material,
                size: size.clone(),
                count,
                len,
                turns,
                level: &r.level,
                elements: el.clone(),
                status: if from == "the conductor note" { wire_status } else { "rule" },
                place: place.clone(),
                rule: (from != "the conductor note").then(|| format!("raceway from {from}")),
            },
        );
        // A bushing at each raceway end where the conductors are large: per raceway, not per
        // conductor; grounding bushings on a service or bonded feeder.
        if parts.iter().any(|(_, sz, g)| !g && large_at(sz, bushing_min)) && !cable_assembly(&rw.material) {
            let service = tag.is_some_and(|t| service_or_bonded(t, &sp));
            let (item, why) = if service {
                ("Grounding bushing (insulated)", "a service or bonded raceway: a grounding bushing at each end (NEC 250.92(B), 250.97)".to_string())
            } else {
                (
                    bushing(&rw.material),
                    format!(
                        "conductors #{} AWG and up: an insulating bushing at each raceway end (NEC 300.4(G); bom.bushing_min_awg)",
                        if bushing_min <= 0 { format!("{}/0", 1 - bushing_min) } else { bushing_min.to_string() }
                    ),
                )
            };
            add(
                lines,
                Line {
                    trade: "electrical",
                    category: "conduit fittings".into(),
                    item: item.into(),
                    size,
                    unit: "each",
                    net: 2.0 * count,
                    status: "rule",
                    rule: Some(why),
                    level: r.level.clone(),
                    elements: el.clone(),
                    room: place.0.clone(),
                    phase: place.1.clone(),
                    area: place.2.clone(),
                    ..Line::default()
                },
            );
        }
    }
    labels_and_grounding(&sp, d, &rules, lines);
    Ok(())
}

/// The labels (`labels.bom`) and grounding (`grounding.takeoff`) as lines: one arc flash label
/// per labeled piece of gear (made from the study, or waiting for it), the tape and label stock,
/// directory cards; the grounding conductors, clamps, rods and connections.
fn labels_and_grounding(sp: &BuildingSpec, d: &cadaddy_doc::Drawing, rules: &Rules, lines: &mut Vec<Line>) {
    let level_of = |tag: &str| sp.mep.equipment.iter().find(|e| e.tag.trim().eq_ignore_ascii_case(tag)).map(|e| e.level.clone()).unwrap_or_default();
    let (labels, _, problems) = super::labels::make(sp, d);
    let mut gear: Vec<(String, bool)> =
        labels.iter().filter(|l| l.set == "arc_flash").filter_map(|l| l.id.strip_prefix("af:").map(|t| (t.to_string(), true))).collect();
    gear.extend(problems.iter().filter(|p| p.error).filter_map(|p| p.label.strip_prefix("af:").map(|t| (t.to_string(), false))));
    gear.sort();
    gear.dedup_by(|a, b| a.0 == b.0);
    for (tag, studied) in gear.iter().take(MAX_LINES) {
        add(
            lines,
            Line {
                trade: "electrical",
                category: "labels".into(),
                item: format!("Arc flash label, 4\" x 6\", {tag}"),
                unit: "each",
                net: 1.0,
                status: "rule",
                rule: Some(if *studied {
                    "one per labeled piece of gear, from the study's ratings (NEC 110.16; labels.bom)".into()
                } else {
                    "one per labeled piece of gear, waiting for the study's ratings (NEC 110.16; labels.bom)".into()
                }),
                level: level_of(tag),
                elements: vec![format!("gear:{tag}")],
                ..Line::default()
            },
        );
    }
    let unit = |u: &str| if matches!(u, "ft" | "lf") { "ft" } else { "each" };
    for v in super::labels::bom(sp, d) {
        let item = v["item"].as_str().unwrap_or_default();
        // The arc flash labels are above, one per gear; the printer is a required tool.
        if item.starts_with("Arc flash label") || item.contains("(tool)") {
            continue;
        }
        add(
            lines,
            Line {
                trade: "electrical",
                category: "labels".into(),
                item: item.to_string(),
                unit: unit(v["unit"].as_str().unwrap_or_default()),
                net: v["qty"].as_f64().unwrap_or(0.0),
                status: "rule",
                rule: Some(format!("labels.bom: {}", v["note"].as_str().unwrap_or_default()).trim_end_matches([':', ' ']).to_string()),
                ..Line::default()
            },
        );
    }
    let _ = rules;
    for v in super::grounding::takeoff(sp, d) {
        add(
            lines,
            Line {
                trade: "electrical",
                category: "grounding".into(),
                item: v["item"].as_str().unwrap_or_default().to_string(),
                unit: unit(v["unit"].as_str().unwrap_or_default()),
                net: v["qty"].as_f64().unwrap_or(0.0),
                status: "fixed",
                rule: Some(format!("grounding.takeoff: {}", v["note"].as_str().unwrap_or_default()).trim_end_matches([':', ' ']).to_string()),
                ..Line::default()
            },
        );
    }
}

/// A trade size as printed (`3/4`, `1-1/4`).
fn trade_size(v: f64) -> String {
    let e = (v * 8.0).round() as i64;
    let (w, r) = (e / 8, e % 8);
    let frac = match r {
        0 => String::new(),
        x if x % 4 == 0 => format!("{}/2", x / 4),
        x if x % 2 == 0 => format!("{}/4", x / 2),
        x => format!("{x}/8"),
    };
    match (w, frac.is_empty()) {
        (0, false) => frac,
        (w, true) => w.to_string(),
        (w, false) => format!("{w}-{frac}"),
    }
}

// ---- Mechanical and plumbing ------------------------------------------------------------------------

/// Plumbing equipment by its kind: the category it is bought in.
fn plumbing_category(kind: &str) -> Option<&'static str> {
    let k = kind.to_ascii_lowercase();
    Some(match k.as_str() {
        "water_closet" | "lavatory" | "urinal" | "shower" | "bathtub" | "mop_sink" | "floor_drain" | "sink" | "drinking_fountain" | "hose_bibb" => {
            "plumbing fixtures"
        }
        "water_heater" => "water heaters",
        "cleanout" => "cleanouts",
        k if k.contains("valve") || k == "backflow_preventer" => "valves",
        _ => return None,
    })
}

/// A schedule row's model: the first of its model/catalog columns.
fn model_of(e: &cadaddy_walk3d::MepEquipment) -> Option<String> {
    e.properties.iter().find(|(k, v)| attr_of(k) == Some("model") && !v.trim().is_empty()).map(|(_, v)| v.trim().to_string())
}

/// `water_closet` → `Water closet`.
fn words(kind: &str) -> String {
    let w = kind.replace('_', " ");
    let mut c = w.chars();
    c.next().map(|f| f.to_uppercase().collect::<String>() + c.as_str()).unwrap_or_default()
}

/// Mechanical and plumbing from `mep.qty` (conduit is counted above, from the runs).
fn mechanical(s: &mut Session, lines: &mut Vec<Line>) {
    let Ok((sp, _)) = current(s, "bim.bom") else { return };
    let rules_owned = s.doc().map(|d| d.project_rules.clone()).unwrap_or_default();
    let rules = Rules(&rules_owned);
    // Raceway and cable systems are taken off above, with their own fittings and supports.
    let raceway: Vec<String> = sp
        .mep
        .systems
        .iter()
        .filter(|x| matches!(x.kind, cadaddy_walk3d::MepKind::Conduit | cadaddy_walk3d::MepKind::Cable))
        .map(|x| x.name.clone())
        .collect();
    let Ok(q) = s.execute("mep.qty", &json!({ "group_by": "level_system" })) else { return };
    let ours = |v: &Value| v["system"].as_str().is_some_and(|x| raceway.iter().any(|r| r == x));
    let arr = |k: &str| q[k].as_array().cloned().unwrap_or_default();
    let st = |v: &Value, k: &str| v[k].as_str().unwrap_or_default().to_string();
    let num = |v: &Value, k: &str| v[k].as_f64().unwrap_or(0.0);
    let duct_type = |sys: &str| sp.mep.systems.iter().find(|x| x.name == sys).and_then(|x| x.duct_type);
    let (wrap_item, wrap_thk, wrap_key) = rules.part(&["bom.duct_wrap".into()], ("Duct wrap", "1-1/2\""));
    for d in arr("ducts") {
        let size = format!("{} {}", st(&d, "size"), st(&d, "shape"));
        let lv = st(&d, "level");
        let sys = st(&d, "system");
        add(
            lines,
            Line {
                trade: "mechanical",
                category: "duct".into(),
                item: format!("Duct {sys}, {} ga", d["gauge"]),
                size: size.clone(),
                unit: "ft",
                net: num(&d, "length_ft"),
                status: "fixed",
                level: lv.clone(),
                ..Line::default()
            },
        );
        add(
            lines,
            Line {
                trade: "mechanical",
                category: "duct".into(),
                item: format!("Sheet metal {} ga", d["gauge"]),
                size,
                unit: "lb",
                net: num(&d, "weight_lb"),
                status: "fixed",
                level: lv.clone(),
                ..Line::default()
            },
        );
        // Wrap on supply and outside air duct (or the systems the rules name).
        let wrapped = rules
            .text(&format!("bom.duct_wrap.{sys}"))
            .map_or_else(|| matches!(duct_type(&sys), Some(cadaddy_walk3d::DuctType::Supply | cadaddy_walk3d::DuctType::OutsideAir)), |v| truthy(&v));
        if wrapped {
            add(
                lines,
                Line {
                    trade: "mechanical",
                    category: "insulation".into(),
                    item: format!("{wrap_item}, {wrap_thk}"),
                    size: sys.clone(),
                    unit: "sq ft",
                    net: num(&d, "area_sqft"),
                    status: "rule",
                    rule: Some(format!("the duct's outside area, supply and outside air (bom.duct_wrap.<system>; {wrap_key})")),
                    level: lv,
                    ..Line::default()
                },
            );
        }
    }
    let (ins_item, ins_thk, ins_key) = rules.part(&["bom.pipe_insulation".into()], ("Pipe insulation", "1\""));
    for p in arr("pipes").into_iter().filter(|p| !matches!(p["kind"].as_str(), Some("conduit" | "cable"))) {
        let trade = if p["kind"].as_str() == Some("tray") { "electrical" } else { "plumbing" };
        let sys = st(&p, "system");
        add(
            lines,
            Line {
                trade,
                category: "pipe".into(),
                item: format!("{sys} {}", st(&p, "kind")),
                size: st(&p, "size"),
                unit: "ft",
                net: num(&p, "length_ft"),
                status: "fixed",
                level: st(&p, "level"),
                ..Line::default()
            },
        );
        // Insulation on hot and chilled water (or the systems the rules name).
        let u = sys.to_ascii_uppercase();
        let insulated = rules
            .text(&format!("bom.pipe_insulation.{sys}"))
            .map_or_else(|| p["kind"].as_str() == Some("pipe") && (u.contains("HW") || u.contains("CHW")), |v| truthy(&v));
        if insulated {
            add(
                lines,
                Line {
                    trade,
                    category: "insulation".into(),
                    item: format!("{ins_item}, {ins_thk} ({sys})"),
                    size: st(&p, "size"),
                    unit: "ft",
                    net: num(&p, "length_ft"),
                    status: "rule",
                    rule: Some(format!("hot and chilled water piping is insulated (bom.pipe_insulation.<system>; {ins_key})")),
                    level: st(&p, "level"),
                    ..Line::default()
                },
            );
        }
    }
    for f in arr("fittings").into_iter().filter(|f| !ours(f)) {
        add(
            lines,
            Line {
                trade: "mechanical",
                category: "fittings".into(),
                item: format!("{} {}", st(&f, "system"), st(&f, "type")),
                size: st(&f, "size"),
                unit: "each",
                net: num(&f, "count"),
                status: "fixed",
                level: st(&f, "level"),
                ..Line::default()
            },
        );
    }
    let (flex_ft, flex_key) = rules.num(&["bom.flex_ft".into()], 5.0, 0.0..=50.0);
    let (flex_in, flex_size_key) = rules.num(&["bom.flex_size_in".into()], 8.0, 3.0..=24.0);
    let (dampers, damper_key) = rules.num(&["bom.dampers_per_terminal".into()], 1.0, 0.0..=4.0);
    for t in arr("terminals") {
        let kind = st(&t, "kind");
        let n = num(&t, "count");
        add(
            lines,
            Line {
                trade: "mechanical",
                category: "terminals".into(),
                item: kind.clone(),
                size: st(&t, "size"),
                unit: "each",
                net: n,
                status: "fixed",
                level: st(&t, "level"),
                ..Line::default()
            },
        );
        // Each air outlet on its flex connection and balancing damper.
        if ["diffuser", "register", "grille"].iter().any(|k| kind.contains(k)) {
            // A round neck is the terminal's size; a square face takes the rules' flex size.
            let size = st(&t, "size");
            let neck = if size.contains('x') || size.is_empty() { format!("{}\"", trade_size(flex_in)) } else { size.clone() };
            for (item, qty, unit, rule) in [
                ("Flex duct, insulated", n * flex_ft, "ft", format!("{flex_ft}' per air outlet ({flex_key}; size {flex_size_key})")),
                ("Volume damper", n * dampers, "each", format!("{dampers} per air outlet ({damper_key})")),
            ] {
                add(
                    lines,
                    Line {
                        trade: "mechanical",
                        category: if item == "Volume damper" { "dampers".into() } else { "duct".into() },
                        item: item.into(),
                        size: neck.clone(),
                        unit,
                        net: qty,
                        status: "rule",
                        rule: Some(rule),
                        level: st(&t, "level"),
                        ..Line::default()
                    },
                );
            }
        }
    }
    // Lugs ordered separately from their gear (`gear.check`'s lug kits): a line per part of the
    // kit, on the gear's level, so they are bought with it.
    let equipment = arr("equipment");
    for k in arr("lug_kits") {
        let gear = st(&k, "gear");
        let level = equipment.iter().find(|e| e["tag"].as_str() == Some(gear.as_str())).map(|e| st(e, "level")).unwrap_or_default();
        let at = format!("{gear} {}", st(&k, "point"));
        let lug = st(&k, "lug");
        let bolt = st(&k, "bolt");
        for (part, qty) in [
            (lug.as_str(), "lugs"),
            (bolt.as_str(), "bolts"),
            ("nut", "nuts"),
            ("flat washer", "flat_washers"),
            ("Belleville washer", "belleville_washers"),
        ] {
            add(
                lines,
                Line {
                    trade: "electrical",
                    category: "lug kits".into(),
                    item: format!("Lug kit {at}: {part}"),
                    unit: "each",
                    net: num(&k, qty),
                    status: "rule",
                    rule: Some("lugs not supplied with the gear (gear.check)".into()),
                    level: level.clone(),
                    elements: vec![format!("gear:{gear}")],
                    ..Line::default()
                },
            );
        }
    }
    let covers = rules.yes("bom.panel_cover", true);
    for e in equipment {
        let tag = st(&e, "tag");
        let kind = st(&e, "kind");
        let own = sp.mep.equipment.iter().find(|x| x.tag == tag);
        let status = status_of(own.map_or("", |x| x.status.as_str()), "schedule");
        let lk = kind.to_ascii_lowercase();
        if let Some(category) = plumbing_category(&kind) {
            // Plumbing by model (the schedule's), else by its tag.
            let model = own.and_then(model_of).unwrap_or_else(|| tag.clone());
            add(
                lines,
                Line {
                    trade: "plumbing",
                    category: category.into(),
                    item: format!("{}, {model}", words(&kind)),
                    unit: "each",
                    net: 1.0,
                    status,
                    level: st(&e, "level"),
                    elements: vec![tag].into_iter().filter(|t| !t.is_empty()).collect(),
                    ..Line::default()
                },
            );
            continue;
        }
        let electrical = matches!(lk.as_str(), "panel" | "panelboard" | "switchboard" | "switchgear" | "transformer" | "disconnect");
        add(
            lines,
            Line {
                trade: if electrical { "electrical" } else { "mechanical" },
                category: "equipment".into(),
                item: format!("{kind} {tag}"),
                size: st(&e, "size"),
                unit: "each",
                net: 1.0,
                status,
                level: st(&e, "level"),
                elements: vec![tag.clone()].into_iter().filter(|t| !t.is_empty()).collect(),
                ..Line::default()
            },
        );
        // A panelboard's cover and trim are sold separately.
        if covers && matches!(lk.as_str(), "panel" | "panelboard") && !tag.is_empty() {
            add(
                lines,
                Line {
                    trade: "electrical",
                    category: "panel accessories".into(),
                    item: format!("Panel cover/trim {tag}"),
                    unit: "each",
                    net: 1.0,
                    status: "rule",
                    rule: Some("a cover and trim per panelboard, sold separately (bom.panel_cover)".into()),
                    level: st(&e, "level"),
                    elements: vec![tag],
                    ..Line::default()
                },
            );
        }
    }
    for h in arr("hangers").into_iter().filter(|h| !ours(h)) {
        let rule = format!("{}' spacing", h["spacing_ft"]);
        add(
            lines,
            Line {
                trade: "mechanical",
                category: "hangers".into(),
                item: format!("Hanger, {}", st(&h, "kind")),
                size: st(&h, "size"),
                unit: "each",
                net: num(&h, "count"),
                status: "rule",
                rule: Some(rule),
                level: st(&h, "level"),
                ..Line::default()
            },
        );
    }
}

// ---- Drywall and framing ----------------------------------------------------------------------------

/// Drywall by level from `drywall.takeoff` (walls) and `rcp.takeoff` (ceilings), with the
/// finishing the board area implies: tape, mud and screws by the rules, corner bead as drawn.
fn drywall(s: &mut Session, lines: &mut Vec<Line>) {
    let rules_owned = s.doc().map(|d| d.project_rules.clone()).unwrap_or_default();
    let rules = Rules(&rules_owned);
    let mut board_by_level: BTreeMap<String, f64> = BTreeMap::new();
    let board = |lines: &mut Vec<Line>, level: &str, material: &str, size: String, face: &str, area: f64| {
        add(
            lines,
            Line {
                trade: "drywall",
                category: "drywall".into(),
                item: format!("Board {material}, {face}"),
                size,
                unit: "sq ft",
                net: area,
                status: "fixed",
                level: level.to_string(),
                ..Line::default()
            },
        );
    };
    if let Ok(q) = s.execute("drywall.takeoff", &json!({ "waste": 0, "groupBy": "level" })) {
        for r in q["rows"].as_array().into_iter().flatten() {
            let level = r["group"].as_str().unwrap_or_default().to_string();
            for b in r["boards"].as_array().into_iter().flatten() {
                let area = b["areaSqFt"].as_f64().unwrap_or(0.0);
                board(lines, &level, b["material"].as_str().unwrap_or_default(), format!("{}\"", b["thicknessIn"]), "wall", area);
                *board_by_level.entry(level.clone()).or_default() += area.max(0.0);
            }
            for (item, v, unit) in [
                (format!("Studs {}", r["studs"]["material"].as_str().unwrap_or_default()), r["studs"]["lengthFt"].as_f64(), "ft"),
                ("Track".to_string(), r["track"]["lengthFt"].as_f64(), "ft"),
                ("Corner bead".to_string(), r["cornerBead"]["lengthFt"].as_f64(), "ft"),
            ] {
                add(
                    lines,
                    Line {
                        trade: "drywall",
                        category: "drywall".into(),
                        item,
                        unit,
                        net: v.unwrap_or(0.0),
                        status: "fixed",
                        level: level.clone(),
                        ..Line::default()
                    },
                );
            }
        }
    }
    if let Ok(q) = s.execute("rcp.takeoff", &json!({})) {
        for r in q["rows"].as_array().into_iter().flatten() {
            let part = r["part"].as_str().unwrap_or_default();
            if r["unit"].as_str() != Some("sf") || !(part == "ceiling" || part.starts_with("soffit")) {
                continue;
            }
            let level = r["level"].as_str().unwrap_or_default().to_string();
            let area = r["quantity"].as_f64().unwrap_or(0.0);
            board(lines, &level, r["material"].as_str().unwrap_or_default(), String::new(), "ceiling", area);
            *board_by_level.entry(level).or_default() += area.max(0.0);
        }
    }
    let (tape, tape_key) = rules.num(&["bom.drywall.tape_ft_per_sf".into()], 0.37, 0.0..=5.0);
    let (mud, mud_key) = rules.num(&["bom.drywall.mud_gal_per_sf".into()], 0.014, 0.0..=1.0);
    let (screws, screws_key) = rules.num(&["bom.drywall.screws_per_sf".into()], 1.0, 0.0..=10.0);
    for (level, area) in board_by_level {
        for (item, net, unit, rule) in [
            ("Joint tape", area * tape, "ft", format!("{tape} ft per sq ft of board ({tape_key})")),
            ("Joint compound", area * mud, "gal", format!("{mud} gal per sq ft of board ({mud_key})")),
            ("Drywall screws", (area * screws).ceil(), "each", format!("{screws} per sq ft of board ({screws_key})")),
        ] {
            add(
                lines,
                Line {
                    trade: "drywall",
                    category: "drywall".into(),
                    item: item.into(),
                    unit,
                    net,
                    status: "rule",
                    rule: Some(rule),
                    level: level.clone(),
                    ..Line::default()
                },
            );
        }
    }
}

/// Framing members from the spec's framing takeoff (Q303): by member, section and length, each
/// row's status as the layout gives it.
fn framing(s: &Session, lines: &mut Vec<Line>) {
    let Ok((sp, _)) = current(s, "bim.bom") else { return };
    for r in sp.framing.iter().take(MAX_LINES) {
        let len = if r.length_in.is_finite() && r.length_in > 0.0 {
            format!(", {}' {}\"", (r.length_in / 12.0).floor(), trade_size(r.length_in % 12.0))
        } else {
            String::new()
        };
        add(
            lines,
            Line {
                trade: "framing",
                category: "framing".into(),
                item: words(&r.member.replace(' ', "_")),
                size: format!("{}{len}", r.section),
                unit: "each",
                net: f64::from(r.count),
                status: status_of(&r.status, "rule"),
                rule: (r.status == "rule").then(|| "laid out by the wall type's spacing (Q303)".to_string()),
                level: r.level.clone(),
                ..Line::default()
            },
        );
    }
}

/// Concrete from the concrete takeoff: the elements in our scope as lines; the rest listed by
/// whose they are, at no cost to us.
fn concrete(s: &Session, lines: &mut Vec<Line>) -> Vec<Value> {
    use super::concrete as c;
    let (Ok((sp, _)), Ok(d)) = (current(s, "bim.bom"), s.doc()) else { return Vec::new() };
    let r = c::Rules::of(&sp);
    let upf = c::upf(&sp);
    let mut others: Vec<Value> = Vec::new();
    for x in c::elements(&sp, &d.bim_types) {
        let e = &x.e;
        let q = c::quantities(e, upf, &r);
        let scope = c::scope_of(&r, &e.owner);
        if scope != "ours" {
            if others.len() < MAX_LINES {
                others.push(json!({
                    "trade": "concrete", "element": e.id, "kind": e.kind, "level": e.level, "owner": e.owner, "scope": scope,
                    "cy": c::r2(q.cy_waste), "cost_to_us": 0.0,
                }));
            }
            continue;
        }
        let kind = e.kind.replace('_', " ");
        let (status, rule) = if x.derived {
            ("rule", Some("housekeeping pad: the equipment's footprint plus the concrete rules' margin each side".to_string()))
        } else {
            ("fixed", None)
        };
        let mut put = |category: &str, item: String, size: String, unit: &'static str, net: f64, waste_qty: Option<f64>| {
            add(
                lines,
                Line {
                    trade: "concrete",
                    category: category.into(),
                    item,
                    size,
                    unit,
                    net,
                    status,
                    rule: rule.clone(),
                    level: e.level.clone(),
                    elements: vec![e.id.clone()],
                    waste_qty,
                    ..Line::default()
                },
            );
        };
        put("concrete", format!("Concrete, {kind}"), format!("{}\"", c::r2(e.depth_in)), "cu yd", q.cy, Some((q.cy_waste - q.cy).max(0.0)));
        put("forms", format!("Forms, {kind}"), String::new(), "sq ft", q.forms_sf, None);
        for (bar, (_, lb)) in &q.rebar {
            put("rebar", "Rebar".into(), bar.clone(), "lb", *lb, None);
        }
        put("welded wire", "Welded wire".into(), e.mesh.clone(), "sq ft", q.mesh_sf, None);
        for ((k, size), (n, ft)) in &q.embeds {
            let mut item = k.replace('_', " ");
            if let Some(f) = item.get(..1).map(str::to_ascii_uppercase) {
                item.replace_range(..1, &f);
            }
            if *ft > 0.0 {
                put("embeds", item, size.clone(), "ft", *ft, None);
            } else {
                put("embeds", item, size.clone(), "each", *n as f64, None);
            }
        }
        put("finish", "Finish".into(), String::new(), "sq ft", q.finish_sf, None);
        put("finish", "Curing".into(), String::new(), "sq ft", q.curing_sf, None);
    }
    others
}

// ---- Required items ---------------------------------------------------------------------------------

/// What the required items are worked out from besides the lines.
struct Job {
    crew: f64,
    floors: f64,
    /// The duration, weeks (0: not given): rentals say it.
    weeks: f64,
    /// The highest run bottom above its floor, feet.
    highest_ft: f64,
    /// Site trenching is modelled (an underground service, an encased run).
    trench: bool,
    /// Aluminum conductor terminations (two per conductor).
    al_ends: f64,
    /// Each raceway's material and trade size (inches) with conduit on the BOM.
    raceways: Vec<(String, f64)>,
}

/// A raceway's outside diameter, inches (EMT and RMC/IMC tables; PVC as RMC).
fn raceway_od(mat: &str, size_in: f64) -> f64 {
    const EMT: [(f64, f64); 9] =
        [(0.5, 0.706), (0.75, 0.922), (1.0, 1.163), (1.25, 1.510), (1.5, 1.740), (2.0, 2.197), (2.5, 2.875), (3.0, 3.5), (4.0, 4.5)];
    const RMC: [(f64, f64); 9] =
        [(0.5, 0.840), (0.75, 1.050), (1.0, 1.315), (1.25, 1.660), (1.5, 1.900), (2.0, 2.375), (2.5, 2.875), (3.0, 3.5), (4.0, 4.5)];
    let t = if mat == "EMT" { &EMT } else { &RMC };
    t.iter().find(|(s, _)| (s - size_in).abs() < 1e-6).map_or(size_in + 0.25, |(_, od)| *od)
}

/// The self-feed bit for a raceway through framing: the smallest standard bit at least 1/8"
/// over its outside diameter.
fn bit_for(od: f64) -> f64 {
    const BITS: [f64; 14] = [0.875, 1.0, 1.125, 1.25, 1.375, 1.5, 1.75, 2.0, 2.125, 2.25, 2.5625, 3.0, 3.625, 4.625];
    BITS.iter().copied().find(|b| *b >= od + 0.125).unwrap_or(od + 0.25)
}

/// The job's required items from the modelled work, the crew, the floors and the duration.
fn required(lines: &[Line], job: &Job) -> Vec<(String, f64, &'static str, String)> {
    let mut out: Vec<(String, f64, &'static str, String)> = Vec::new();
    let has = |cat: &str| lines.iter().any(|l| l.category == cat);
    let (crew, floors) = (job.crew.max(1.0), job.floors.max(1.0));
    let pairs = (crew / 2.0).ceil();
    let conduit_sizes: Vec<String> = {
        let mut v: Vec<String> = lines.iter().filter(|l| l.category == "conduit" && l.item.ends_with("conduit")).map(|l| l.size.clone()).collect();
        v.sort();
        v.dedup();
        v
    };
    for sz in &conduit_sizes {
        out.push((format!("Carbide hole saw for {sz} conduit"), 1.0, "purchase", format!("{sz} conduit is modelled: a hole saw sized to it")));
        out.push((format!("Conduit bender {sz}"), 1.0, "owned", format!("{sz} EMT bends are modelled")));
    }
    if !conduit_sizes.is_empty() {
        out.push(("Knockout set".into(), 1.0, "owned", "conduit enters boxes and panels".into()));
        // Holes through framing: a hole hog per two people, a bit per raceway size each.
        out.push(("Right-angle drill (hole hog)".into(), pairs, "owned", format!("one per two of the crew of {crew}")));
        let mut bits: Vec<(f64, String)> = Vec::new();
        for (mat, size) in &job.raceways {
            let bit = bit_for(raceway_od(mat, *size));
            let name = format!("Self-feed bit {}\"", trade_size(bit));
            if !bits.iter().any(|(_, n)| *n == name) {
                bits.push((bit, name.clone()));
                out.push((
                    name,
                    pairs,
                    "purchase",
                    format!("{}\" {mat} is {:.3}\" outside: the bit 1/8\" over it, one per hole hog", trade_size(*size), raceway_od(mat, *size)),
                ));
            }
        }
    }
    let wire_ft: f64 = lines.iter().filter(|l| l.category == "wire").map(|l| l.net).sum();
    if wire_ft > 0.0 {
        out.push(("Fish tape".into(), 1.0, "owned", "wire is pulled in raceways".into()));
        if wire_ft > 2_000.0 {
            out.push(("Wire tugger".into(), 1.0, "rental", format!("{wire_ft:.0} ft of wire to pull")));
        }
        out.push(("Wire pulling lube".into(), (wire_ft / 5_000.0).ceil().max(1.0), "purchase", "one quart per 5,000 ft pulled".into()));
        out.push(("Pull rope, 600' spool".into(), floors, "purchase", "one spool per floor for the pulls".into()));
    }
    if lines.iter().any(|l| l.category == "wire" && large_conductor(l.size.trim_start_matches('#'))) {
        out.push(("Crimper with dies".into(), 1.0, "rental", "#4 AWG and larger conductors need lugs".into()));
        out.push(("Torque wrench".into(), 1.0, "owned", "lugs are torqued to the listing".into()));
    }
    if job.al_ends > 0.0 {
        out.push((
            "Oxide inhibitor, 8 oz".into(),
            (job.al_ends / 20.0).ceil(),
            "purchase",
            format!("{:.0} aluminum terminations, one tube per 20", job.al_ends),
        ));
    }
    let cable_ft: f64 = lines.iter().filter(|l| l.category == "wire" && l.item.ends_with(" cable")).map(|l| l.net).sum();
    if cable_ft > 0.0 {
        out.push((
            "Cable staples, box of 100".into(),
            (cable_ft / 4.5 / 100.0).ceil(),
            "purchase",
            format!("{cable_ft:.0} ft of cable stapled every 4.5'"),
        ));
        out.push(("Staple gun".into(), pairs, "owned", format!("one per two of the crew of {crew}")));
    }
    if has("devices") || has("fixtures") || has("labels") {
        out.push(("Label printer and tape".into(), 1.0, "purchase", "devices and panels are labelled".into()));
    }
    if !lines.is_empty() {
        out.push(("Gang box".into(), floors, "owned", "one per floor".into()));
        out.push(("Padlock".into(), floors, "purchase", "one per gang box".into()));
        out.push(("Chain, 10'".into(), floors, "purchase", "one per gang box".into()));
        out.push(("Material cart".into(), floors * (crew / 4.0).ceil(), "owned", format!("one per four of the crew of {crew}, per floor")));
        out.push(("Step ladder, 8'".into(), floors * pairs, "owned", format!("one per two of the crew of {crew}, per floor")));
        out.push(("Temporary power".into(), 1.0, "rental", "the job needs power before the service".into()));
    }
    if job.highest_ft > LADDER_REACH_FT {
        out.push((
            "Scissor lift, 19'".into(),
            (crew / 4.0).ceil(),
            "rental",
            format!("work at {:.1}' is above a ladder's {LADDER_REACH_FT}': one per four of the crew of {crew}", job.highest_ft),
        ));
    }
    if job.trench {
        out.push(("Trencher".into(), 1.0, "rental", "site trenching is modelled (an underground service or encased run)".into()));
    }
    out
}

/// The electrical and mechanical lines per floor, before grouping and waste, each with every
/// element it counts (`bim.bom` lists at most 500): what progress marks resolve against (Q332).
pub(super) fn model_lines(s: &mut Session) -> Result<Vec<Value>> {
    let mut lines: Vec<Line> = Vec::new();
    electrical(s, &mut lines)?;
    mechanical(s, &mut lines);
    Ok(lines
        .into_iter()
        .map(|l| {
            json!({"trade": l.trade, "category": l.category, "item": l.item, "size": l.size, "unit": l.unit, "net": l.net, "level": l.level, "elements": l.elements})
        })
        .collect())
}

/// A number param, else a number project rule, in range.
fn number(p: &Value, key: &str, rules: &ProjectRules, rule: &str) -> Option<f64> {
    p[key]
        .as_f64()
        .or_else(|| rules.resolve(RuleDomain::Custom, rule).and_then(|v| v.value.trim().parse::<f64>().ok()))
        .filter(|v| v.is_finite() && (0.0..=10_000.0).contains(v))
}

fn run(s: &mut Session, p: &Value) -> Result<Value> {
    let group = str_param(p, "group_by").unwrap_or("job").to_ascii_lowercase();
    let all_elements = p.get("all_elements").and_then(Value::as_bool).unwrap_or(false);
    let dims: Vec<&str> = match group.as_str() {
        "job" | "trade" => Vec::new(),
        "level" => vec!["level"],
        "room" => vec!["room"],
        "phase" => vec!["phase"],
        "area" | "unit_type" => vec!["area"],
        _ => return Err(bad("bim.bom", "`group_by` is job, level, trade, room, phase or area")),
    };
    let mut lines: Vec<Line> = Vec::new();
    electrical(s, &mut lines)?;
    mechanical(s, &mut lines);
    drywall(s, &mut lines);
    framing(s, &mut lines);
    let by_others = concrete(s, &mut lines);
    // Waivers: recorded in the project rules, with the reason.
    if let Some(ws) = p.get("waive").and_then(Value::as_array) {
        for w in ws.iter().take(1_000) {
            let (Some(item), Some(reason)) = (w["item"].as_str(), w["reason"].as_str().map(str::trim).filter(|r| !r.is_empty())) else {
                return Err(bad("bim.bom", "each waiver needs an `item` and a `reason` (owned / not needed / by others)"));
            };
            s.doc_mut()?.project_rules.set(RuleDomain::Custom, &format!("bom.waive.{item}"), RuleValue::new(reason), reason);
        }
    }
    let (sp, _) = current(s, "bim.bom")?;
    let d = s.doc()?;
    let rules = &d.project_rules;
    let waived = |item: &str| rules.resolve(RuleDomain::Custom, &format!("bom.waive.{item}")).map(|v| v.value.clone());
    // A percent the job sets (the param, else the project rules).
    let waste_set = |cat: &str| -> Option<f64> {
        p["waste"][cat]
            .as_f64()
            .or_else(|| {
                rules.resolve(RuleDomain::Custom, &format!("waste.{cat}")).and_then(|v| v.value.trim().trim_end_matches('%').parse::<f64>().ok())
            })
            .filter(|v| v.is_finite() && (0.0..=200.0).contains(v))
    };
    let upf = if sp.units_per_foot.is_finite() && sp.units_per_foot > 0.0 { sp.units_per_foot } else { 12.0 };
    // The job's facts for its required items.
    let job = Job {
        crew: number(p, "crew", rules, "bom.crew").unwrap_or(CREW).clamp(1.0, 500.0),
        floors: number(p, "floors", rules, "bom.floors").unwrap_or(sp.levels.len().max(1) as f64).clamp(1.0, 500.0),
        weeks: number(p, "weeks", rules, "bom.weeks").unwrap_or(0.0),
        highest_ft: sp.mep.runs.iter().flat_map(|r| r.bottoms_ft.iter().copied()).filter(|b| b.is_finite()).fold(0.0, f64::max),
        trench: sp.mep.runs.iter().any(|r| r.encased) || sp.mep.services.iter().any(|x| x.from.is_some() && x.at.is_some()),
        al_ends: sp
            .tags
            .iter()
            .filter(|t| matches!(t.kind.as_str(), "homerun" | "feeder"))
            .filter_map(|t| t.fields.get("conductors"))
            .filter(|f| f.value.to_ascii_uppercase().split(|c: char| !c.is_ascii_alphanumeric()).any(|w| w == "AL"))
            .map(|f| conductor_parts(&f.value).iter().map(|(n, _, _)| f64::from(*n) * 2.0).sum::<f64>())
            .sum::<f64>()
            + sp.mep
                .feeders
                .iter()
                .filter(|f| f.material.eq_ignore_ascii_case("al"))
                .map(|f| f64::from(f.sets * (f.phases + f.neutrals) * 2))
                .sum::<f64>(),
        raceways: {
            let mut v: Vec<(String, f64)> = lines
                .iter()
                .filter(|l| l.category == "conduit" && l.item.ends_with(" conduit"))
                .filter_map(|l| Some((l.item.trim_end_matches(" conduit").to_string(), parse_trade_size(l.size.trim_end_matches('"'))?)))
                .collect();
            v.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.total_cmp(&b.1)));
            v.dedup();
            v
        },
    };
    let _ = upf;
    // Group.
    let mut grouped: Vec<Line> = Vec::new();
    let mut index: BTreeMap<String, usize> = BTreeMap::new();
    for l in lines {
        let l = Line {
            level: if dims.contains(&"level") { l.level } else { String::new() },
            room: if dims.contains(&"room") { l.room } else { String::new() },
            phase: if dims.contains(&"phase") { l.phase } else { String::new() },
            area: if dims.contains(&"area") { l.area } else { String::new() },
            ..l
        };
        let k = format!("{}|{}|{:?}", l.key(&dims), l.status, l.rule);
        match index.get(&k).and_then(|i| grouped.get_mut(*i)) {
            Some(x) => merge(x, l),
            None => {
                index.insert(k, grouped.len());
                grouped.push(l);
            }
        }
    }
    grouped.sort_by(|a, b| {
        a.trade
            .cmp(b.trade)
            .then_with(|| a.category.cmp(&b.category))
            .then_with(|| a.item.cmp(&b.item))
            .then_with(|| a.size.cmp(&b.size))
            .then_with(|| a.level.cmp(&b.level))
            .then_with(|| a.room.cmp(&b.room))
            .then_with(|| a.phase.cmp(&b.phase))
            .then_with(|| a.area.cmp(&b.area))
    });
    let round = |v: f64| (v * 100.0).round() / 100.0;
    let out_lines: Vec<Value> = grouped
        .iter()
        .map(|l| {
            let set = waste_set(&l.category);
            let (pct, waste) = match (set, l.waste_qty) {
                // The takeoff's own waste, unless the job sets the category's.
                (None, Some(w)) if l.net > 0.0 => (round(w / l.net * 100.0), w),
                _ => {
                    let pct = set.unwrap_or_else(|| default_waste(&l.category));
                    // Whole units of counted items (to the nearest one), lengths and areas as they are.
                    (pct, if l.unit == "each" { (l.net * pct / 100.0).round() } else { l.net * pct / 100.0 })
                }
            };
            let mut v = json!({
                "trade": l.trade, "category": l.category, "item": l.item, "size": l.size, "unit": l.unit,
                "net": round(l.net), "waste_pct": pct, "waste": round(waste), "total": round(l.net + waste),
                "status": l.status, "elements": l.elements.iter().take(if all_elements { usize::MAX } else { 500 }).collect::<Vec<_>>(),
            });
            if let Some(r) = &l.rule {
                v["rule"] = json!(r);
            }
            if !l.mark.is_empty() {
                v["mark"] = json!(l.mark);
            }
            if !l.attrs.is_empty() {
                v["attributes"] = json!(l.attrs.iter().map(|(k, (val, _))| (k.clone(), json!(val))).collect::<serde_json::Map<_, _>>());
                v["attribute_sources"] = json!(l.attrs.iter().map(|(k, (_, src))| (k.clone(), json!(src))).collect::<serde_json::Map<_, _>>());
            }
            for (dim, val) in [("level", &l.level), ("room", &l.room), ("phase", &l.phase), ("area", &l.area)] {
                if dims.contains(&dim) {
                    v[dim] = json!(val);
                }
            }
            v
        })
        .collect();
    // The totals by trade, category and unit: the summary.
    let mut sums: BTreeMap<(String, String, String), f64> = BTreeMap::new();
    for l in &out_lines {
        let k = (
            l["trade"].as_str().unwrap_or_default().to_string(),
            l["category"].as_str().unwrap_or_default().to_string(),
            l["unit"].as_str().unwrap_or_default().to_string(),
        );
        *sums.entry(k).or_default() += l["total"].as_f64().unwrap_or(0.0);
    }
    let summary: Vec<Value> = sums.iter().map(|((t, c, u), v)| json!({"trade": t, "category": c, "unit": u, "total": round(*v)})).collect();
    // Required job items and their waivers.
    let req: Vec<Value> = required(&grouped, &job)
        .into_iter()
        .map(|(item, qty, supply, rule)| {
            let w = waived(&item);
            let mut v = json!({ "item": item, "qty": if w.is_some() { 0.0 } else { qty }, "supply": supply, "rule": rule });
            if supply == "rental" && job.weeks > 0.0 {
                v["weeks"] = json!(job.weeks);
            }
            if let Some(w) = w {
                v["waived"] = json!(w);
            }
            v
        })
        .collect();
    // Spec check: a requirement is carried only by a line with the type's mark and attribute.
    let mut check: Vec<Value> = Vec::new();
    for t in d.bim_types.types.iter().take(MAX_LINES) {
        let wanted: Vec<(&str, &cadaddy_doc::bimspec::SpecField)> =
            ["finish", "color", "colour", "options"].iter().filter_map(|k| t.fields.get(*k).map(|f| (attr_of(k).unwrap_or(k), f))).collect();
        if wanted.is_empty() || t.mark.trim().is_empty() {
            continue;
        }
        let with_mark: Vec<&Line> = grouped.iter().filter(|l| l.mark.eq_ignore_ascii_case(t.mark.trim())).collect();
        for (attr, f) in wanted {
            let spec = f.value.trim();
            let carried = with_mark.iter().any(|l| l.attrs.get(attr).is_some_and(|(v, _)| v.eq_ignore_ascii_case(spec)));
            let contra: Vec<&&Line> = with_mark.iter().filter(|l| l.attrs.get(attr).is_some_and(|(v, _)| !v.eq_ignore_ascii_case(spec))).collect();
            for l in &contra {
                let (val, src) = l.attrs.get(attr).cloned().unwrap_or_default();
                check.push(json!({
                    "issue": "attribute_contradicts_spec", "item": t.mark, "field": attr, "spec": spec, "line": val, "line_source": src, "source": f.source,
                    "detail": format!("{} ({}) says {attr} {val}; the documents say {spec} ({})", l.item, l.size, f.source),
                }));
            }
            if !carried && contra.is_empty() {
                check.push(json!({
                    "issue": "requirement_without_line", "item": t.mark, "field": attr, "spec": spec, "source": f.source,
                    "detail": format!("type {} needs {attr} {spec} ({}): no line with that mark carries it", t.mark, f.source),
                }));
            }
        }
    }
    let devices: f64 = grouped.iter().filter(|l| l.category == "devices" && !l.item.contains("floor")).map(|l| l.net).sum();
    let plates: f64 = grouped.iter().filter(|l| l.category == "boxes" && l.item.contains("plate")).map(|l| l.net).sum();
    if devices > 0.0 && plates <= 0.0 {
        check.push(json!({ "issue": "accessory_missing", "item": "Device plate", "detail": "devices are on the BOM without their plates (sold separately)" }));
    }
    for e in grouped.iter().filter(|l| l.category == "equipment" && l.trade == "electrical" && l.item.to_ascii_lowercase().starts_with("panel")) {
        for tag in e.elements.iter().take(MAX_LINES) {
            let acc = format!("Panel cover/trim {tag}");
            if !grouped.iter().any(|l| l.item == acc) {
                check.push(
                    json!({ "issue": "accessory_missing", "item": acc, "detail": format!("{} has no cover/trim line (sold separately)", e.item) }),
                );
            }
        }
    }
    // Checks can be waived like required items, with a reason.
    for c in &mut check {
        if let Some(w) = c["item"].as_str().and_then(waived) {
            c["waived"] = json!(w);
        }
    }
    let req_ok = req.iter().all(|r| r["qty"].as_f64().unwrap_or(0.0) > 0.0 || r["waived"].is_string());
    let check_ok = check.iter().all(|c| c["waived"].is_string());
    let mut out = json!({ "lines": out_lines, "summary": summary, "by_others": by_others, "required": req, "spec_check": check, "complete": req_ok && check_ok });
    // What changed since an earlier BOM.
    if let Some(prev) = p.get("compare_to").and_then(|v| v["lines"].as_array().or_else(|| v.as_array())) {
        out["changes"] = changes(prev, out["lines"].as_array().map(Vec::as_slice).unwrap_or_default());
    }
    // CSV / XLSX.
    let cols = ["trade", "category", "item", "size", "unit", "net", "waste", "total", "status", "rule", "level"];
    let header: Vec<String> = cols.iter().map(|c| p["columns"][*c].as_str().map_or_else(|| (*c).to_string(), str::to_string)).collect();
    let mut rows = vec![header];
    for l in out["lines"].as_array().into_iter().flatten() {
        rows.push(
            cols.iter()
                .map(|c| match &l[*c] {
                    Value::String(x) => x.clone(),
                    Value::Null => String::new(),
                    v => v.to_string(),
                })
                .collect(),
        );
    }
    out["csv"] = json!(
        rows.iter()
            .map(|r| r
                .iter()
                .map(|c| if c.contains([',', '"', '\n']) { format!("\"{}\"", c.replace('"', "\"\"")) } else { c.clone() })
                .collect::<Vec<_>>()
                .join(","))
            .collect::<Vec<_>>()
            .join("\n")
    );
    if let Some(path) = str_param(p, "file").filter(|x| !x.trim().is_empty()) {
        let lower = path.to_ascii_lowercase();
        let bytes = if lower.ends_with(".xlsx") {
            super::table::xlsx::write(&rows, "Bill of Materials").map_err(|e| bad("bim.bom", e))?
        } else if lower.ends_with(".pdf") {
            let plot =
                super::file::io().and_then(|h| h.plot).ok_or_else(|| bad("bim.bom", "PDF isn't available in this build (write .csv or .xlsx)"))?;
            let (dwg, params) = summary_drawing(&summary, out["complete"].as_bool().unwrap_or(false));
            plot(&dwg, &cadaddy_doc::Space::Model, &params).map_err(|e| bad("bim.bom", e))?
        } else {
            out["csv"].as_str().unwrap_or_default().as_bytes().to_vec()
        };
        #[cfg(not(target_arch = "wasm32"))]
        {
            crate::writefile::write(path, &bytes).map_err(|e| crate::EngineError::Other(format!("{path}: {e}")))?;
            out["path"] = json!(path);
        }
        #[cfg(target_arch = "wasm32")]
        let _ = bytes;
    }
    Ok(out)
}

/// The PDF summary: every trade's categories with their totals, as Letter pages (a drawing of
/// text, a page window per sheet) and the plot parameters.
fn summary_drawing(summary: &[Value], complete: bool) -> (cadaddy_doc::Drawing, Value) {
    const ROWS: usize = 40;
    let mut d = cadaddy_doc::Drawing::new_imperial();
    let text = |d: &mut cadaddy_doc::Drawing, x: f64, y: f64, h: f64, v: String| {
        let t = cadaddy_doc::Text {
            insert: cadaddy_geom::Vec3::new(x, y, 0.0),
            align_pt: None,
            height: h,
            value: v,
            rotation: 0.0,
            width_factor: 1.0,
            oblique: 0.0,
            style: "Standard".into(),
            halign: Default::default(),
            valign: Default::default(),
        };
        let _ = d.add(&cadaddy_doc::Space::Model, cadaddy_doc::Common::default(), cadaddy_doc::EntityKind::Text(t));
    };
    let pages = summary.len().div_ceil(ROWS).max(1);
    let mut windows = Vec::new();
    for page in 0..pages {
        let y0 = -(page as f64) * 12.0;
        windows.push(json!({"window": [[0.0, y0], [8.5, y0 + 11.0]], "paper": "Letter", "landscape": false}));
        text(&mut d, 0.5, y0 + 10.3, 0.2, format!("BILL OF MATERIALS SUMMARY   page {} of {pages}", page + 1));
        text(&mut d, 0.5, y0 + 10.0, 0.11, if complete { "complete".into() } else { "not complete: see the required items and spec check".into() });
        for (i, s) in summary.iter().skip(page * ROWS).take(ROWS).enumerate() {
            let y = y0 + 9.6 - i as f64 * 0.22;
            let st = |k: &str| s[k].as_str().unwrap_or_default().to_string();
            text(&mut d, 0.5, y, 0.11, st("trade"));
            text(&mut d, 1.8, y, 0.11, st("category"));
            text(&mut d, 5.0, y, 0.11, format!("{}", s["total"]));
            text(&mut d, 6.5, y, 0.11, st("unit"));
        }
    }
    (d, json!({"pages": windows, "margins": [0.0, 0.0, 0.0, 0.0], "lineweights": false, "stamp": false}))
}

/// Added, removed and changed lines between two BOMs (by trade, category, item, size, level).
fn changes(prev: &[Value], now: &[Value]) -> Value {
    let key = |l: &Value| {
        format!(
            "{}|{}|{}|{}|{}",
            l["trade"].as_str().unwrap_or_default(),
            l["category"].as_str().unwrap_or_default(),
            l["item"].as_str().unwrap_or_default(),
            l["size"].as_str().unwrap_or_default(),
            l["level"].as_str().unwrap_or_default()
        )
    };
    let sum = |ls: &[Value]| {
        let mut m: BTreeMap<String, f64> = BTreeMap::new();
        for l in ls.iter().take(MAX_LINES) {
            *m.entry(key(l)).or_insert(0.0) += l["net"].as_f64().unwrap_or(0.0);
        }
        m
    };
    let (a, b) = (sum(prev), sum(now));
    let added: Vec<Value> = b.iter().filter(|(k, _)| !a.contains_key(*k)).map(|(k, v)| json!({ "line": k, "net": v })).collect();
    let removed: Vec<Value> = a.iter().filter(|(k, _)| !b.contains_key(*k)).map(|(k, v)| json!({ "line": k, "net": v })).collect();
    let changed: Vec<Value> = b
        .iter()
        .filter_map(|(k, v)| {
            a.get(k).filter(|w| (*w - v).abs() > 1e-6).map(|w| json!({ "line": k, "was": w, "now": v, "delta": ((v - w) * 100.0).round() / 100.0 }))
        })
        .collect();
    json!({ "added": added, "removed": removed, "changed": changed })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A small electrical job: two luminaires, three receptacles (one GFCI), a 25' conduit run
    /// with a turn, and a homerun of `2#12 + 1#12G` over 30'.
    fn job(s: &mut Session) {
        let tags = json!([
            {"id": "L1", "kind": "luminaire", "level": "1", "at_ft": [1, 1], "fields": {"type": {"value": "A", "source": "plan"}, "luminaire": {"value": "2X4 TROFFER", "source": "schedule"}, "room": {"value": "101", "source": "plan"}}},
            {"id": "L2", "kind": "luminaire", "level": "1", "at_ft": [2, 1], "fields": {"type": {"value": "A", "source": "plan"}, "room": {"value": "102", "source": "plan"}}},
            {"id": "R1", "kind": "receptacle", "level": "1", "at_ft": [3, 1], "fields": {"device_type": {"value": "standard duplex receptacle", "source": "default"}, "room": {"value": "101", "source": "plan"}}},
            {"id": "R2", "kind": "receptacle", "level": "1", "at_ft": [4, 1], "fields": {"device_type": {"value": "standard duplex receptacle", "source": "default"}, "room": {"value": "101", "source": "plan"}}},
            {"id": "R3", "kind": "receptacle", "level": "1", "at_ft": [5, 1], "fields": {"device_type": {"value": "GFCI receptacle", "source": "plan"}, "room": {"value": "102", "source": "plan"}}},
            {"id": "H1", "kind": "homerun", "level": "1", "at_ft": [0, 0], "fields": {"conductors": {"value": "2#12 + 1#12G", "source": "note"}}}
        ]);
        let spec = json!({
            "levels": [{"name": "1", "elevation_ft": 0, "height_ft": 12}],
            "mep": {
                "systems": [
                    {"name": "E-BRANCH-COND", "layer": "E-BRANCH-COND", "kind": "conduit", "size_in": [0.75, 0], "bottom_ft": 9},
                    {"name": "E-HOMERUN", "layer": "E-HOMERUN", "kind": "cable", "size_in": [0.5, 0], "bottom_ft": 9}
                ],
                "runs": [
                    {"system": "E-BRANCH-COND", "level": "1", "points": [[0, 0], [240, 0], [240, 60]], "size_in": [0.75, 0], "bottoms_ft": [9, 9]},
                    {"system": "E-HOMERUN", "level": "1", "points": [[0, 0], [360, 0]], "size_in": [0.5, 0], "bottoms_ft": [9]}
                ]
            },
            "tags": tags
        });
        s.execute("bspec.set", &json!({ "replace": true, "spec": spec })).unwrap();
    }

    fn line<'a>(r: &'a Value, item: &str) -> &'a Value {
        r["lines"].as_array().unwrap().iter().find(|l| l["item"] == json!(item)).unwrap_or(&Value::Null)
    }

    fn req(r: &Value, item: &str) -> f64 {
        r["required"].as_array().unwrap().iter().find(|x| x["item"] == json!(item)).map_or(0.0, |x| x["qty"].as_f64().unwrap())
    }

    fn set_rule(s: &mut Session, key: &str, value: &str) {
        s.doc_mut().unwrap().project_rules.set(RuleDomain::Custom, key, RuleValue::new(value), "test");
    }

    /// Edit the spec in place.
    fn edit(s: &mut Session, f: impl FnOnce(&mut Value)) {
        let mut spec = s.execute("bspec.get", &json!({})).unwrap()["spec"].clone();
        f(&mut spec);
        s.execute("bspec.set", &json!({ "replace": true, "spec": spec })).unwrap();
    }

    #[test]
    fn a_small_job_takes_off_with_its_implied_parts_and_waste() {
        let mut s = Session::new();
        job(&mut s);
        let r = s.execute("bim.bom", &json!({})).unwrap();
        assert_eq!(line(&r, "Luminaire type A")["net"], json!(2.0), "{r}");
        assert_eq!(line(&r, "standard duplex receptacle")["net"], json!(2.0));
        assert_eq!(line(&r, "GFCI receptacle")["net"], json!(1.0));
        assert_eq!(line(&r, "Device plate")["net"], json!(3.0));
        assert!(line(&r, "Device plate")["rule"].is_string());
        // 25' of 3/4" EMT: 3 sticks → 2 couplings, 2 connectors, 4 straps, 1 elbow; 10% waste.
        let c = line(&r, "EMT conduit");
        assert_eq!(
            (c["size"].clone(), c["net"].clone(), c["waste"].clone(), c["total"].clone()),
            (json!("3/4\""), json!(25.0), json!(2.5), json!(27.5))
        );
        assert_eq!(line(&r, "EMT coupling")["net"], json!(2.0));
        assert_eq!(line(&r, "EMT connector")["net"], json!(2.0));
        assert_eq!(line(&r, "Conduit strap")["net"], json!(4.0));
        assert_eq!(line(&r, "EMT 90° elbow")["net"], json!(1.0));
        // 2#12 + 1#12G over 30' plus 1' makeup at each end: 64' of #12 THHN, 32' of ground.
        let w = r["lines"].as_array().unwrap().iter().find(|l| l["item"] == json!("THHN copper")).unwrap();
        assert_eq!((w["size"].clone(), w["net"].clone()), (json!("#12"), json!(64.0)));
        assert_eq!(line(&r, "Copper ground, green")["net"], json!(32.0));
        // Fixed (the plan says so) vs rule.
        assert_eq!(line(&r, "GFCI receptacle")["status"], json!("fixed"));
        assert_eq!(line(&r, "EMT coupling")["status"], json!("rule"));
        // Required items follow the work.
        let req: Vec<&str> = r["required"].as_array().unwrap().iter().filter_map(|x| x["item"].as_str()).collect();
        assert!(
            req.contains(&"Carbide hole saw for 3/4\" conduit") && req.contains(&"Knockout set") && req.contains(&"Label printer and tape"),
            "{req:?}"
        );
        assert!(r["csv"].as_str().unwrap().lines().count() > 10);
    }

    #[test]
    fn waivers_are_recorded_and_changes_are_reported() {
        let mut s = Session::new();
        job(&mut s);
        let before = s.execute("bim.bom", &json!({ "group_by": "level" })).unwrap();
        // Waive one required item with a reason; a waiver with no reason is refused.
        assert!(s.execute("bim.bom", &json!({ "waive": [{ "item": "Knockout set" }] })).is_err());
        let r = s.execute("bim.bom", &json!({ "waive": [{ "item": "Knockout set", "reason": "owned" }] })).unwrap();
        let ko = r["required"].as_array().unwrap().iter().find(|x| x["item"] == json!("Knockout set")).unwrap();
        assert_eq!((ko["qty"].clone(), ko["waived"].clone()), (json!(0.0), json!("owned")));
        // A revision: a receptacle removed → a changed line.
        edit(&mut s, |spec| spec["tags"].as_array_mut().unwrap().retain(|t| t["id"] != json!("R2")));
        let after = s.execute("bim.bom", &json!({ "group_by": "level", "compare_to": { "lines": before["lines"] } })).unwrap();
        let ch = &after["changes"]["changed"];
        assert!(ch.as_array().unwrap().iter().any(|c| c["line"].as_str().unwrap().contains("standard duplex") && c["delta"] == json!(-1.0)), "{ch}");
        // Waste override and grouping by trade.
        let r = s.execute("bim.bom", &json!({ "waste": { "conduit": 0 }, "group_by": "trade" })).unwrap();
        assert_eq!(line(&r, "EMT conduit")["waste"], json!(0.0));
        assert!(s.execute("bim.bom", &json!({ "group_by": "nonsense" })).is_err());
    }

    #[test]
    fn spec_requirements_without_a_line_are_flagged() {
        let mut s = Session::new();
        job(&mut s);
        let mut t = cadaddy_doc::bimspec::BimType::new("WP1");
        t.category = "receptacle".into();
        t.set_field("finish", cadaddy_doc::bimspec::SpecField { value: "WHITE".into(), source: "26 27 26 2.3".into(), ..Default::default() });
        s.doc_mut().unwrap().bim_types.types.push(t);
        let r = s.execute("bim.bom", &json!({})).unwrap();
        let issues: Vec<&str> = r["spec_check"].as_array().unwrap().iter().filter_map(|c| c["issue"].as_str()).collect();
        assert!(issues.contains(&"requirement_without_line"), "{r}");
        assert_eq!(r["complete"], json!(false));
        assert!(large_conductor("4") && large_conductor("1/0") && !large_conductor("12"));
        assert_eq!(conductor_parts("3#8 THHN + 1#10G in 3/4\" EMT"), vec![(3, "8".into(), false), (1, "10".into(), true)]);
        assert_eq!(conductor_parts("2#12, 1#12G"), vec![(2, "12".into(), false), (1, "12".into(), true)]);
    }

    /// One 40' homerun or feeder route, its tag at the start with `note` (and `extra` fields).
    fn route(s: &mut Session, kind: &str, note: &str, extra: Value) {
        let mut fields = json!({"conductors": {"value": note, "source": "note"}});
        for (k, v) in extra.as_object().into_iter().flatten() {
            fields[k] = v.clone();
        }
        let spec = json!({
            "levels": [{"name": "1", "elevation_ft": 0, "height_ft": 12}],
            "mep": {
                "systems": [{"name": "E-HOMERUN", "layer": "E-HOMERUN", "kind": "cable", "size_in": [0.5, 0], "bottom_ft": 9}],
                "runs": [{"system": "E-HOMERUN", "level": "1", "points": [[0, 0], [480, 0]], "size_in": [0.5, 0], "bottoms_ft": [9]}]
            },
            "tags": [{"id": "H1", "kind": kind, "level": "1", "at_ft": [0, 0], "fields": fields}]
        });
        s.execute("bspec.set", &json!({ "replace": true, "spec": spec })).unwrap();
    }

    #[test]
    fn a_homeruns_raceway_comes_from_its_note_with_a_bushing_per_raceway_end() {
        let mut s = Session::new();
        route(&mut s, "homerun", "3#4 + 1#4N + 1#8G in 1-1/4\" RMC", json!({}));
        let r = s.execute("bim.bom", &json!({})).unwrap();
        // 40' of 1-1/4" RMC in 10' sticks: 3 couplings; two locknuts each end; straps within 3' of
        // each end and every 10': 5; straight, no elbows.
        let c = line(&r, "RMC conduit");
        assert_eq!((c["size"].clone(), c["net"].clone()), (json!("1-1/4\""), json!(40.0)), "{r}");
        assert_eq!(line(&r, "RMC coupling")["net"], json!(3.0));
        assert_eq!(line(&r, "Locknut")["net"], json!(4.0));
        assert_eq!(line(&r, "Conduit strap")["net"], json!(5.0));
        assert!(line(&r, "RMC 90° elbow").is_null() && line(&r, "EMT conduit").is_null());
        // Screw-on bushings, one at each end of the one raceway: 2, not one per conductor part.
        assert_eq!(line(&r, "Insulating bushing (screw-on)")["net"], json!(2.0));
        assert!(line(&r, "Insulating bushing (bang-on)").is_null() && line(&r, "Grounding bushing (insulated)").is_null());
        // Wire: three phases and the neutral #4, the ground #8, each 40' plus 1' at each end.
        let w = line(&r, "THHN copper");
        assert_eq!((w["size"].clone(), w["net"].clone()), (json!("#4"), json!(168.0)));
        assert_eq!((line(&r, "Copper ground, green")["size"].clone(), line(&r, "Copper ground, green")["net"].clone()), (json!("#8"), json!(42.0)));
        // EMT in two raceways: bang-on bushings, two per raceway.
        route(&mut s, "homerun", "3#2 + 1#6G in (2) 1\" EMT", json!({}));
        let r = s.execute("bim.bom", &json!({})).unwrap();
        assert_eq!(line(&r, "EMT conduit")["net"], json!(80.0));
        assert_eq!(line(&r, "EMT connector")["net"], json!(4.0));
        assert_eq!(line(&r, "EMT coupling")["net"], json!(6.0));
        assert_eq!(line(&r, "Insulating bushing (bang-on)")["net"], json!(4.0));
        // Small conductors: no bushings.
        route(&mut s, "homerun", "2#12 + 1#12G in 3/4\" EMT", json!({}));
        let r = s.execute("bim.bom", &json!({})).unwrap();
        assert!(line(&r, "Insulating bushing (bang-on)").is_null(), "{r}");
        assert_eq!(line(&r, "EMT conduit")["size"], json!("3/4\""));
    }

    #[test]
    fn a_service_feeder_gets_grounding_bushings() {
        let mut s = Session::new();
        route(&mut s, "feeder", "3#3/0 + 1#3/0N + 1#4G in 2\" RMC", json!({"service": {"value": "yes", "source": "plan"}}));
        let r = s.execute("bim.bom", &json!({})).unwrap();
        let g = line(&r, "Grounding bushing (insulated)");
        assert_eq!((g["size"].clone(), g["net"].clone()), (json!("2\""), json!(2.0)), "{r}");
        assert!(line(&r, "Insulating bushing (screw-on)").is_null());
        assert_eq!(line(&r, "RMC conduit")["net"], json!(40.0));
        // The same feeder, not a service's: plain screw-on bushings.
        route(&mut s, "feeder", "3#3/0 + 1#3/0N + 1#4G in 2\" RMC", json!({}));
        let r = s.execute("bim.bom", &json!({})).unwrap();
        assert_eq!(line(&r, "Insulating bushing (screw-on)")["net"], json!(2.0));
        assert!(line(&r, "Grounding bushing (insulated)").is_null());
    }

    #[test]
    fn implied_part_rules_come_from_the_project_rules() {
        let mut s = Session::new();
        job(&mut s);
        let r = s.execute("bim.bom", &json!({})).unwrap();
        assert!(line(&r, "Conduit strap")["rule"].as_str().unwrap().contains("bom.strap_spacing_ft"), "{r}");
        assert!(line(&r, "EMT coupling")["rule"].as_str().unwrap().contains("bom.stick_ft"));
        assert!(line(&r, "Box, 4\" square")["rule"].as_str().unwrap().contains("bom.box"));
        // Straps every 8': 25' → ceil(25/8) + 1 = 5; 5' sticks → 4 couplings; 2' makeup →
        // 2 × (30 + 4) = 68' of #12; a deeper box for receptacles.
        set_rule(&mut s, "bom.strap_spacing_ft", "8");
        set_rule(&mut s, "bom.stick_ft", "5");
        set_rule(&mut s, "bom.makeup_ft", "2");
        set_rule(&mut s, "bom.box.receptacle", "Box, 4-11/16\" square; 2-1/8\" deep");
        let r = s.execute("bim.bom", &json!({})).unwrap();
        let strap = line(&r, "Conduit strap");
        assert_eq!(strap["net"], json!(5.0), "{r}");
        assert!(strap["rule"].as_str().unwrap().contains("every 8' (bom.strap_spacing_ft)"), "{strap}");
        assert_eq!(line(&r, "EMT coupling")["net"], json!(4.0));
        assert_eq!(line(&r, "THHN copper")["net"], json!(68.0));
        assert_eq!(line(&r, "Box, 4-11/16\" square")["net"], json!(3.0));
        assert!(line(&r, "Box, 4\" square").is_null());
        // The ring by the wall's assembly.
        edit(&mut s, |spec| spec["tags"][2]["fields"]["wall"] = json!({"value": "W-3", "source": "plan"}));
        set_rule(&mut s, "bom.ring.W-3", "Plaster ring; 1 gang, 5/8\" raised");
        let r = s.execute("bim.bom", &json!({})).unwrap();
        let rings: Vec<(&str, f64)> = r["lines"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|l| l["item"] == json!("Plaster ring"))
            .map(|l| (l["size"].as_str().unwrap(), l["net"].as_f64().unwrap()))
            .collect();
        assert_eq!(rings, vec![("1 gang", 2.0), ("1 gang, 5/8\" raised", 1.0)]);
    }

    /// The job's two standard receptacles as type WP1 with `finish`, and WP1 needing WHITE.
    fn typed(s: &mut Session, finish: Option<&str>) {
        job(s);
        edit(s, |spec| {
            for i in [2, 3] {
                spec["tags"][i]["fields"]["mark"] = json!({"value": "WP1", "source": "plan"});
                if let Some(f) = finish {
                    spec["tags"][i]["fields"]["finish"] = json!({"value": f, "source": "schedule"});
                }
            }
        });
        let mut t = cadaddy_doc::bimspec::BimType::new("WP1");
        t.category = "receptacle".into();
        t.set_field("finish", cadaddy_doc::bimspec::SpecField { value: "WHITE".into(), source: "26 27 26 2.3".into(), ..Default::default() });
        s.doc_mut().unwrap().bim_types.types.push(t);
    }

    #[test]
    fn a_spec_finish_is_carried_by_lines_with_that_mark_and_finish() {
        // WHITE devices of type WP1: carried; the BOM is complete.
        let mut s = Session::new();
        typed(&mut s, Some("WHITE"));
        let r = s.execute("bim.bom", &json!({})).unwrap();
        assert_eq!(r["spec_check"], json!([]), "{r}");
        assert_eq!(r["complete"], json!(true));
        let l = line(&r, "standard duplex receptacle");
        assert_eq!((l["mark"].clone(), l["attributes"]["finish"].clone(), l["net"].clone()), (json!("WP1"), json!("WHITE"), json!(2.0)));
        // GREY: a contradiction, with the spec's source for the finish.
        let mut s = Session::new();
        typed(&mut s, Some("GREY"));
        let r = s.execute("bim.bom", &json!({})).unwrap();
        let c = &r["spec_check"][0];
        assert_eq!(
            (c["issue"].clone(), c["item"].clone(), c["field"].clone(), c["spec"].clone(), c["line"].clone(), c["source"].clone()),
            (json!("attribute_contradicts_spec"), json!("WP1"), json!("finish"), json!("WHITE"), json!("GREY"), json!("26 27 26 2.3")),
            "{r}"
        );
        assert_eq!(r["complete"], json!(false));
        // An approved submittal saying WHITE wins over the plan's GREY.
        s.execute("bim.prints", &json!({"prints": {"submittals": [{"mark": "WP1", "section": "26 27 26", "fields": {"finish": "WHITE"}}]}})).unwrap();
        let r = s.execute("bim.bom", &json!({})).unwrap();
        assert_eq!(r["spec_check"], json!([]), "{r}");
        assert_eq!(line(&r, "standard duplex receptacle")["attribute_sources"]["finish"], json!("approved submittal 26 27 26"));
        // No devices of type WP1: the requirement has no line; waived with a reason, complete.
        let mut s = Session::new();
        typed(&mut s, None);
        edit(&mut s, |spec| spec["tags"].as_array_mut().unwrap().retain(|t| t["fields"]["mark"].is_null()));
        let r = s.execute("bim.bom", &json!({})).unwrap();
        assert_eq!(
            (r["spec_check"][0]["issue"].clone(), r["spec_check"][0]["item"].clone()),
            (json!("requirement_without_line"), json!("WP1")),
            "{r}"
        );
        assert!(s.execute("bim.bom", &json!({"waive": [{"item": "WP1"}]})).is_err());
        let r = s.execute("bim.bom", &json!({"waive": [{"item": "WP1", "reason": "type WP1 deleted by addendum 2"}]})).unwrap();
        assert_eq!((r["spec_check"][0]["waived"].clone(), r["complete"].clone()), (json!("type WP1 deleted by addendum 2"), json!(true)));
    }

    /// The job plus panelboards (`tags`) on level 1.
    fn panels(s: &mut Session, tags: &[&str]) {
        job(s);
        edit(s, |spec| {
            spec["mep"]["equipment"] = json!(tags
                .iter()
                .enumerate()
                .map(|(i, t)| json!({"tag": t, "kind": "panelboard", "level": "1", "min": [i as f64 * 48.0, 100.0], "max": [i as f64 * 48.0 + 20.0, 104.0], "bottom_ft": 0, "height_ft": 6}))
                .collect::<Vec<_>>())
        });
    }

    #[test]
    fn a_job_with_a_panel_can_be_complete() {
        let mut s = Session::new();
        panels(&mut s, &["LP1"]);
        let r = s.execute("bim.bom", &json!({})).unwrap();
        let cover = line(&r, "Panel cover/trim LP1");
        assert_eq!((cover["net"].clone(), cover["status"].clone()), (json!(1.0), json!("rule")), "{r}");
        assert_eq!(r["spec_check"], json!([]));
        assert_eq!(r["complete"], json!(true));
        // Covers bought with the panel (the rules say so): missing until waived with a reason.
        set_rule(&mut s, "bom.panel_cover", "no");
        let r = s.execute("bim.bom", &json!({})).unwrap();
        assert_eq!((r["spec_check"][0]["issue"].clone(), r["complete"].clone()), (json!("accessory_missing"), json!(false)), "{r}");
        let r = s.execute("bim.bom", &json!({"waive": [{"item": "Panel cover/trim LP1", "reason": "supplied with the panel"}]})).unwrap();
        assert_eq!(r["complete"], json!(true), "{r}");
    }

    #[test]
    fn labels_and_grounding_reach_the_bom() {
        let mut s = Session::new();
        panels(&mut s, &["LP1", "LP2"]);
        edit(&mut s, |spec| {
            spec["mep"]["grounding"] = json!({"service": "LP1", "electrodes": [{"tag": "GR-1", "kind": "ground_rod", "level": "1", "at": [0, 0], "gec_size": "#6", "path": [[0, 0, 0], [240, 0, 0]]}]});
        });
        let r = s.execute("bim.bom", &json!({})).unwrap();
        // One arc flash label per labeled panel.
        let af: Vec<(&str, f64, &str)> = r["lines"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|l| l["item"].as_str().unwrap().starts_with("Arc flash label"))
            .map(|l| (l["item"].as_str().unwrap(), l["net"].as_f64().unwrap(), l["category"].as_str().unwrap()))
            .collect();
        assert_eq!(af, vec![("Arc flash label, 4\" x 6\", LP1", 1.0, "labels"), ("Arc flash label, 4\" x 6\", LP2", 1.0, "labels")], "{r}");
        // Every grounding takeoff item is a line with its quantity.
        let g = s.execute("grounding.takeoff", &json!({})).unwrap();
        let items = g["items"].as_array().unwrap();
        assert!(items.iter().any(|i| i["item"] == json!("Grounding conductor, #6 copper") && i["qty"] == json!(22.0)), "{g}");
        for i in items {
            let l = line(&r, i["item"].as_str().unwrap());
            assert_eq!((l["category"].clone(), l["net"].clone()), (json!("grounding"), i["qty"].clone()), "{i}");
        }
    }

    #[test]
    fn required_items_follow_the_crew_floors_and_work() {
        let mut s = Session::new();
        job(&mut s);
        let a = s.execute("bim.bom", &json!({"crew": 4})).unwrap();
        let b = s.execute("bim.bom", &json!({"crew": 8, "floors": 3, "weeks": 6})).unwrap();
        for (item, small, big) in [
            ("Step ladder, 8'", 2.0, 12.0),
            ("Material cart", 1.0, 6.0),
            ("Gang box", 1.0, 3.0),
            ("Padlock", 1.0, 3.0),
            ("Chain, 10'", 1.0, 3.0),
            ("Right-angle drill (hole hog)", 2.0, 4.0),
            // 3/4" EMT is 0.922" outside: the 1-1/8" bit.
            ("Self-feed bit 1-1/8\"", 2.0, 4.0),
            ("Pull rope, 600' spool", 1.0, 3.0),
            ("Knockout set", 1.0, 1.0),
        ] {
            assert_eq!((req(&a, item), req(&b, item)), (small, big), "{item}: {}", b["required"]);
        }
        assert_eq!(b["required"].as_array().unwrap().iter().find(|x| x["item"] == json!("Temporary power")).unwrap()["weeks"], json!(6.0));
        // Nothing high, no trench, no aluminum, no cable: no lift, trencher, inhibitor or staples.
        for item in ["Scissor lift, 19'", "Trencher", "Oxide inhibitor, 8 oz", "Staple gun"] {
            assert_eq!(req(&b, item), 0.0, "{item}");
        }
        // Conduit at 14': a lift per four of the crew; an encased run: a trencher; an aluminum
        // feeder: an inhibitor tube per 20 terminations; MC cable: staples and a gun.
        edit(&mut s, |spec| {
            spec["mep"]["runs"][0]["bottoms_ft"] = json!([14, 14]);
            spec["mep"]["runs"][0]["encased"] = json!(true);
            spec["mep"]["runs"][1]["points"] = json!([[0, 0], [5400, 0]]);
            spec["tags"][5]["fields"]["conductors"]["value"] = json!("3#1/0 AL + 1#1/0 AL + 1#6G in MC");
        });
        let c = s.execute("bim.bom", &json!({"crew": 8})).unwrap();
        assert_eq!(req(&c, "Scissor lift, 19'"), 2.0, "{}", c["required"]);
        assert_eq!(req(&c, "Trencher"), 1.0);
        // Five conductors, ten ends (the ground's too): one tube.
        assert_eq!(req(&c, "Oxide inhibitor, 8 oz"), 1.0);
        // 452' of MC (450' route plus 1' each end), stapled every 4.5': 101 → two boxes of 100.
        assert_eq!(line(&c, "MC cable")["net"], json!(452.0));
        assert_eq!((req(&c, "Cable staples, box of 100"), req(&c, "Staple gun")), (2.0, 4.0));
    }

    #[test]
    fn lines_group_by_room_and_phase_with_exact_totals() {
        let mut s = Session::new();
        job(&mut s);
        let r = s.execute("bim.bom", &json!({"group_by": "room"})).unwrap();
        let at = |item: &str, room: &str| {
            r["lines"]
                .as_array()
                .unwrap()
                .iter()
                .filter(|l| l["item"] == json!(item) && l["room"] == json!(room))
                .map(|l| l["net"].as_f64().unwrap())
                .sum::<f64>()
        };
        assert_eq!((at("Luminaire type A", "101"), at("Luminaire type A", "102")), (1.0, 1.0), "{r}");
        assert_eq!((at("standard duplex receptacle", "101"), at("GFCI receptacle", "102")), (2.0, 1.0));
        assert_eq!((at("Device plate", "101"), at("Device plate", "102")), (2.0, 1.0));
        // The conduit carries no room: unassigned.
        assert_eq!(at("EMT conduit", ""), 25.0);
        edit(&mut s, |spec| {
            for i in 0..5 {
                spec["tags"][i]["fields"]["phase"] = json!({"value": if i < 2 { "trim" } else { "rough-in" }, "source": "user"});
            }
        });
        let r = s.execute("bim.bom", &json!({"group_by": "phase"})).unwrap();
        let ph = |item: &str, phase: &str| {
            r["lines"]
                .as_array()
                .unwrap()
                .iter()
                .filter(|l| l["item"] == json!(item) && l["phase"] == json!(phase))
                .map(|l| l["net"].as_f64().unwrap())
                .sum::<f64>()
        };
        assert_eq!((ph("Luminaire type A", "trim"), ph("Box, 4\" square", "rough-in")), (2.0, 3.0), "{r}");
        // The summary totals every category.
        let sum = |cat: &str| r["summary"].as_array().unwrap().iter().find(|x| x["category"] == json!(cat)).unwrap()["total"].clone();
        // 2% of two luminaires rounds to none; 10% of 25' of conduit is 2.5'.
        assert_eq!((sum("fixtures"), sum("conduit")), (json!(2.0), json!(27.5)));
    }

    #[test]
    fn status_comes_from_the_element() {
        let mut s = Session::new();
        job(&mut s);
        edit(&mut s, |spec| {
            let tags = spec["tags"].as_array_mut().unwrap();
            for (id, ty, status, source) in [
                ("Z1", "zoned receptacle", "zone", "plan"),
                ("F1", "verified receptacle", "field_verified", "plan"),
                ("A1", "agent receptacle", "agent", "plan"),
                ("U1", "user receptacle", "", "user"),
            ] {
                tags.push(json!({"id": id, "kind": "receptacle", "level": "1", "at_ft": [9, 9], "status": status, "fields": {"device_type": {"value": ty, "source": source}}}));
            }
        });
        let r = s.execute("bim.bom", &json!({})).unwrap();
        for (item, want) in [
            ("GFCI receptacle", "fixed"),
            ("standard duplex receptacle", "rule"),
            ("zoned receptacle", "zone"),
            ("verified receptacle", "field_verified"),
            ("user receptacle", "override"),
            ("agent receptacle", "agent"),
            // Wire from the documents' note is fixed, not a rule; the parts a rule implies are.
            ("THHN copper", "fixed"),
            ("Plaster ring", "rule"),
        ] {
            assert_eq!(line(&r, item)["status"], json!(want), "{item}");
        }
        // A rule's default conductors (no note) are a rule.
        edit(&mut s, |spec| spec["tags"].as_array_mut().unwrap().retain(|t| t["id"] != json!("H1")));
        let r = s.execute("bim.bom", &json!({})).unwrap();
        assert_eq!(line(&r, "THHN copper")["status"], json!("rule"));
    }

    #[test]
    fn raceway_notes_and_sizes_parse() {
        assert_eq!(parse_raceway("3#4 + 1#8G in 1-1/4\" RMC"), Some(Raceway { count: 1, size_in: 1.25, material: "RMC".into() }));
        assert_eq!(parse_raceway("4#500 in (3) 3\" PVC"), Some(Raceway { count: 3, size_in: 3.0, material: "PVC".into() }));
        assert_eq!(parse_raceway("2#12 in 3/4\" RGS"), Some(Raceway { count: 1, size_in: 0.75, material: "RMC".into() }));
        assert_eq!(parse_raceway("3#12 in MC").map(|r| r.material), Some("MC".into()));
        assert_eq!(parse_raceway("2#12 + 1#12G"), None);
        assert_eq!(parse_raceway("2#12 in 3/4\" BANANA"), None);
        assert_eq!(
            (awg_order("12"), awg_order("4"), awg_order("1/0"), awg_order("4/0"), awg_order("250")),
            (Some(12), Some(4), Some(0), Some(-3), Some(-6))
        );
        assert!((bit_for(raceway_od("EMT", 0.75)) - 1.125).abs() < 1e-9 && (bit_for(raceway_od("RMC", 1.25)) - 1.875_f64.max(2.0)).abs() < 1e-9);
    }
}
