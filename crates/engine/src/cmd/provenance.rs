//! Provenance and corrections (Q6216; owner 11-05: the field builds what the drawings leave out
//! from code and trade practice, and corrects what they get wrong).
//!
//! **Provenance.** Every object of the model says what it is by the documents ([`Made`]): drawn
//! by them, derived (copied from a named source that matches: another floor's plan, a typical
//! room's), or inferred (laid in from a rule of the code or of trade practice where nothing draws
//! it), and, when it is not drawn, its basis (the source or the rule). One mark, one reader:
//! - a built element: its build tags `status:<made>` and `basis:<text>` ([`of_element`], [`tags`]);
//! - a spec run: [`MepRun::made`] and [`MepRun::basis`] ([`of_run`]).
//!
//! The marks earlier builds wrote are read as the same thing, never kept as fields of their own:
//! a run whose `height_source` begins `derived` / `inferred`, a branch route the router laid
//! (`routed`) or estimated (`estimated route`), an element a note or a rule proposed
//! (`prop:STATUS=proposed`), a column put at a grid crossing by rule (`status:rule`), a piece on
//! an estimated route (`estimated`). [`stamp`] writes the mark on every built element that has
//! none, so each carries one; nothing filled in can pass for drawn.
//!
//! **Corrections.** Where the documents conflict with each other, with the code or with physical
//! reality, the build installs what the field would and keeps the design value beside it
//! ([`correct`]): the element's property holds the corrected value (the rule its source, the
//! reason its note), `design:<key>` the design value, `correction:<key>` the rule and the reason.
//! A design value once kept is never overwritten. Takeoff and field output read the property (the
//! corrected value); `bim.provenance` shows each difference.

use std::collections::{BTreeMap, HashMap};

use cadaddy_doc::props::{self, Prop, Props, Source, SourceKind};
use cadaddy_doc::{Drawing, Entity, Handle, RawTag};
use cadaddy_walk3d::{Made, MepRun};
use serde::Deserialize;
use serde_json::{Value, json};

use super::*;
use crate::{Result, Session};

/// The build tag of an element's provenance (`status:inferred`) and of its basis.
pub(crate) const STATUS_TAG: &str = "status";
pub(crate) const BASIS_TAG: &str = "basis";
/// The property prefixes of a correction: the design value kept, and the rule and reason.
pub(crate) const DESIGN_PREFIX: &str = "design:";
pub(crate) const CORRECTION_PREFIX: &str = "correction:";
/// Longest basis kept on an element.
const MAX_BASIS: usize = 240;
/// Most elements looked at (hostile drawings), listed, and bases reported.
const MAX_ELEMENTS: usize = 3_000_000;
const MAX_LISTED: usize = 500;
const MAX_BASES: usize = 60;

/// The trade practice rules data (`tradepractice.json`).
const RULES: &str = include_str!("tradepractice.json");

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new("bim.provenance", "Model Provenance", run_provenance)
            .params("{level?, part?: what the build made it as (wall, device, mep…), made?: drawn|derived|inferred, list?: n (elements listed, default 0, at most 500), corrections?: bool (list each corrected value beside its design value), connections?: bool (what is connected per trade: fixtures to their lines, power devices to a panel, air terminals to a duct)} → {elements, marked (carry the mark themselves), drawn, derived, inferred, derived_with_basis, inferred_with_basis, inferred_without_basis, by_part: {part: {drawn, derived, inferred}}, bases: [{made, basis, count}], corrections: {elements, values, by_rule: {rule: n}, listed?: [{handle, key, design, corrected, reason}]}, listed?: [{handle, part, level, made, basis}], runs?: {by_system: {<kind and system>: {drawn|derived|inferred: {runs, ft}}}, circuits_corrected}, connections?: {plumbing: {fixtures, connected, unconnected, unconnected_by_level}, electrical: {devices, connected, unconnected, by_kind}, hvac: {terminals, connected, connected_by_inferred_duct}}}: what each built object is by the documents (drawn, derived from a matching source, or inferred by a rule) with its basis, and every value the build corrected for the field beside its design value")
            .noundo(),
    ]
}

// ─── Rules data ──────────────────────────────────────────────────────────────

/// Words as whole words: upper case, anything but letters and digits a space, a space each side
/// (`EWC` is not `WC`).
fn spaced(s: &str) -> String {
    let w: Vec<String> = s.split(|c: char| !c.is_ascii_alphanumeric()).filter(|w| !w.is_empty()).map(str::to_ascii_uppercase).collect();
    format!(" {} ", w.join(" "))
}

/// One trade practice rule: what it fills in or corrects, and why (the basis every object it
/// lays in carries). Its numbers are the trade's own fields.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub(crate) struct PracticeRule {
    pub id: String,
    pub trade: String,
    /// What the rule fills in or corrects.
    pub fills: String,
    /// Why: the trade practice, citing the code where the code sets a number.
    pub basis: String,
    /// The systems it lays in (a fixture's waste or vent; a terminal's duct).
    pub systems: Vec<String>,
    /// Farthest a need is served from (feet).
    pub reach_ft: f64,
    /// A need this close to its system is already connected (feet).
    pub connect_reach_ft: f64,
    /// A size by the words of what is served: the first row whose words are in its name.
    pub sizes: Vec<SizeRow>,
    /// The size when no row matches (inches).
    pub default_size_in: f64,
}

/// A size by the words of what is served (a fixture's schedule name, a terminal's kind).
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub(crate) struct SizeRow {
    pub words: Vec<String>,
    pub size_in: f64,
}

#[derive(Default, Deserialize)]
#[serde(default)]
struct RuleFile {
    rules: Vec<PracticeRule>,
}

/// The trade practice rules.
pub(crate) fn rules() -> Vec<PracticeRule> {
    serde_json::from_str::<RuleFile>(RULES).map(|f| f.rules).unwrap_or_default()
}

/// The rule `id` (an empty one when the data has none).
pub(crate) fn rule(id: &str) -> PracticeRule {
    rules().into_iter().find(|r| r.id.eq_ignore_ascii_case(id)).unwrap_or_default()
}

impl PracticeRule {
    /// The size for what `words` names (its schedule name, its kind), else the default.
    pub(crate) fn size_for(&self, words: &str) -> f64 {
        let up = spaced(words);
        self.sizes.iter().find(|r| r.words.iter().any(|w| up.contains(&spaced(w)))).map_or(self.default_size_in, |r| r.size_in).max(0.0)
    }

    /// The basis an object this rule lays in carries: the rule's id and its reason.
    pub(crate) fn basis_for(&self, what: &str) -> String {
        let what = what.trim();
        if what.is_empty() { format!("{}: {}", self.id, self.basis) } else { format!("{}: {} ({what})", self.id, self.basis) }
    }
}

// ─── The mark ────────────────────────────────────────────────────────────────

/// The build tags of a provenance: `status:<made>`, and `basis:<text>` when it is not drawn.
pub(crate) fn tags(made: Made, basis: &str) -> Vec<RawTag> {
    let mut v = vec![RawTag { code: 1000, value: format!("{STATUS_TAG}:{}", made.as_str()) }];
    let b = basis.trim();
    if !made.is_drawn() && !b.is_empty() {
        v.push(RawTag { code: 1000, value: format!("{BASIS_TAG}:{}", b.chars().take(MAX_BASIS).collect::<String>()) });
    }
    v
}

/// The word an older build put first in a run's height source (`derived (capped under …)`).
fn first_word(s: &str) -> &str {
    s.trim().split(|c: char| !c.is_ascii_alphabetic()).next().unwrap_or("")
}

/// What a run is by the documents, and its basis: its own mark, else the marks earlier builds
/// wrote (a height source beginning `derived` / `inferred` with the basis in its size source; a
/// route the router laid or estimated).
pub(crate) fn of_run(r: &MepRun) -> (Made, String) {
    if !r.made.is_drawn() {
        let basis = if r.basis.trim().is_empty() { r.size_source.clone() } else { r.basis.clone() };
        return (r.made, basis);
    }
    if let Some(m) = Made::parse(first_word(&r.height_source)).filter(|m| !m.is_drawn()) {
        return (m, r.size_source.clone());
    }
    let hs = r.height_source.trim();
    if hs.eq_ignore_ascii_case(super::conduitroute::ROUTED) {
        return (Made::Inferred, rule("E-ROUTE").basis_for(""));
    }
    if hs.eq_ignore_ascii_case(super::printcond::ESTIMATED) {
        return (Made::Inferred, rule("E-ESTIMATED").basis_for(""));
    }
    (Made::Drawn, String::new())
}

/// What a run is by the documents ([`of_run`] without the basis).
pub(crate) fn made_of_run(r: &MepRun) -> Made {
    of_run(r).0
}

/// Mark a run: what it is and why.
pub(crate) fn set_run(r: &mut MepRun, made: Made, basis: impl Into<String>) {
    r.made = made;
    r.basis = if made.is_drawn() { String::new() } else { basis.into() };
}

/// What a built element is by the documents, and its basis (`None`: not a built element): its
/// own mark, else the marks earlier builds wrote.
pub(crate) fn of_element(e: &Entity) -> Option<(Made, String)> {
    super::bimlook::made_tags(e)?;
    let basis = || super::bimlook::made_tag(e, BASIS_TAG).unwrap_or_default().to_string();
    if let Some(v) = super::bimlook::made_tag(e, STATUS_TAG) {
        if let Some(m) = Made::parse(v) {
            return Some((m, if m.is_drawn() { String::new() } else { basis() }));
        }
        // A column put at a grid crossing by rule (`status:rule`).
        if v.eq_ignore_ascii_case("rule") {
            let b = basis();
            return Some((Made::Inferred, if b.is_empty() { rule("A-GRID-COLUMN").basis_for("") } else { b }));
        }
    }
    // An element a step built from a rule or a matching source says so in its properties
    // (`prop:STATUS=inferred`, `prop:BASIS=…`).
    if let Some(m) = super::bimlook::made_prop(e, "STATUS").and_then(Made::parse) {
        let b = super::bimlook::made_prop(e, "BASIS").unwrap_or_default().to_string();
        return Some((m, if m.is_drawn() { String::new() } else { b }));
    }
    if super::bimlook::made_proposed(e) {
        let b = ["NOT_DRAWN", "BASIS", "RULE", "WHY"]
            .iter()
            .find_map(|k| super::bimlook::made_prop(e, k).filter(|v| !v.trim().is_empty()))
            .map_or_else(|| "proposed: only a note or a rule calls for it, no document draws it".to_string(), str::to_string);
        return Some((Made::Inferred, b));
    }
    let ps = props::read(&e.common.xdata);
    if ps.get("estimated").is_some_and(|p| p.text() == "true") {
        return Some((Made::Inferred, rule("E-ESTIMATED").basis_for("")));
    }
    Some((Made::Drawn, String::new()))
}

/// `xdata` with the provenance tags put at the end of the build's group (a mark already there
/// replaced).
fn with_mark(xdata: &[RawTag], made: Made, basis: &str) -> Vec<RawTag> {
    let mut out: Vec<RawTag> = Vec::with_capacity(xdata.len() + 2);
    let mut in_build = false;
    let mut placed = false;
    let put = |out: &mut Vec<RawTag>| out.extend(tags(made, basis));
    for t in xdata {
        if t.code == 1001 {
            if in_build && !placed {
                put(&mut out);
                placed = true;
            }
            in_build = t.value.trim().eq_ignore_ascii_case(super::wallgen::APP);
        } else if in_build && t.code == 1000 {
            let key = t.value.split_once(':').map(|(k, _)| k.trim());
            if key.is_some_and(|k| k.eq_ignore_ascii_case(STATUS_TAG) || k.eq_ignore_ascii_case(BASIS_TAG)) {
                continue;
            }
        }
        out.push(t.clone());
    }
    if in_build && !placed {
        put(&mut out);
    }
    out
}

/// What [`stamp`] did.
#[derive(Debug, Default)]
pub(crate) struct Stamped {
    /// Built elements, those that already carried the mark, and those marked now by what they are.
    pub elements: usize,
    pub had: usize,
    pub marked: BTreeMap<&'static str, usize>,
}

impl Stamped {
    pub(crate) fn to_json(&self) -> Value {
        json!({"elements": self.elements, "had_mark": self.had, "marked_now": self.marked})
    }
}

/// Mark every built element that carries no provenance of its own with what it is ([`of_element`]:
/// the marks earlier steps wrote), and a not-drawn one with no basis with the generic basis of
/// its kind of mark. After this every built element carries `status:`.
pub(crate) fn stamp(d: &mut Drawing) -> Stamped {
    let mut out = Stamped::default();
    let mut todo: Vec<(Handle, Made, String)> = Vec::new();
    for e in d.model.iter().take(MAX_ELEMENTS) {
        let Some((m, b)) = of_element(e) else { continue };
        out.elements += 1;
        let own = super::bimlook::made_tag(e, STATUS_TAG).and_then(Made::parse);
        if own.is_some() && (m.is_drawn() || !b.trim().is_empty()) {
            out.had += 1;
            continue;
        }
        let b = if !m.is_drawn() && b.trim().is_empty() { format!("{}: no basis was recorded", m.as_str()) } else { b };
        todo.push((e.handle, m, b));
    }
    for (h, m, b) in todo {
        if d.modify_entity(h, |e| {
            let x = with_mark(&e.common.xdata, m, &b);
            e.common.xdata = x.into();
        })
        .is_ok()
        {
            *out.marked.entry(m.as_str()).or_default() += 1;
        }
    }
    out
}

// ─── Corrections ─────────────────────────────────────────────────────────────

/// One value the build corrects for the field: the property, the design value (as the documents
/// give it), the value installed, why, and the rule.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Correction {
    pub key: String,
    pub design: String,
    pub corrected: String,
    pub reason: String,
    pub rule: String,
}

/// Put a correction into an element's properties: the property takes the corrected value (the
/// rule its source, the reason its note), `design:<key>` keeps the design value, `correction:<key>`
/// the rule and the reason. A design value kept by an earlier correction stays: the first design
/// value is the documents', a later correction never overwrites it. False when nothing changes
/// (the corrected value is the design value, or the store is full).
pub(crate) fn correct(ps: &mut Props, c: &Correction, design_source: Source) -> bool {
    if c.key.trim().is_empty() || c.corrected.trim() == c.design.trim() {
        return false;
    }
    let dkey = format!("{DESIGN_PREFIX}{}", c.key);
    if !ps.get(&dkey).is_some_and(Prop::has_value) && !ps.set(&dkey, Prop::new(c.design.clone(), design_source)) {
        return false;
    }
    let mut p = Prop::new(c.corrected.clone(), Source::from_doc(SourceKind::Rule, &c.rule));
    p.note = Some(c.reason.clone());
    if !ps.set(&c.key, p) {
        return false;
    }
    let mut why = Prop::new(format!("{}: {}", c.rule, c.reason), Source::from_doc(SourceKind::Rule, &c.rule));
    why.note = Some(format!("design {} → installed {}", c.design, c.corrected));
    ps.set(&format!("{CORRECTION_PREFIX}{}", c.key), why)
}

/// The corrections in an element's properties: (key, design, corrected, why).
pub(crate) fn corrections_of(ps: &Props) -> Vec<(String, String, String, String)> {
    ps.iter()
        .filter_map(|(k, p)| {
            let key = k.strip_prefix(DESIGN_PREFIX)?;
            let now = ps.get(key).map(Prop::text).unwrap_or_default();
            let why = ps.get(&format!("{CORRECTION_PREFIX}{key}")).map(Prop::text).unwrap_or_default();
            Some((key.to_string(), p.text(), now, why))
        })
        .collect()
}

/// Correct a built element's property in the drawing ([`correct`]); the design value's source is
/// the property's own (else the documents'). True when it changed.
pub(crate) fn correct_element(d: &mut Drawing, h: Handle, c: &Correction) -> bool {
    let mut changed = false;
    let _ = d.modify_entity(h, |e| {
        let mut ps = props::read(&e.common.xdata);
        let src = ps.get(&c.key).map_or_else(|| Source::of(SourceKind::Print), |p| p.source.clone());
        if correct(&mut ps, c, src) {
            let x = props::write(&e.common.xdata, &ps);
            e.common.xdata = x.into();
            changed = true;
        }
    });
    changed
}

// ─── The report ──────────────────────────────────────────────────────────────

#[derive(Default)]
struct Count {
    by: [usize; 3],
}

impl Count {
    fn add(&mut self, m: Made) {
        if let Some(n) = self.by.get_mut(m as usize) {
            *n += 1;
        }
    }
    fn json(&self) -> Value {
        let [drawn, derived, inferred] = self.by;
        json!({"drawn": drawn, "derived": derived, "inferred": inferred})
    }
}

/// What the built model is by the documents (`bim.provenance`).
pub(crate) fn summary(d: &Drawing, level: Option<&str>, part: Option<&str>, made: Option<Made>, list: usize, list_corrections: bool) -> Value {
    let mut all = Count::default();
    let mut by_part: BTreeMap<String, Count> = BTreeMap::new();
    let mut bases: HashMap<(Made, String), usize> = HashMap::new();
    let (mut elements, mut marked) = (0usize, 0usize);
    let (mut derived_basis, mut inferred_basis, mut inferred_none) = (0usize, 0usize, 0usize);
    let mut listed: Vec<Value> = Vec::new();
    let (mut corrected_elements, mut corrected_values) = (0usize, 0usize);
    let mut by_rule: BTreeMap<String, usize> = BTreeMap::new();
    let mut corrections: Vec<Value> = Vec::new();
    for e in d.model.iter().take(MAX_ELEMENTS) {
        let Some((m, b)) = of_element(e) else { continue };
        let p = super::wallgen::made_as(e).unwrap_or_default();
        let l = super::bspec::made_level(e).unwrap_or_default();
        if level.is_some_and(|x| !x.eq_ignore_ascii_case(&l)) || part.is_some_and(|x| !x.eq_ignore_ascii_case(&p)) || made.is_some_and(|x| x != m) {
            continue;
        }
        elements += 1;
        marked += usize::from(super::bimlook::made_tag(e, STATUS_TAG).and_then(Made::parse).is_some());
        all.add(m);
        by_part.entry(p.clone()).or_default().add(m);
        let has_basis = !b.trim().is_empty();
        match m {
            Made::Derived => derived_basis += usize::from(has_basis),
            Made::Inferred => {
                inferred_basis += usize::from(has_basis);
                inferred_none += usize::from(!has_basis);
            }
            Made::Drawn => {}
        }
        if !m.is_drawn() {
            // The basis without what it served (`rule: why (what)`), so like ones count together.
            let key = b.split(" (").next().unwrap_or(&b).chars().take(160).collect::<String>();
            *bases.entry((m, key)).or_default() += 1;
        }
        if listed.len() < list {
            listed.push(json!({"handle": e.handle.hex(), "part": p, "level": l, "made": m.as_str(), "basis": b}));
        }
        let ps = props::read(&e.common.xdata);
        let cs = corrections_of(&ps);
        if !cs.is_empty() {
            corrected_elements += 1;
            corrected_values += cs.len();
            for (k, design, now, why) in cs {
                let rule = why.split(':').next().unwrap_or_default().trim().to_string();
                *by_rule.entry(rule).or_default() += 1;
                if list_corrections && corrections.len() < MAX_LISTED {
                    corrections.push(json!({"handle": e.handle.hex(), "part": p, "key": k, "design": design, "corrected": now, "reason": why}));
                }
            }
        }
    }
    let mut bases: Vec<((Made, String), usize)> = bases.into_iter().collect();
    bases.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    let [drawn, derived, inferred] = all.by;
    let mut out = json!({
        "elements": elements,
        "marked": marked,
        "drawn": drawn, "derived": derived, "inferred": inferred,
        "derived_with_basis": derived_basis, "inferred_with_basis": inferred_basis, "inferred_without_basis": inferred_none,
        "by_part": by_part.iter().map(|(k, c)| (k.clone(), c.json())).collect::<serde_json::Map<_, _>>(),
        "bases": bases.iter().take(MAX_BASES).map(|((m, b), n)| json!({"made": m.as_str(), "basis": b, "count": n})).collect::<Vec<_>>(),
        "corrections": {"elements": corrected_elements, "values": corrected_values, "by_rule": by_rule},
    });
    if list_corrections {
        out["corrections"]["listed"] = json!(corrections);
    }
    if list > 0 {
        out["listed"] = json!(listed);
    }
    out
}

/// The spec's runs by kind and what they are: how many and their plan feet (drawn feet are what
/// the documents draw; a fill-in adds derived and inferred feet, never changes drawn ones), and
/// the circuits whose values a correction changed (their design values kept).
fn spec_runs(sp: &cadaddy_walk3d::BuildingSpec) -> Value {
    let upf = if sp.units_per_foot.is_finite() && sp.units_per_foot > 0.0 { sp.units_per_foot } else { 12.0 };
    let mut by: BTreeMap<String, BTreeMap<&'static str, (usize, f64)>> = BTreeMap::new();
    for r in sp.mep.runs.iter().take(MAX_ELEMENTS) {
        let kind = sp.mep.systems.iter().find(|y| y.name == r.system).map_or("other", |y| match y.kind {
            cadaddy_walk3d::MepKind::Duct => "duct",
            cadaddy_walk3d::MepKind::Pipe => "pipe",
            cadaddy_walk3d::MepKind::Conduit => "conduit",
            cadaddy_walk3d::MepKind::Tray => "tray",
            cadaddy_walk3d::MepKind::Cable => "cable",
        });
        let ft = r.points.windows(2).map(|w| (w[1][0] - w[0][0]).hypot(w[1][1] - w[0][1])).sum::<f64>() / upf;
        let e = by.entry(format!("{kind} {}", r.system)).or_default().entry(made_of_run(r).as_str()).or_default();
        e.0 += 1;
        e.1 += if ft.is_finite() { ft } else { 0.0 };
    }
    let corrected = sp.tags.iter().filter(|t| t.fields.keys().any(|k| k.starts_with(DESIGN_PREFIX))).count();
    json!({
        "by_system": by.iter().map(|(k, m)| (k.clone(), Value::Object(m.iter().map(|(made, (n, ft))| (made.to_string(), json!({"runs": n, "ft": (ft * 10.0).round() / 10.0}))).collect()))).collect::<serde_json::Map<_, _>>(),
        "circuits_corrected": corrected,
    })
}

fn run_provenance(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "bim.provenance";
    let made = match str_param(p, "made") {
        Some(m) => Some(Made::parse(m).ok_or_else(|| bad(CMD, "`made` is drawn, derived or inferred"))?),
        None => None,
    };
    let list = p.get("list").and_then(Value::as_u64).map_or(0, |n| (n as usize).min(MAX_LISTED));
    let corrections = p.get("corrections").and_then(Value::as_bool).unwrap_or(false);
    let mut out = summary(s.doc()?, str_param(p, "level"), str_param(p, "part"), made, list, corrections);
    let sp = super::bspec::current(s, CMD).ok().map(|x| x.0);
    if let Some(sp) = &sp {
        out["runs"] = spec_runs(sp);
    }
    if p.get("connections").and_then(Value::as_bool).unwrap_or(false) {
        out["connections"] = super::fillin::connections(s.doc()?, sp.as_ref());
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use cadaddy_doc::{Common, EntityKind, Space};

    fn built(d: &mut Drawing, extra: &[&str]) -> Handle {
        let mut x = vec![
            RawTag { code: 1001, value: super::super::wallgen::APP.into() },
            RawTag { code: 1000, value: "mep".into() },
            RawTag { code: 1000, value: "1".into() },
            RawTag { code: 1000, value: "1A".into() },
        ];
        x.extend(extra.iter().map(|v| RawTag { code: 1000, value: (*v).into() }));
        d.add(&Space::Model, Common { xdata: x.into(), ..Common::default() }, EntityKind::Mesh(super::super::wallgen::empty_mesh()))
            .unwrap_or_else(|e| panic!("{e}"))
    }

    #[test]
    fn the_older_marks_read_as_one_provenance() {
        let mut d = Drawing::default();
        let drawn = built(&mut d, &["system:CW"]);
        let inf = built(&mut d, &["status:inferred", "basis:P-X: a rule"]);
        let col = built(&mut d, &["status:rule"]);
        let prop = built(&mut d, &["prop:STATUS=proposed", "prop:NOT_DRAWN=no document draws it: rules data"]);
        let by_prop = built(&mut d, &["prop:STATUS=inferred", "prop:BASIS=M-X: a code rule"]);
        let get = |h: Handle| d.entity(h).and_then(|e| of_element(e)).unwrap_or_else(|| panic!("built"));
        assert_eq!(get(drawn), (Made::Drawn, String::new()));
        assert_eq!(get(inf), (Made::Inferred, "P-X: a rule".to_string()));
        assert_eq!(get(col).0, Made::Inferred);
        assert!(!get(col).1.is_empty(), "a rule column says why");
        assert_eq!(get(prop), (Made::Inferred, "no document draws it: rules data".to_string()));
        assert_eq!(get(by_prop), (Made::Inferred, "M-X: a code rule".to_string()));
    }

    #[test]
    fn a_runs_older_marks_and_its_own_are_one_provenance() {
        let mut r: MepRun =
            serde_json::from_value(json!({"system": "CW", "level": "1", "points": [[0, 0], [10, 0]], "size_in": [1, 0], "bottoms_ft": [9]}))
                .unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(of_run(&r).0, Made::Drawn);
        r.height_source = "derived (capped under the floor above)".into();
        r.size_source = "derived from Level 1".into();
        assert_eq!(of_run(&r), (Made::Derived, "derived from Level 1".to_string()));
        r.height_source = super::super::conduitroute::ROUTED.into();
        assert_eq!(of_run(&r).0, Made::Inferred);
        assert!(of_run(&r).1.starts_with("E-ROUTE"), "{}", of_run(&r).1);
        set_run(&mut r, Made::Inferred, "P-BRANCH-WASTE: a rule");
        assert_eq!(of_run(&r), (Made::Inferred, "P-BRANCH-WASTE: a rule".to_string()));
        // Kept in the spec, read back the same.
        let back: MepRun = serde_json::from_value(serde_json::to_value(&r).unwrap_or_default()).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(of_run(&back), of_run(&r));
    }

    #[test]
    fn every_built_element_carries_the_mark_after_stamping_and_no_inferred_one_reads_drawn() {
        let mut d = Drawing::default();
        built(&mut d, &["system:CW"]);
        built(&mut d, &["status:inferred"]);
        built(&mut d, &["prop:STATUS=proposed"]);
        built(&mut d, &["status:derived", "basis:copied from Level 1"]);
        // A drawing's own line (not built) is no element.
        let _ = d.add(
            &Space::Model,
            Common::default(),
            EntityKind::Line(cadaddy_doc::Line { a: cadaddy_geom::Vec3::new(0.0, 0.0, 0.0), b: cadaddy_geom::Vec3::new(1.0, 0.0, 0.0) }),
        );
        let st = stamp(&mut d);
        assert_eq!(st.elements, 4);
        let s = summary(&d, None, None, None, 10, false);
        assert_eq!(s["elements"], 4);
        assert_eq!(s["marked"], 4, "{s}");
        assert_eq!((s["drawn"].as_u64(), s["derived"].as_u64(), s["inferred"].as_u64()), (Some(1), Some(1), Some(2)));
        assert_eq!(s["inferred_without_basis"], 0, "{s}");
        assert_eq!(s["inferred_with_basis"], 2);
        // No element with an inferred mark of any age reads as drawn.
        for e in d.model.iter() {
            let tags: Vec<&str> = e.common.xdata.iter().map(|t| t.value.as_str()).collect();
            let older = tags.iter().any(|t| *t == "status:inferred" || t.contains("STATUS=proposed"));
            if older {
                assert_eq!(of_element(e).map(|x| x.0), Some(Made::Inferred), "{tags:?}");
            }
        }
        // Stamping again changes nothing.
        let again = stamp(&mut d);
        assert_eq!((again.elements, again.had), (4, 4));
    }

    #[test]
    fn a_correction_keeps_the_design_value_and_never_overwrites_it() {
        let mut ps = Props::default();
        ps.set("size", Prop::new("4\" square", Source::from_doc(SourceKind::Print, "E-91")));
        let c = Correction {
            key: "size".into(),
            design: "4\" square".into(),
            corrected: "4-11/16\" square".into(),
            reason: "box fill 34.5 cu in over 21".into(),
            rule: "NEC 314.16".into(),
        };
        assert!(correct(&mut ps, &c, Source::from_doc(SourceKind::Print, "E-91")));
        assert_eq!(ps.get("size").map(Prop::text).as_deref(), Some("4-11/16\" square"));
        assert_eq!(ps.get("design:size").map(Prop::text).as_deref(), Some("4\" square"));
        // A second correction of the same property keeps the first design value.
        let c2 = Correction { design: "4-11/16\" square".into(), corrected: "5\" square".into(), ..c.clone() };
        assert!(correct(&mut ps, &c2, Source::computed()));
        assert_eq!(ps.get("size").map(Prop::text).as_deref(), Some("5\" square"));
        assert_eq!(ps.get("design:size").map(Prop::text).as_deref(), Some("4\" square"), "the design value is never overwritten");
        let listed = corrections_of(&ps);
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].1, "4\" square");
        assert!(listed[0].3.starts_with("NEC 314.16"), "{listed:?}");
        // No change when the value is already right.
        let same = Correction { design: "5\" square".into(), corrected: "5\" square".into(), ..c };
        assert!(!correct(&mut ps, &same, Source::computed()));
    }

    #[test]
    fn the_trade_practice_rules_are_data_with_their_bases() {
        let rs = rules();
        assert!(rs.len() >= 5, "{}", rs.len());
        for r in &rs {
            assert!(!r.id.is_empty() && !r.trade.is_empty() && !r.fills.is_empty(), "{r:?}");
            assert!(r.basis.len() > 40, "each rule says why: {r:?}");
        }
        let w = rule("P-BRANCH-WASTE");
        assert!(w.size_for("WC-1 WATER CLOSET") > w.size_for("L-1 LAVATORY"));
        assert_eq!(w.size_for("EWC-1 COOLER"), 1.25, "a word inside another word (WC in EWC) is not it");
        assert!(w.size_for("anything else") > 0.0);
    }
}
