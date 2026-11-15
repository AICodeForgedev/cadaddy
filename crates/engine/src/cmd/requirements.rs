//! What each object needs before it can be installed (Q6619; owner 11-13: "i write into software
//! building codes, rules and build a proper bim then the rfi takes care of itself").
//!
//! The RFI list is what the build produces when it cannot complete an object the way the codes
//! and trade practice require, raised on that object, never a side comparison. Each kind of
//! object has its requirements as rule data (`requirements.json`): a luminaire needs a circuit, a
//! fixture type the schedule lists and a control; a receptacle a circuit; a switch a load; powered
//! equipment a circuit at its nameplate rating; a panel a feeder. Each requirement says which
//! property meets it, the code section or trade rule behind it, the question it asks and the
//! build step it must be answered before.
//!
//! [`evaluate`] looks at every object a requirement applies to and sorts it: met by the documents
//! (a property read from a sheet, schedule, specification, submittal or note, or computed from
//! them), completed by a rule (a value the build assumed: still open), or missing. Each object
//! left open is a finding on that object; the findings of one requirement in one space are one
//! RFI ([`RfiKind::Requirement`]), written the way a contractor writes it: where, what each
//! document shows, what is asked, what the build proposes meanwhile and the step it blocks.
//!
//! The build does not stop at a finding: [`apply`] completes the object by the next source (the
//! neighbour's value: the fixture type, circuit or control of the nearest object of its kind
//! that the documents give one) as a proposal marked unsure (source `rule`, low confidence, its
//! basis and the RFI in its note) so the takeoff and the estimate carry it, and marks the object
//! with the open requirement (`open:<requirement>`). Nothing filled in passes for drawn.
//!
//! The counts are measured, not assumed: each requirement reports the objects it looked at and
//! what became of them; a requirement that found no object to look at "did not run", never
//! "clean".

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::OnceLock;

use cadaddy_doc::props::{self, Prop, Source, SourceKind};
use cadaddy_doc::{Drawing, Handle};
use cadaddy_walk3d::{BuildingSpec, Rfi, RfiKind, RfiSide, RfiStatus, Severity};
use serde::Deserialize;
use serde_json::{Value, json};

use super::propuse::{El, Model};
use crate::{Result, Session};

/// The requirements rules data.
const RULES: &str = include_str!("requirements.json");
/// The property prefix that marks an object carrying an open requirement (`open:E-LUM-CIRCUIT`).
pub(crate) const OPEN_PREFIX: &str = "open:";
/// Objects one requirement looks at, at most (hostile models).
const MAX_OBJECTS: usize = 500_000;
/// RFIs the requirements raise at most (the rest are counted).
const MAX_RAISED: usize = 2_000;
/// Items a report lists.
const MAX_LISTED: usize = 20;
/// Labels named in one RFI's text.
const MAX_NAMED: usize = 12;
/// The confidence a proposal carries: unsure.
const PROPOSAL_CONFIDENCE: f64 = 0.3;

// ─── Rules data ──────────────────────────────────────────────────────────────

/// A build step the answers are needed before.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub(crate) struct Step {
    pub id: String,
    pub order: u32,
    pub label: String,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
struct TopicStep {
    starts: String,
    step: String,
}

/// One requirement of a kind of object.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub(crate) struct Requirement {
    pub id: String,
    /// The object in words (`luminaire`).
    pub object: String,
    pub trade: String,
    /// The element kinds it applies to.
    pub kinds: Vec<String>,
    /// A device's own kinds (its `kind` property) it applies to; empty: every one of `kinds`.
    pub device_kinds: Vec<String>,
    /// Equipment it applies to by the kind its schedule gives (the spec's equipment by tag).
    pub equipment_kinds: Vec<String>,
    /// Or by the electrical data its schedule gives (any of these keys).
    pub with_keys: Vec<String>,
    /// Never an object with any of these keys (a panel's bus is not a load).
    pub except_keys: Vec<String>,
    /// The properties that meet it (any one).
    pub needs: Vec<String>,
    /// A feeder of the spec landing on the object meets it too.
    pub or_feeder: bool,
    /// What it needs, in words.
    pub what: String,
    /// The code section or trade rule behind it.
    pub basis: String,
    /// The question an object that doesn't meet it asks.
    pub ask: String,
    /// The build step the answer is needed before.
    pub step: String,
    pub severity: String,
    /// How the build completes it meanwhile: `neighbour`, `spare_circuit`, `room_loads`, `none`.
    pub propose: String,
    /// The properties a neighbour's proposal copies (default: `needs`).
    pub propose_keys: Vec<String>,
    /// How far a neighbour may be (feet).
    pub reach_ft: f64,
    /// A build step that measures the requirement on its own objects (`breakers`: the plans'
    /// circuits against the panel schedules' rows), and the kinds of its findings this one is.
    pub from_build: String,
    pub build_kinds: Vec<String>,
}

impl Requirement {
    fn severity(&self) -> Severity {
        Severity::parse(&self.severity).unwrap_or(Severity::Medium)
    }

    fn propose_keys(&self) -> &[String] {
        if self.propose_keys.is_empty() { &self.needs } else { &self.propose_keys }
    }
}

/// The requirements and the build steps.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub(crate) struct Rules {
    documented_sources: Vec<String>,
    empty_values: Vec<String>,
    pub steps: Vec<Step>,
    default_step: String,
    /// The properties a panel's schedule gives (a panel to put a new circuit on).
    panel_keys: Vec<String>,
    kind_steps: BTreeMap<String, String>,
    topic_steps: Vec<TopicStep>,
    pub requirements: Vec<Requirement>,
}

/// The rules data (read once; unreadable data is no rules, never a panic: a test reads it).
pub(crate) fn rules() -> &'static Rules {
    static RULES_DATA: OnceLock<Rules> = OnceLock::new();
    RULES_DATA.get_or_init(|| serde_json::from_str(RULES).unwrap_or_default())
}

impl Rules {
    /// A value that says nothing (`none found`, `?`).
    fn empty(&self, v: &str) -> bool {
        let t = v.trim();
        t.is_empty() || self.empty_values.iter().any(|e| e.eq_ignore_ascii_case(t))
    }

    /// A value from the documents (or computed from them), not assumed.
    fn documented(&self, src: SourceKind) -> bool {
        self.documented_sources.iter().any(|s| s.eq_ignore_ascii_case(src.as_str()))
    }

    /// The step an item must be answered before: its own, else its topic's, else its kind's.
    pub(crate) fn step_of(&self, r: &Rfi) -> &str {
        if !r.before.is_empty()
            && let Some(s) = self.steps.iter().find(|s| s.id == r.before)
        {
            return &s.id;
        }
        let topic = r.topic.to_ascii_lowercase();
        if !topic.is_empty()
            && let Some(t) = self.topic_steps.iter().find(|t| topic.starts_with(&t.starts.to_ascii_lowercase()))
        {
            return &t.step;
        }
        self.kind_steps.get(r.kind_name()).map_or(self.default_step.as_str(), String::as_str)
    }

    /// A step's place in the build (unknown steps last).
    pub(crate) fn step_order(&self, id: &str) -> u32 {
        self.steps.iter().find(|s| s.id == id).map_or(u32::MAX, |s| s.order)
    }

    fn step_label(&self, id: &str) -> String {
        self.steps.iter().find(|s| s.id == id).map_or_else(|| id.replace('_', " "), |s| s.label.clone())
    }
}

// ─── What the model's objects meet ───────────────────────────────────────────

/// Where one object stands on one requirement.
#[derive(Clone, Debug, PartialEq)]
enum Met {
    /// A document gives it.
    Documented,
    /// The build assumed it (a rule's or a default's value): still open, the value its proposal.
    ByRule(String, String),
    Missing,
}

/// One object a requirement leaves open.
#[derive(Clone, Debug)]
struct Open {
    handle: Handle,
    label: String,
    level: String,
    room: String,
    at: [f64; 2],
    lo: [f64; 2],
    hi: [f64; 2],
    sheet: String,
    /// What the build proposes for it, and the key and value it would set (none: only words).
    proposal: String,
    set: Option<(String, String)>,
    /// The build assumed its value by a rule (else no value at all).
    assumed: bool,
}

/// One requirement's measure.
#[derive(Clone, Debug, Default)]
pub(crate) struct Outcome {
    pub id: String,
    pub object: String,
    pub trade: String,
    pub objects: usize,
    pub documented: usize,
    pub by_rule: usize,
    pub missing: usize,
    open: Vec<Open>,
}

impl Outcome {
    fn json(&self, raised: usize, basis: &str) -> Value {
        let status = if self.objects == 0 { format!("did not run: no {} in the model", self.object) } else { "ran".to_string() };
        json!({
            "id": self.id, "object": self.object, "trade": self.trade, "ran": self.objects > 0, "status": status,
            "objects": self.objects, "met_by_documents": self.documented, "completed_by_rule": self.by_rule, "missing": self.missing,
            "open_objects": self.by_rule + self.missing, "findings": raised, "basis": basis,
        })
    }
}

/// What the requirements found on a model: each requirement's measure, the RFIs, and the
/// proposals to write on the objects.
#[derive(Clone, Debug, Default)]
pub(crate) struct Evaluation {
    pub outcomes: Vec<Outcome>,
    pub rfis: Vec<Rfi>,
    /// Requirement RFIs beyond [`MAX_RAISED`], counted.
    pub not_raised: usize,
    writes: Vec<(Handle, Vec<(String, Prop)>)>,
    /// The objects an earlier pass marked open (their marks go before the new ones are set).
    marked: Vec<Handle>,
    /// The schedules' arithmetic (feeder rows, calculation counts) and the corrections it makes.
    schedules: Option<super::loadcalc::arith::Report>,
}

fn lower(v: &[String]) -> Vec<String> {
    v.iter().map(|x| x.trim().to_ascii_lowercase()).collect()
}

/// The schedule's kind of each equipment tag, and the property names its schedule gives.
fn equipment_index(sp: Option<&BuildingSpec>) -> HashMap<String, (String, BTreeSet<String>)> {
    let mut out: HashMap<String, (String, BTreeSet<String>)> = HashMap::new();
    for e in sp.into_iter().flat_map(|sp| sp.mep.equipment.iter()).take(MAX_OBJECTS) {
        let tag = e.tag.trim().to_ascii_uppercase();
        if tag.is_empty() {
            continue;
        }
        let entry = out.entry(tag).or_insert_with(|| (e.kind.to_ascii_lowercase(), BTreeSet::new()));
        entry.1.extend(e.properties.keys().map(|k| k.trim().to_ascii_lowercase()));
    }
    out
}

/// The gear the spec's feeders land on, upper case: the feeder schedule's, and each gear down a
/// feeder run's chain (`feeder 12 P1→P2→P3`: the panels one feeder loops through).
fn fed_gear(sp: Option<&BuildingSpec>) -> BTreeSet<String> {
    let mut out: BTreeSet<String> =
        sp.into_iter().flat_map(|sp| sp.mep.feeders.iter()).map(|f| f.to.trim().to_ascii_uppercase()).filter(|t| !t.is_empty()).collect();
    let feeders = sp.into_iter().flat_map(|sp| sp.mep.runs.iter()).filter(|r| r.system.eq_ignore_ascii_case(super::gearbuild::FEEDER_SYSTEM));
    for r in feeders.take(MAX_OBJECTS) {
        for g in r.source_sheet.split('\u{2192}').skip(1) {
            let t = g.trim().to_ascii_uppercase();
            if !t.is_empty() {
                out.insert(t);
            }
        }
    }
    out
}

/// A device's own kind (its `kind` property), lower case.
fn device_kind(el: &El) -> String {
    el.text("kind").unwrap_or_default().to_ascii_lowercase()
}

/// Whether a requirement applies to an object.
fn applies(r: &Requirement, el: &El, equipment: &HashMap<String, (String, BTreeSet<String>)>) -> bool {
    if !r.kinds.iter().any(|k| k.eq_ignore_ascii_case(&el.kind)) {
        return false;
    }
    if !r.device_kinds.is_empty() && !lower(&r.device_kinds).contains(&device_kind(el)) {
        return false;
    }
    let sched = el.tag().and_then(|t| equipment.get(&t.trim().to_ascii_uppercase()));
    let has = |keys: &[String]| {
        keys.iter().any(|k| el.get(k).is_some() || sched.is_some_and(|(_, ks)| ks.iter().any(|x| x.contains(&k.to_ascii_lowercase()))))
    };
    if has(&r.except_keys) {
        return false;
    }
    if r.equipment_kinds.is_empty() && r.with_keys.is_empty() {
        return true;
    }
    let by_kind = sched.is_some_and(|(k, _)| lower(&r.equipment_kinds).contains(k));
    by_kind || has(&r.with_keys)
}

/// Where an object stands on a requirement.
fn met(rules: &Rules, r: &Requirement, el: &El, fed: &BTreeSet<String>) -> Met {
    let mut assumed: Option<(String, String)> = None;
    for key in &r.needs {
        let Some(p) = el.get(key) else { continue };
        let v = p.text();
        if rules.empty(&v) {
            continue;
        }
        if rules.documented(p.source.kind) {
            return Met::Documented;
        }
        if assumed.is_none() {
            assumed = Some((key.clone(), v.trim().to_string()));
        }
    }
    if r.or_feeder
        && let Some(t) = el.tag()
    {
        let up = t.trim().to_ascii_uppercase();
        // A unit's copy of a typical panel (`P-1A`) is fed as its typical panel (`P`) is.
        let base = up.split(['-', ' ']).next().unwrap_or("").to_string();
        if fed.contains(&up) || (!base.is_empty() && fed.contains(&base)) {
            return Met::Documented;
        }
    }
    assumed.map_or(Met::Missing, |(k, v)| Met::ByRule(k, v))
}

fn plan_at(el: &El) -> [f64; 2] {
    let c = el.center();
    [c[0], c[1]]
}

fn dist(a: [f64; 2], b: [f64; 2]) -> f64 {
    (a[0] - b[0]).hypot(a[1] - b[1])
}

fn same_level(a: &str, b: &str) -> bool {
    a.trim().eq_ignore_ascii_case(b.trim())
}

/// A documented value of the first of `keys` an object has.
fn documented_value(rules: &Rules, el: &El, keys: &[String]) -> Option<(String, String)> {
    keys.iter().find_map(|k| {
        let p = el.get(k)?;
        let v = p.text();
        (!rules.empty(&v) && rules.documented(p.source.kind)).then(|| (k.clone(), v.trim().to_string()))
    })
}

/// How the build completes an object a requirement leaves open: its words and the property it
/// would set.
fn propose(rules: &Rules, r: &Requirement, m: &Model, idx: &[usize], i: usize, el: &El, upf: f64) -> (String, Option<(String, String)>) {
    let at = plan_at(el);
    let level = el.level();
    let room = el.text("room").unwrap_or_default();
    match r.propose.as_str() {
        "neighbour" => {
            let reach = r.reach_ft.max(0.0) * upf;
            // The nearest object of its kind that the documents complete: its room's first.
            let best = idx
                .iter()
                .filter(|j| **j != i)
                .filter_map(|j| m.get(*j))
                .filter(|o| same_level(&o.level(), &level))
                .filter_map(|o| documented_value(rules, o, r.propose_keys()).map(|v| (o, v)))
                .map(|(o, v)| {
                    let in_room = !room.is_empty() && o.text("room").is_some_and(|x| x.eq_ignore_ascii_case(&room));
                    (o, v, in_room, dist(plan_at(o), at))
                })
                .filter(|(_, _, in_room, d)| *in_room || *d <= reach)
                .min_by(|a, b| b.2.cmp(&a.2).then(a.3.total_cmp(&b.3)));
            match best {
                Some((o, (k, v), in_room, d)) => {
                    let whose =
                        if in_room { format!("its room's {}", r.object) } else { format!("the nearest {} ({:.0} ft)", r.object, d / upf.max(1e-9)) };
                    (
                        format!("{} {v}, as {whose} {} has from the documents (trade practice: the neighbour's)", k.replace('_', " "), o.label()),
                        Some((k, v)),
                    )
                }
                None => ("none: no object of its kind near it has one from the documents".to_string(), None),
            }
        }
        "spare_circuit" => {
            let panel = m
                .els
                .iter()
                // A panel by its schedule (its bus, main, breakers), never the load itself, and
                // never another unit's own panel (its typical's copy named for its room: `P-12`
                // in room 12), as E-UNCIRCUITED has it.
                .filter(|o| o.handle() != el.handle() && rules.panel_keys.iter().any(|k| o.get(k).is_some()))
                .filter(|o| o.tag().is_some())
                .filter(|o| {
                    let theirs = o.text("room").unwrap_or_default();
                    let unit =
                        !theirs.is_empty() && o.tag().is_some_and(|t| t.to_ascii_uppercase().ends_with(&format!("-{}", theirs.to_ascii_uppercase())));
                    !unit || theirs.eq_ignore_ascii_case(&room)
                })
                .min_by(|a, b| {
                    let la = !same_level(&a.level(), &level);
                    let lb = !same_level(&b.level(), &level);
                    la.cmp(&lb).then(dist(plan_at(a), at).total_cmp(&dist(plan_at(b), at)))
                });
            match panel.and_then(|p| p.tag()) {
                Some(p) => (
                    format!(
                        "a new circuit on a spare of panel {p} (the nearest panel{}), sized from its nameplate, its disconnect within sight (trade practice E-UNCIRCUITED; NEC 440.14, 430.102)",
                        if panel.is_some_and(|x| same_level(&x.level(), &level)) { " of its level" } else { "" }
                    ),
                    None,
                ),
                None => ("none: no panel in the model to take it".to_string(), None),
            }
        }
        "room_loads" => {
            if room.is_empty() {
                return ("none: it stands in no room".to_string(), None);
            }
            let n = m
                .els
                .iter()
                .filter(|o| o.kind == "light_fixture" && same_level(&o.level(), &level))
                .filter(|o| o.text("room").is_some_and(|x| x.eq_ignore_ascii_case(&room)))
                .count();
            if n == 0 {
                ("none: its room has no luminaire".to_string(), None)
            } else {
                (format!("the {n} luminaire(s) of its room {room} (trade practice: a room's switch controls its room's lights)"), None)
            }
        }
        _ => ("none: an answer is needed".to_string(), None),
    }
}

/// A short stable key of a requirement in a space (an RFI id that survives a rebuild).
fn stable_id(req: &str, level: &str, room: &str) -> String {
    // FNV-1a: the same requirement in the same space keeps its id (and its answer).
    let mut h: u32 = 0x811c_9dc5;
    for b in format!("{req}|{}|{}", level.trim().to_ascii_uppercase(), room.trim().to_ascii_uppercase()).bytes() {
        h ^= u32::from(b);
        h = h.wrapping_mul(0x0100_0193);
    }
    format!("RFI-R{h:08x}")
}

/// The RFI for one requirement's open objects in one space.
fn rfi_for(rules: &Rules, r: &Requirement, level: &str, room: &str, open: &[&Open], by_rule: usize) -> Rfi {
    let n = open.len();
    let place = if room.is_empty() { format!("on {level}") } else { format!("in room {room} on {level}") };
    let names: Vec<String> = open.iter().take(MAX_NAMED).map(|o| o.label.clone()).collect();
    let more = n.saturating_sub(MAX_NAMED);
    let named = if more > 0 { format!("{} and {more} more", names.join(", ")) } else { names.join(", ") };
    let sheets: Vec<String> = open.iter().map(|o| o.sheet.clone()).filter(|s| !s.is_empty()).collect::<BTreeSet<_>>().into_iter().collect();
    let shown = if sheets.is_empty() { "the drawings".to_string() } else { sheets.join(", ") };
    let issue = if by_rule == n {
        format!("{n} {} {place} ({named}): no document gives {}; the build assumed one by a rule", r.object, r.what)
    } else {
        format!("{n} {} {place} ({named}): no sheet, schedule or submittal gives {}", r.object, r.what)
    };
    let proposals: Vec<String> = open.iter().map(|o| o.proposal.clone()).collect::<BTreeSet<_>>().into_iter().take(3).collect();
    let assumed = format!("Proposed meanwhile (unsure, built so the takeoff carries it): {}", proposals.join("; "));
    let mut lo = [f64::INFINITY; 2];
    let mut hi = [f64::NEG_INFINITY; 2];
    for o in open {
        for k in 0..2 {
            if let (Some(l), Some(h), Some(a), Some(b)) = (lo.get_mut(k), hi.get_mut(k), o.lo.get(k), o.hi.get(k)) {
                *l = l.min(*a);
                *h = h.max(*b);
            }
        }
    }
    let extent = lo.iter().chain(hi.iter()).all(|v| v.is_finite()).then_some([lo, hi]);
    let mut rfi =
        Rfi::new(RfiKind::Requirement, level, open.first().map_or([0.0, 0.0], |o| o.at), format!("{n} {} {place}", r.object), issue, assumed);
    rfi.id = stable_id(&r.id, level, room);
    rfi.severity = r.severity();
    rfi.extent = extent;
    rfi.sheets = sheets;
    rfi.topic = format!("requirement: {} ({})", r.id, r.object);
    rfi.proposed = Some(format!("{} {}", r.ask, proposals.first().map(|p| format!("Proposed: {p}.")).unwrap_or_default()).trim().to_string());
    rfi.sides = vec![
        RfiSide { document: shown, says: format!("shows {n} {} {place} and does not give {}", r.object, r.what) },
        RfiSide { document: r.basis.split(':').next().unwrap_or("").trim().to_string(), says: r.basis.clone() },
    ];
    rfi.elements = open.iter().take(cadaddy_walk3d::rfi::MAX_RFI_ELEMENTS).map(|o| o.handle.hex()).collect();
    rfi.before = if rules.steps.iter().any(|s| s.id == r.step) { r.step.clone() } else { rules.default_step.clone() };
    rfi
}

/// A requirement a build step measures on its own objects: the breakers step's plans against
/// schedules (a circuit on the plans no schedule row has, a row nothing on the plans is on, a
/// label naming a panel no document has). Each finding of its kinds is one RFI on its objects;
/// the objects counted are the step's own (the devices on circuits, or the breakers).
fn from_build(rules: &Rules, r: &Requirement, report: Option<&super::breakers::Report>, ev: &mut Evaluation) {
    let mut out = Outcome { id: r.id.clone(), object: r.object.clone(), trade: r.trade.clone(), ..Outcome::default() };
    let Some(rep) = report.filter(|_| r.from_build == "breakers") else {
        ev.outcomes.push(out);
        return;
    };
    let kinds = lower(&r.build_kinds);
    let breaker_side = kinds.iter().any(|k| k == "row_no_plan");
    // The objects the step looked at: its breakers, its panels' schedules, or the devices on
    // the plans' circuits.
    out.objects = if breaker_side {
        rep.breakers + rep.spares
    } else if kinds.iter().any(|k| k == "no_panel") {
        rep.panels.len() + rep.schedules_without_panel.len()
    } else {
        rep.devices_linked + rep.devices_unlinked
    };
    for m in rep.mismatches.iter().filter(|m| kinds.contains(&m.kind.to_ascii_lowercase())) {
        out.missing += if breaker_side { 1 } else { m.devices.max(1) };
        if ev.rfis.len() >= MAX_RAISED {
            ev.not_raised += 1;
            continue;
        }
        let place = format!("panel {} circuit {}", m.panel, m.circuit);
        let mut rfi = Rfi::new(RfiKind::Requirement, &m.level, [0.0, 0.0], place.clone(), m.title(), String::new());
        rfi.id = stable_id(&r.id, &m.level, &format!("{}|{}|{}", m.kind, m.panel, m.circuit));
        rfi.severity = r.severity();
        rfi.topic = format!("requirement: {} ({})", r.id, r.object);
        rfi.assumed =
            "Built as the plans show it meanwhile (the schedule's row, or the plans' circuit, kept as printed beside the other)".to_string();
        rfi.proposed = Some(r.ask.clone());
        if !m.sheet.is_empty() {
            rfi.sheets = vec![m.sheet.clone()];
        }
        rfi.sides = vec![
            RfiSide { document: if m.sheet.is_empty() { "the panel schedules".to_string() } else { m.sheet.clone() }, says: m.title() },
            RfiSide { document: r.basis.split(':').next().unwrap_or("").trim().to_string(), says: r.basis.clone() },
        ];
        rfi.elements = m.elements.iter().take(cadaddy_walk3d::rfi::MAX_RFI_ELEMENTS).cloned().collect();
        rfi.before = if rules.steps.iter().any(|s| s.id == r.step) { r.step.clone() } else { rules.default_step.clone() };
        ev.rfis.push(rfi);
    }
    out.missing = out.missing.min(out.objects);
    out.documented = out.objects - out.missing;
    ev.outcomes.push(out);
}

/// A requirement the schedules' own arithmetic measures (`schedules`: a feeder row against the
/// panel it feeds, a load calculation's per-unit count against its total and the plans). Each
/// mismatch of its kinds is one RFI, with the correction the field makes recorded beside the
/// printed value ([`super::loadcalc::arith::correct`]).
fn from_schedules(rules: &Rules, r: &Requirement, report: Option<&super::loadcalc::arith::Report>, ev: &mut Evaluation) {
    let mut out = Outcome { id: r.id.clone(), object: r.object.clone(), trade: r.trade.clone(), ..Outcome::default() };
    let Some(rep) = report else {
        ev.outcomes.push(out);
        return;
    };
    let kinds = lower(&r.build_kinds);
    out.objects = if kinds.iter().any(|k| k == "feeder_row") { rep.feeder_rows } else { rep.count_lines };
    for m in rep.mismatches.iter().filter(|m| kinds.iter().any(|k| k == m.kind)) {
        out.missing += 1;
        if ev.rfis.len() >= MAX_RAISED {
            ev.not_raised += 1;
            continue;
        }
        let place = if m.kind == "feeder_row" {
            format!("panel {} circuit {}", m.panel, m.item)
        } else if m.panel.is_empty() {
            format!("load calculation line '{}'", m.item)
        } else {
            format!("panel {} load calculation line '{}'", m.panel, m.item)
        };
        let mut rfi = Rfi::new(RfiKind::Requirement, "", [0.0, 0.0], place, m.message.clone(), format!("Corrected for the field: {}", m.correction));
        rfi.id = stable_id(&r.id, "", &format!("{}|{}|{}", m.kind, m.panel, m.item));
        rfi.severity = r.severity();
        rfi.topic = format!("requirement: {} ({})", r.id, r.object);
        rfi.proposed = Some(r.ask.clone());
        rfi.sheets = m.sheets.clone();
        let doc = if m.sheets.is_empty() { "the panel schedules".to_string() } else { m.sheets.join(", ") };
        rfi.sides = vec![
            RfiSide { document: doc, says: m.message.clone() },
            RfiSide { document: r.basis.split(':').next().unwrap_or("").trim().to_string(), says: m.cite.clone() },
        ];
        rfi.before = if rules.steps.iter().any(|s| s.id == r.step) { r.step.clone() } else { rules.default_step.clone() };
        ev.rfis.push(rfi);
    }
    out.missing = out.missing.min(out.objects);
    out.documented = out.objects - out.missing;
    ev.outcomes.push(out);
}

/// Every requirement over the model's objects: the measure, the RFIs (one per requirement and
/// space) and the proposals.
pub(crate) fn evaluate(d: &Drawing, sp: Option<&BuildingSpec>) -> Evaluation {
    let rules = rules();
    let kinds: BTreeSet<String> = rules.requirements.iter().filter(|r| r.from_build.is_empty()).flat_map(|r| lower(&r.kinds)).collect();
    let m = Model::of(d, |k| kinds.contains(&k.to_ascii_lowercase()));
    let upf = if m.upf.is_finite() && m.upf > 0.0 { m.upf } else { 12.0 };
    let equipment = equipment_index(sp);
    let fed = fed_gear(sp);
    let mut ev = Evaluation {
        marked: m.els.iter().filter(|el| el.props.iter().any(|(k, _, _)| k.starts_with(OPEN_PREFIX))).map(El::handle).take(MAX_OBJECTS).collect(),
        ..Evaluation::default()
    };
    let breakers = super::breakers::stored(d);
    if rules.requirements.iter().any(|r| r.from_build == "schedules") {
        ev.schedules = sp.map(|sp| super::loadcalc::arith::check(d, sp));
    }
    for r in &rules.requirements {
        if r.from_build == "schedules" {
            let rep = ev.schedules.take();
            from_schedules(rules, r, rep.as_ref(), &mut ev);
            ev.schedules = rep;
            continue;
        }
        if !r.from_build.is_empty() {
            from_build(rules, r, breakers.as_ref(), &mut ev);
            continue;
        }
        let idx: Vec<usize> = m.els.iter().enumerate().filter(|(_, el)| applies(r, el, &equipment)).map(|(i, _)| i).take(MAX_OBJECTS).collect();
        let mut out = Outcome { id: r.id.clone(), object: r.object.clone(), trade: r.trade.clone(), objects: idx.len(), ..Outcome::default() };
        for &i in &idx {
            let Some(el) = m.get(i) else { continue };
            let state = met(rules, r, el, &fed);
            let (proposal, set) = match &state {
                Met::Documented => {
                    out.documented += 1;
                    continue;
                }
                Met::ByRule(k, v) => {
                    out.by_rule += 1;
                    let note = el.get(k).and_then(|p| p.note.clone()).unwrap_or_default();
                    (
                        format!(
                            "{} {v} as the build assumed it{}",
                            k.replace('_', " "),
                            if note.is_empty() { String::new() } else { format!(" ({note})") }
                        ),
                        None,
                    )
                }
                Met::Missing => {
                    out.missing += 1;
                    propose(rules, r, &m, &idx, i, el, upf)
                }
            };
            let assumed = matches!(state, Met::ByRule(..));
            let pl = el.plan();
            out.open.push(Open {
                handle: el.handle(),
                label: el.label(),
                level: el.level(),
                room: el.text("room").unwrap_or_default(),
                at: plan_at(el),
                lo: [pl.min.x, pl.min.y],
                hi: [pl.max.x, pl.max.y],
                sheet: el.text("sheet").unwrap_or_default(),
                proposal,
                set,
                assumed,
            });
        }
        // One RFI per requirement and space; the objects carry the proposal and the open mark.
        let mut groups: BTreeMap<(String, String), Vec<&Open>> = BTreeMap::new();
        for o in &out.open {
            groups.entry((o.level.clone(), o.room.clone())).or_default().push(o);
        }
        for ((level, room), open) in groups {
            if ev.rfis.len() >= MAX_RAISED {
                ev.not_raised += 1;
                continue;
            }
            let by_rule = open.iter().filter(|o| o.assumed).count();
            let rfi = rfi_for(rules, r, &level, &room, &open, by_rule);
            let src = Source { kind: SourceKind::Rule, doc: Some(format!("requirement {} ({})", r.id, rfi.id)), at: None };
            for o in &open {
                let mut w: Vec<(String, Prop)> = Vec::new();
                let mut mark = Prop::new(json!(format!("{}: {} {}", rfi.id, r.ask, o.proposal)), src.clone());
                mark.note = Some(r.basis.clone());
                w.push((format!("{OPEN_PREFIX}{}", r.id), mark));
                if let Some((k, v)) = &o.set {
                    let mut p = Prop::new(json!(v), src.clone());
                    p.confidence = Some(PROPOSAL_CONFIDENCE);
                    p.note = Some(format!("proposed, unsure: {}; {} asks to confirm", o.proposal, rfi.id));
                    w.push((k.clone(), p));
                }
                ev.writes.push((o.handle, w));
            }
            ev.rfis.push(rfi);
        }
        ev.outcomes.push(out);
    }
    ev
}

/// Write an evaluation into the model: each open object's proposal and mark (a proposal never
/// overwrites a value; a mark is replaced), and the RFIs into the spec (an item answered keeps
/// its answer; an open item no longer raised goes).
pub(crate) fn apply(s: &mut Session, ev: &Evaluation) -> Result<usize> {
    let mut written = 0usize;
    {
        let d = s.doc_mut()?;
        // The marks of an earlier pass go first: an object completed since carries none.
        for h in &ev.marked {
            let _ = d.modify_entity(*h, |e| {
                let mut ps = props::read(&e.common.xdata);
                let keys: Vec<String> = ps.iter().map(|(k, _)| k.to_string()).filter(|k| k.starts_with(OPEN_PREFIX)).collect();
                for k in keys {
                    ps.remove(&k);
                }
                e.common.xdata = props::write(&e.common.xdata, &ps).into();
            });
        }
        for (h, w) in &ev.writes {
            let _ = d.modify_entity(*h, |e| {
                let mut ps = props::read(&e.common.xdata);
                for (k, p) in w {
                    let holds = ps.get(k).is_some_and(Prop::has_value);
                    if k.starts_with(OPEN_PREFIX) || !holds {
                        ps.set(k, p.clone());
                        written += 1;
                    }
                }
                e.common.xdata = props::write(&e.common.xdata, &ps).into();
            });
        }
    }
    let (mut sp, stored) = super::bspec::current(s, "requirements")?;
    if !stored {
        return Ok(written);
    }
    // The schedules' corrections, each beside the printed value.
    if let Some(rep) = &ev.schedules {
        written += super::loadcalc::arith::correct(&mut sp, rep);
    }
    let raised: BTreeSet<&str> = ev.rfis.iter().map(|r| r.id.as_str()).collect();
    sp.rfis.retain(|x| x.kind != Some(RfiKind::Requirement) || x.status != RfiStatus::Open || raised.contains(x.id.as_str()));
    for r in &ev.rfis {
        let n = sp.rfis.len();
        match sp.rfis.iter_mut().find(|x| x.id == r.id) {
            Some(x) if x.status == RfiStatus::Open => *x = r.clone(),
            Some(_) => {}
            None if n < cadaddy_walk3d::rfi::MAX_RFIS => sp.rfis.push(r.clone()),
            None => {}
        }
    }
    s.doc_mut()?.building_spec = Some(sp.to_json()).into();
    Ok(written)
}

// ─── Reports ─────────────────────────────────────────────────────────────────

/// The requirements' measure: each requirement's counts (with how many RFIs it raised), and the
/// totals.
pub(crate) fn summary(ev: &Evaluation) -> Value {
    let rules = rules();
    let checks: Vec<Value> = ev
        .outcomes
        .iter()
        .map(|o| {
            let raised = ev.rfis.iter().filter(|r| r.topic.starts_with(&format!("requirement: {} ", o.id))).count();
            let basis = rules.requirements.iter().find(|r| r.id == o.id).map_or("", |r| r.basis.as_str());
            o.json(raised, basis)
        })
        .collect();
    let ran = ev.outcomes.iter().filter(|o| o.objects > 0).count();
    let objects: usize = ev.outcomes.iter().map(|o| o.objects).sum();
    let open: usize = ev.outcomes.iter().map(|o| o.by_rule + o.missing).sum();
    json!({
        "requirements": ev.outcomes.len(), "ran": ran, "did_not_run": ev.outcomes.len() - ran,
        "objects_checked": objects, "open_objects": open, "findings": ev.rfis.len(), "not_raised": ev.not_raised,
        "checks": checks,
    })
}

/// The documents' pass, before anything is built: the read devices against the requirements a
/// device's own reading can meet (its circuit). Measured like the model's pass; it raises no RFI
/// (the build raises them on the objects it makes).
pub(crate) fn documents_pass(devices: &[super::readers::ReadDevice]) -> Value {
    let rules = rules();
    let luminaire = |k: &str| {
        let k = k.to_ascii_lowercase();
        (k.contains("luminaire") || k.contains("light")) && !["switch", "sensor", "control", "dimmer"].iter().any(|w| k.contains(w))
    };
    let mut checks: Vec<Value> = Vec::new();
    for r in rules.requirements.iter().filter(|r| r.needs.iter().any(|n| n == "circuit")) {
        let kinds = lower(&r.kinds);
        let dks = lower(&r.device_kinds);
        let mut objects = 0usize;
        let mut missing = 0usize;
        let mut sample: Vec<String> = Vec::new();
        for d in devices.iter().take(MAX_OBJECTS) {
            let kind = if d.kind.trim().is_empty() { super::readers::device_kind(&d.tag).to_string() } else { d.kind.to_ascii_lowercase() };
            let fits = if luminaire(&kind) {
                kinds.iter().any(|k| k == "light_fixture")
            } else {
                kinds.iter().any(|k| k == "device") && dks.contains(&kind)
            };
            if !fits {
                continue;
            }
            objects += 1;
            if rules.empty(&d.circuit) {
                missing += 1;
                if sample.len() < 5 {
                    sample.push(format!("{} {} on {} ({})", kind, d.tag, if d.level.is_empty() { "?" } else { &d.level }, d.sheet));
                }
            }
        }
        let status = if objects == 0 { format!("did not run: no {} read", r.object) } else { "ran".to_string() };
        checks.push(
            json!({"id": r.id, "object": r.object, "ran": objects > 0, "status": status, "objects": objects, "missing": missing, "sample": sample}),
        );
    }
    let ran = checks.iter().filter(|c| c["ran"].as_bool() == Some(true)).count();
    let missing: u64 = checks.iter().filter_map(|c| c["missing"].as_u64()).sum();
    json!({"pass": "documents", "ran": ran, "checks": checks, "objects_missing": missing,
        "note": "the devices as read off the sheets, before the model is built: the build raises an RFI on each object it cannot complete"})
}

/// The inconsistencies the model carries, for a report: the headline (`N things to ask before
/// you build: X high`), the open items by the build step they block, the first items in that
/// order, and the requirements' measure.
pub(crate) fn report(sp: &BuildingSpec, measured: Option<&Value>) -> Value {
    let rules = rules();
    let mut open: Vec<Rfi> = cadaddy_walk3d::rfi::list(sp).into_iter().filter(|r| r.status == RfiStatus::Open).collect();
    sort_by_step(&mut open);
    let high = open.iter().filter(|r| r.severity == Severity::High).count();
    let mut by_step: Vec<Value> = Vec::new();
    for st in &rules.steps {
        let mine: Vec<&Rfi> = open.iter().filter(|r| rules.step_of(r) == st.id).collect();
        let hi = mine.iter().filter(|r| r.severity == Severity::High).count();
        by_step.push(json!({"step": st.id, "label": st.label, "items": mine.len(), "high": hi}));
    }
    let mut by_kind: BTreeMap<String, usize> = BTreeMap::new();
    for r in &open {
        *by_kind.entry(r.kind_name().to_string()).or_default() += 1;
    }
    let items: Vec<Value> = open.iter().take(MAX_LISTED).map(item_json).collect();
    let mut v = json!({
        "headline": headline(open.len(), high),
        "open": open.len(), "high": high, "by_step": by_step, "by_kind": by_kind, "items": items,
        "export": "rfi.export {path: <file>.pdf|.xlsx|.csv} sends them, one numbered item each",
    });
    if let Some(m) = measured {
        v["requirements"] = m.clone();
    }
    v
}

/// The line a report leads with.
pub(crate) fn headline(open: usize, high: usize) -> String {
    format!("{open} {} to ask before you build: {high} high", if open == 1 { "thing" } else { "things" })
}

/// An item for a report or a list: its kind's name and title, and the step it blocks.
pub(crate) fn item_json(r: &Rfi) -> Value {
    let rules = rules();
    let mut v = serde_json::to_value(r).unwrap_or(Value::Null);
    let step = rules.step_of(r).to_string();
    v["kind"] = json!(r.kind_name());
    v["title"] = json!(r.kind.map_or("Other", RfiKind::title));
    v["before_label"] = json!(rules.step_label(&step));
    v["before"] = json!(step);
    v
}

/// Ordered by when it hurts: what blocks underground and the slab first, then rough-in, then
/// trim; the most severe first within a step.
pub(crate) fn sort_by_step(list: &mut [Rfi]) {
    let rules = rules();
    list.sort_by(|a, b| {
        rules
            .step_order(rules.step_of(a))
            .cmp(&rules.step_order(rules.step_of(b)))
            .then_with(|| a.severity.cmp(&b.severity))
            .then_with(|| a.id.cmp(&b.id))
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_requirements_are_rule_data_with_their_bases_and_steps() {
        let r = rules();
        assert!(!r.requirements.is_empty(), "requirements.json reads");
        let steps: BTreeSet<&str> = r.steps.iter().map(|s| s.id.as_str()).collect();
        let mut ids = BTreeSet::new();
        for q in &r.requirements {
            assert!(ids.insert(q.id.as_str()), "{} twice", q.id);
            assert!(!q.basis.trim().is_empty() && !q.ask.trim().is_empty() && !q.what.trim().is_empty(), "{} says why and asks", q.id);
            assert!(steps.contains(q.step.as_str()), "{}: step {}", q.id, q.step);
            assert!(Severity::parse(&q.severity).is_some(), "{}: severity {}", q.id, q.severity);
            assert!(!q.kinds.is_empty() && !q.needs.is_empty(), "{}", q.id);
            assert!(matches!(q.propose.as_str(), "neighbour" | "spare_circuit" | "room_loads" | "none"), "{}: propose {}", q.id, q.propose);
        }
        // Every RFI kind has a step.
        for k in RfiKind::ALL.into_iter().filter(|k| *k != RfiKind::Requirement) {
            assert!(r.kind_steps.contains_key(k.name()), "{} has no step", k.name());
            assert!(steps.contains(r.kind_steps.get(k.name()).map_or("", String::as_str)), "{}", k.name());
        }
        // The electrical kinds the owner named first: luminaire, receptacle, switch, equipment, panel.
        for object in ["luminaire", "receptacle", "switch or control", "equipment connection", "powered equipment", "panel"] {
            assert!(r.requirements.iter().any(|q| q.object == object), "{object}");
        }
    }

    #[test]
    fn items_are_ordered_by_the_step_they_block_then_severity() {
        let mut a = Rfi::new(RfiKind::Lighting, "1", [0.0, 0.0], "a", "cct", "");
        a.topic = "lighting: CCT".into();
        a.id = "A".into();
        let mut b = Rfi::new(RfiKind::PourHold, "1", [0.0, 0.0], "b", "pour", "");
        b.id = "B".into();
        b.severity = Severity::Low;
        let mut c = Rfi::new(RfiKind::Requirement, "1", [0.0, 0.0], "c", "circuit", "");
        c.before = "rough_in".into();
        c.id = "C".into();
        let mut d = Rfi::new(RfiKind::Lighting, "1", [0.0, 0.0], "d", "no control", "");
        d.topic = "lighting: fixture with no control".into();
        d.id = "D".into();
        d.severity = Severity::High;
        let mut list = vec![a, c, d, b];
        sort_by_step(&mut list);
        let ids: Vec<&str> = list.iter().map(|r| r.id.as_str()).collect();
        assert_eq!(ids, ["B", "D", "C", "A"], "underground first, a rough-in control before a medium one, the CCT at trim");
        assert_eq!(rules().step_of(&list[3]), "trim");
    }

    #[test]
    fn a_requirement_in_one_space_keeps_its_id() {
        assert_eq!(stable_id("E-LUM-CIRCUIT", "Level 2", "R-A"), stable_id("E-LUM-CIRCUIT", "level 2 ", "r-a"));
        assert_ne!(stable_id("E-LUM-CIRCUIT", "Level 2", "R-A"), stable_id("E-LUM-CIRCUIT", "Level 2", "R-B"));
        assert_ne!(stable_id("E-LUM-CIRCUIT", "Level 2", "R-A"), stable_id("E-REC-CIRCUIT", "Level 2", "R-A"));
    }
}
