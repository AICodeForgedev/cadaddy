//! What the fire alarm's sequence of operations calls for (Q6647): every output of its matrix
//! (a shunt trip, a damper release, a fan shutdown, a door release, an elevator recall) and every
//! schedule note that ties equipment to the fire alarm needs a device or relay that performs it,
//! and its power, on the drawings.
//!
//! - [`outputs`] reads the outputs off the sequence pages: a page whose texts name a sequence or
//!   matrix; each short text naming an output class ([`class_of`]). On a matrix (marks in a grid)
//!   an output counts only where an input marks its column (a header row, level or turned) or its
//!   row (the matrix printed the other way round); a written sequence (no marks) counts as written.
//! - [`check`] measures each output (and each schedule tie, [`ties`]) against the model: the objects
//!   it acts on ([`targets`]), and for each the device that performs it: a relay or control module
//!   for it (linked by `FOR`, or beside it), or, for a shunt trip, a breaker or switch the
//!   schedules mark shunt trip, with a circuit for its control power. A device the build set by a
//!   rule (inferred, proposed) completes the object but leaves it open.
//! - [`complete`] builds what the field installs where the drawings show none: a fire alarm
//!   control (relay) module beside each object, inferred, with its basis, so the takeoff carries it.
//! - [`actuator_loads`]: each circuit feeding damper actuators carries their load in its schedule.
//!
//! Every word, class and code section is rules data (`famatrix.json`); nothing names a job's
//! sheet, room or tag.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::OnceLock;

use cadaddy_geom::Vec2;
use cadaddy_mepfix::sheet_facts::TextItem;
use cadaddy_walk3d::{BuildingSpec, FaOutput, MepEquipment};
use serde::Deserialize;

const RULES: &str = include_str!("famatrix.json");

/// Texts looked at on one page, at most (hostile input).
const MAX_TEXTS: usize = 200_000;
/// Matrix marks looked at on one page, at most.
const MAX_MARKS: usize = 50_000;
/// Outputs read, at most.
const MAX_OUTPUTS: usize = 500;
/// Equipment looked at, at most.
const MAX_EQUIPMENT: usize = 200_000;
/// Objects one output acts on, at most.
const MAX_TARGETS: usize = 5_000;
/// Words of one text read, at most.
const MAX_WORDS: usize = 200;

// ─── Rules data ──────────────────────────────────────────────────────────────

/// The objects an output acts on when its text holds one of `words` (none: any output of the
/// class): equipment of `kinds`, or whose description holds the words.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub(crate) struct Target {
    pub words: Vec<Vec<String>>,
    pub kinds: Vec<String>,
}

/// A class of fire alarm output.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub(crate) struct Class {
    pub id: String,
    pub name: String,
    /// Word groups: an output of the class holds every word of one group…
    pub words: Vec<Vec<String>>,
    /// …and one of these (none: the words are enough).
    pub actions: Vec<String>,
    pub targets: Vec<Target>,
    /// `target`: each object needs its device; `output`: one device performs it for all.
    pub per: String,
    /// `relay` or `shunt`.
    pub device: String,
    /// Words of an object's own schedule that give it the device (`SHUNT`).
    pub device_words: Vec<Vec<String>>,
    /// Kinds of read devices that perform it (an elevator recall interface).
    pub device_kinds: Vec<String>,
    /// Words of the circuit that gives the device its control power (none: no power asked).
    pub power_words: Vec<Vec<String>>,
    pub severity: String,
    pub basis: String,
    pub ask: String,
    pub proposal: String,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub(crate) struct Tie {
    pub words: Vec<Vec<String>>,
    pub actions: Vec<String>,
    pub class: String,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub(crate) struct ModuleRule {
    pub kind: String,
    pub name: String,
    pub box_in: [f64; 3],
    pub source: String,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub(crate) struct ActuatorLoad {
    pub box_kind: String,
    pub va_keys: Vec<String>,
    pub tolerance: f64,
    pub basis: String,
    pub ask: String,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub(crate) struct Rules {
    pub page_words: Vec<Vec<String>>,
    pub mark_texts: Vec<String>,
    pub min_marks: usize,
    pub max_text_chars: usize,
    pub char_width: f64,
    pub row_reach: f64,
    pub column_reach: f64,
    pub relay_words: Vec<Vec<String>>,
    pub relay_reach_ft: f64,
    pub rule_statuses: Vec<String>,
    pub rule_keys: Vec<String>,
    pub describe_keys: Vec<String>,
    pub skip_keys: Vec<String>,
    pub own_kind_prefixes: Vec<String>,
    pub own_kind_suffixes: Vec<String>,
    pub tie: Tie,
    pub module: ModuleRule,
    pub outputs: Vec<Class>,
    pub actuator_load: ActuatorLoad,
}

/// The rules data (read once; unreadable data is no rules, never a panic: a test reads it).
pub(crate) fn rules() -> &'static Rules {
    static DATA: OnceLock<Rules> = OnceLock::new();
    DATA.get_or_init(|| serde_json::from_str(RULES).unwrap_or_default())
}

fn class_by_id(id: &str) -> Option<&'static Class> {
    rules().outputs.iter().find(|c| c.id == id)
}

// ─── Words ───────────────────────────────────────────────────────────────────

fn words(t: &str) -> Vec<String> {
    t.to_ascii_uppercase().split(|c: char| !c.is_ascii_alphanumeric()).filter(|w| !w.is_empty()).take(MAX_WORDS).map(str::to_string).collect()
}

/// Whether `w` holds every word of one non-empty group.
fn holds_any(w: &[String], groups: &[Vec<String>]) -> bool {
    groups.iter().any(|g| !g.is_empty() && g.iter().all(|x| w.iter().any(|y| y.eq_ignore_ascii_case(x))))
}

fn has_word(w: &[String], list: &[String]) -> bool {
    list.iter().any(|a| w.iter().any(|y| y.eq_ignore_ascii_case(a)))
}

/// Whether `needle`'s words stand together in `w` (a tag `EF-3` named in an output).
fn names(w: &[String], needle: &[String]) -> bool {
    !needle.is_empty() && w.windows(needle.len()).any(|win| win.iter().zip(needle).all(|(a, b)| a == b))
}

/// The output class a text names, if it names one.
pub(crate) fn class_of(text: &str) -> Option<&'static Class> {
    let w = words(text);
    rules().outputs.iter().find(|c| holds_any(&w, &c.words) && (c.actions.is_empty() || has_word(&w, &c.actions)))
}

// ─── Reading the sequence pages ──────────────────────────────────────────────

/// A text's box (sheet units), along its rotation.
fn text_box(t: &TextItem, char_width: f64) -> Option<(Vec2, Vec2)> {
    let h = t.height;
    if !(h.is_finite() && h > 0.0 && t.at.x.is_finite() && t.at.y.is_finite() && t.rotation.is_finite()) {
        return None;
    }
    let len = h * char_width * t.value.trim().chars().count() as f64;
    let u = Vec2::new(t.rotation.cos(), t.rotation.sin());
    let v = Vec2::new(-u.y, u.x);
    let pts = [t.at, t.at + u * len, t.at + v * h, t.at + u * len + v * h];
    let lo = Vec2::new(pts.iter().map(|p| p.x).fold(f64::INFINITY, f64::min), pts.iter().map(|p| p.y).fold(f64::INFINITY, f64::min));
    let hi = Vec2::new(pts.iter().map(|p| p.x).fold(f64::NEG_INFINITY, f64::max), pts.iter().map(|p| p.y).fold(f64::NEG_INFINITY, f64::max));
    Some((lo, hi))
}

/// The marks of a matrix in `t`'s column (below it) or its row (right of it).
fn marked(t: &TextItem, marks: &[Vec2], r: &Rules) -> u32 {
    let Some((lo, hi)) = text_box(t, r.char_width) else { return 0 };
    let h = t.height;
    let n = marks
        .iter()
        .filter(|m| {
            let column = m.x >= lo.x - h * r.column_reach && m.x <= hi.x + h * r.column_reach && m.y < lo.y;
            let row = m.y >= lo.y - h * r.row_reach && m.y <= hi.y + h * r.row_reach && m.x > hi.x;
            column || row
        })
        .count();
    u32::try_from(n).unwrap_or(u32::MAX)
}

/// The fire alarm outputs on `pages` (`(sheet, texts)`): each short text naming an output class
/// on a page whose texts name a sequence or matrix; on a matrix, only the outputs an input marks.
pub(crate) fn outputs(pages: &[(String, Vec<TextItem>)]) -> Vec<FaOutput> {
    let r = rules();
    let mut out: Vec<FaOutput> = Vec::new();
    for (sheet, texts) in pages {
        let texts = texts.get(..texts.len().min(MAX_TEXTS)).unwrap_or_default();
        if !texts.iter().any(|t| holds_any(&words(&t.value), &r.page_words)) {
            continue;
        }
        let marks: Vec<Vec2> = texts
            .iter()
            .filter(|t| r.mark_texts.iter().any(|m| m == t.value.trim()))
            .filter_map(|t| text_box(t, r.char_width).map(|(lo, hi)| (lo + hi) * 0.5))
            .take(MAX_MARKS)
            .collect();
        let matrix = marks.len() >= r.min_marks.max(1);
        for t in texts {
            let value = t.value.trim();
            if value.is_empty() || value.chars().count() > r.max_text_chars {
                continue;
            }
            let Some(c) = class_of(value) else { continue };
            let inputs = if matrix { marked(t, &marks, r) } else { 0 };
            if matrix && inputs == 0 {
                continue;
            }
            let o = FaOutput { class: c.id.clone(), text: value.to_string(), sheet: sheet.clone(), inputs };
            if !out.iter().any(|x| x.class == o.class && x.text.eq_ignore_ascii_case(&o.text) && x.sheet == o.sheet) {
                out.push(o);
            }
            if out.len() >= MAX_OUTPUTS {
                return out;
            }
        }
    }
    out
}

// ─── The model's objects and devices ─────────────────────────────────────────

fn upf(sp: &BuildingSpec) -> f64 {
    if sp.units_per_foot.is_finite() && sp.units_per_foot > 0.0 { sp.units_per_foot } else { 12.0 }
}

/// A device or module of the fire alarm itself (never an object it acts on).
fn own_kind(kind: &str) -> bool {
    let r = rules();
    let k = kind.to_ascii_lowercase();
    r.own_kind_prefixes.iter().any(|p| k.starts_with(p.as_str())) || r.own_kind_suffixes.iter().any(|s| k.ends_with(s.as_str()))
}

/// An equipment's description in words: its kind, tag, title, type mark and the schedule's
/// describing columns.
fn describe(e: &MepEquipment) -> Vec<String> {
    let r = rules();
    let mut t = format!("{} {} {} {}", e.kind.replace('_', " "), e.tag, e.title, e.type_mark);
    for (k, v) in &e.properties {
        let k = k.to_ascii_uppercase();
        if r.describe_keys.iter().any(|d| k.contains(d.as_str())) && !r.skip_keys.iter().any(|s| s.eq_ignore_ascii_case(&k)) {
            t.push(' ');
            t.push_str(v);
        }
    }
    words(&t)
}

fn center(e: &MepEquipment) -> Vec2 {
    Vec2::new((e.min[0] + e.max[0]) / 2.0, (e.min[1] + e.max[1]) / 2.0)
}

fn same_level(a: &str, b: &str) -> bool {
    a.trim().eq_ignore_ascii_case(b.trim())
}

/// Whether the build set it by a rule (inferred, proposed): it completes an object, the
/// documents don't show it.
fn by_rule(e: &MepEquipment) -> bool {
    let r = rules();
    e.properties.iter().any(|(k, v)| {
        (k.eq_ignore_ascii_case("STATUS") && r.rule_statuses.iter().any(|s| s.eq_ignore_ascii_case(v.trim())))
            || r.rule_keys.iter().any(|x| x.eq_ignore_ascii_case(k))
    })
}

/// Whether an equipment is a relay or control module (or a device of `kinds`).
fn is_relay(e: &MepEquipment, kinds: &[String]) -> bool {
    let r = rules();
    kinds.iter().any(|k| k.eq_ignore_ascii_case(&e.kind))
        || e.kind.eq_ignore_ascii_case(&r.module.kind)
        || holds_any(&words(&format!("{} {}", e.kind.replace('_', " "), e.title)), &r.relay_words)
}

/// The objects an output acts on: the equipment its text names by tag, else the equipment of the
/// class's kinds (or whose description holds the target's words).
pub(crate) fn targets(sp: &BuildingSpec, class: &Class, text: &str) -> Vec<usize> {
    let w = words(text);
    let eq = sp.mep.equipment.get(..sp.mep.equipment.len().min(MAX_EQUIPMENT)).unwrap_or_default();
    let named: Vec<usize> = eq
        .iter()
        .enumerate()
        .filter(|(_, e)| !e.tag.trim().is_empty() && !own_kind(&e.kind) && names(&w, &words(&e.tag)))
        .map(|(i, _)| i)
        .take(MAX_TARGETS)
        .collect();
    if !named.is_empty() {
        return named;
    }
    let Some(t) = class.targets.iter().find(|t| t.words.is_empty() || holds_any(&w, &t.words)) else { return Vec::new() };
    eq.iter()
        .enumerate()
        .filter(|(_, e)| {
            let of_kind = t.kinds.iter().any(|k| k.eq_ignore_ascii_case(&e.kind));
            of_kind || (!own_kind(&e.kind) && holds_any(&describe(e), &t.words))
        })
        .map(|(i, _)| i)
        .take(MAX_TARGETS)
        .collect()
}

/// The equipment schedule notes that tie equipment to the fire alarm (`INTERLOCK WITH THE FIRE
/// ALARM CONTROL PANEL`): (equipment, the note).
pub(crate) fn ties(sp: &BuildingSpec) -> Vec<(usize, String)> {
    let r = rules();
    let mut out = Vec::new();
    for (i, e) in sp.mep.equipment.iter().enumerate().take(MAX_EQUIPMENT) {
        if own_kind(&e.kind) || by_rule(e) {
            continue;
        }
        let note = e.properties.iter().filter(|(k, _)| !r.skip_keys.iter().any(|s| s.eq_ignore_ascii_case(k))).find_map(|(_, v)| {
            let w = words(v);
            (holds_any(&w, &r.tie.words) && has_word(&w, &r.tie.actions)).then(|| v.trim().to_string())
        });
        if let Some(n) = note {
            out.push((i, n));
        }
    }
    out
}

/// How an object stands on its device.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Device {
    /// The documents show it (what and where).
    Documented(String),
    /// The build set it by a rule (its tag).
    ByRule(String),
    Missing,
}

/// The relay or module for object `i`: one linked to it (`FOR`), else one beside it on its level
/// that is no other object's; a documented one first.
fn relay_for(sp: &BuildingSpec, i: usize, class: &Class) -> Device {
    let r = rules();
    let Some(t) = sp.mep.equipment.get(i) else { return Device::Missing };
    let reach = r.relay_reach_ft.max(0.0) * upf(sp);
    let mut rule: Option<String> = None;
    for e in sp.mep.equipment.iter().take(MAX_EQUIPMENT).filter(|e| is_relay(e, &class.device_kinds)) {
        let linked = e.properties.iter().find(|(k, _)| k.eq_ignore_ascii_case("FOR")).map(|(_, v)| v.trim());
        let ours = match linked {
            Some(f) => !t.tag.trim().is_empty() && f.eq_ignore_ascii_case(t.tag.trim()),
            None => same_level(&e.level, &t.level) && (center(e) - center(t)).len() <= reach,
        };
        if !ours {
            continue;
        }
        if !by_rule(e) {
            return Device::Documented(format!("{} {}", e.title.trim(), e.tag).trim().to_string());
        }
        rule.get_or_insert_with(|| e.tag.clone());
    }
    let read = sp.mep.devices.iter().take(MAX_EQUIPMENT).find(|d| {
        let relay =
            class.device_kinds.iter().any(|k| k.eq_ignore_ascii_case(&d.kind)) || holds_any(&words(&d.kind.replace('_', " ")), &r.relay_words);
        relay && d.properties.iter().any(|(k, v)| k.eq_ignore_ascii_case("FOR") && !t.tag.is_empty() && v.trim().eq_ignore_ascii_case(t.tag.trim()))
    });
    if let Some(d) = read {
        return Device::Documented(format!("{} {}", d.kind.replace('_', " "), d.tag));
    }
    rule.map_or(Device::Missing, Device::ByRule)
}

/// A shunt trip for objects `ts`: an object's own schedule marking it, else a panel circuit naming
/// the target's words whose breaker the schedule marks shunt trip. With the circuit that gives its
/// control power (another circuit the power words name).
fn shunt_for(sp: &BuildingSpec, ts: &[usize], class: &Class, text: &str) -> (Option<String>, Option<String>) {
    let mut device: Option<String> = None;
    let mut device_row: Option<(String, String)> = None;
    for &i in ts {
        let Some(e) = sp.mep.equipment.get(i) else { continue };
        let own: Vec<String> = e.properties.values().flat_map(|v| words(v)).chain(describe(e)).collect();
        if holds_any(&own, &class.device_words) {
            device = Some(format!("{} {}'s schedule", e.title.trim(), e.tag).trim().to_string());
            break;
        }
    }
    let w = words(text);
    let target_words: Vec<Vec<String>> =
        class.targets.iter().find(|t| t.words.is_empty() || holds_any(&w, &t.words)).map(|t| t.words.clone()).unwrap_or_default();
    let tags: Vec<Vec<String>> = ts.iter().filter_map(|i| sp.mep.equipment.get(*i)).map(|e| words(&e.tag)).filter(|t| !t.is_empty()).collect();
    if device.is_none() {
        'panels: for ps in &sp.mep.panel_schedules {
            for c in &ps.circuits {
                let d = words(&c.description);
                if !(holds_any(&d, &target_words) || tags.iter().any(|t| names(&d, t))) {
                    continue;
                }
                let marks = c.protection.iter().any(|p| {
                    let meaning = ps.legend.get(p.trim()).cloned().unwrap_or_default();
                    holds_any(&words(&format!("{p} {meaning}")), &class.device_words)
                });
                if marks || holds_any(&d, &class.device_words) {
                    device = Some(format!("panel {} circuit {} ({}): shunt-trip breaker", ps.panel, c.number, c.description.trim()));
                    device_row = Some((ps.panel.clone(), c.number.clone()));
                    break 'panels;
                }
            }
        }
    }
    if class.power_words.is_empty() {
        return (device, Some(String::new()));
    }
    let power = sp.mep.panel_schedules.iter().find_map(|ps| {
        ps.circuits.iter().find_map(|c| {
            let row = Some((ps.panel.clone(), c.number.clone()));
            (row != device_row && holds_any(&words(&c.description), &class.power_words))
                .then(|| format!("panel {} circuit {} ({})", ps.panel, c.number, c.description.trim()))
        })
    });
    (device, power)
}

/// One output (or schedule tie) measured against the model.
#[derive(Clone, Debug)]
pub(crate) struct Finding {
    pub class: &'static Class,
    pub text: String,
    /// The sheets that print it.
    pub sheets: Vec<String>,
    /// The objects it acts on (equipment tags) and their levels.
    pub targets: Vec<(String, String)>,
    /// The objects with no device the documents show, and the build's device for each (if any).
    pub open: Vec<(String, String, Option<String>)>,
    /// What performs it where the documents show it.
    pub devices: Vec<String>,
    /// A device asks for control power and no circuit gives it.
    pub no_power: bool,
    /// It acts on nothing the model has.
    pub no_target: bool,
}

impl Finding {
    /// Objects looked at (an output acting on nothing counts as one).
    pub fn objects(&self) -> usize {
        if self.class.per == "output" || self.no_target { 1 } else { self.targets.len() }
    }

    /// Objects left open (documents show no device): (by a rule, missing).
    pub fn open_counts(&self) -> (usize, usize) {
        if self.class.per == "output" || self.no_target {
            let open = self.no_target || !self.open.is_empty() || self.no_power;
            let rule = open && !self.no_target && !self.open.is_empty() && self.open.iter().all(|o| o.2.is_some()) && self.class.device != "shunt";
            return if !open {
                (0, 0)
            } else if rule {
                (1, 0)
            } else {
                (0, 1)
            };
        }
        let rule = self.open.iter().filter(|o| o.2.is_some()).count();
        (rule, self.open.len() - rule)
    }

    pub fn is_open(&self) -> bool {
        let (a, b) = self.open_counts();
        a + b > 0
    }
}

/// Every output the spec holds and every schedule tie, measured (met ones too, for the counts).
pub(crate) fn check(sp: &BuildingSpec) -> Vec<Finding> {
    let mut out: Vec<Finding> = Vec::new();
    // The outputs, one per class and text (the same output on two sheets is one).
    let mut seen: BTreeMap<(String, String), usize> = BTreeMap::new();
    let mut acted: BTreeSet<usize> = BTreeSet::new();
    for o in sp.mep.fa_outputs.iter().take(MAX_OUTPUTS) {
        let Some(class) = class_by_id(&o.class) else { continue };
        let key = (o.class.clone(), o.text.trim().to_ascii_uppercase());
        if let Some(&i) = seen.get(&key) {
            if let Some(f) = out.get_mut(i)
                && !f.sheets.contains(&o.sheet)
            {
                f.sheets.push(o.sheet.clone());
            }
            continue;
        }
        let ts = targets(sp, class, &o.text);
        acted.extend(ts.iter().copied());
        seen.insert(key, out.len());
        out.push(measure(sp, class, &o.text, vec![o.sheet.clone()], &ts));
    }
    // The schedule ties of equipment no output acts on, one per note.
    let mut by_note: BTreeMap<String, (String, Vec<usize>)> = BTreeMap::new();
    for (i, note) in ties(sp) {
        if acted.contains(&i) {
            continue;
        }
        by_note.entry(note.to_ascii_uppercase()).or_insert_with(|| (note.clone(), Vec::new())).1.push(i);
    }
    if let Some(class) = class_by_id(&rules().tie.class) {
        for (_, (note, ts)) in by_note {
            let sheets: Vec<String> = ts
                .iter()
                .filter_map(|i| sp.mep.equipment.get(*i))
                .filter_map(|e| e.sources.values().next().cloned())
                .filter(|s| !s.is_empty())
                .collect::<BTreeSet<_>>()
                .into_iter()
                .take(4)
                .collect();
            out.push(measure(sp, class, &note, sheets, &ts));
        }
    }
    out
}

fn measure(sp: &BuildingSpec, class: &'static Class, text: &str, sheets: Vec<String>, ts: &[usize]) -> Finding {
    let tag_level = |i: &usize| sp.mep.equipment.get(*i).map(|e| (e.tag.clone(), e.level.clone()));
    let targets: Vec<(String, String)> = ts.iter().filter_map(tag_level).collect();
    let mut f =
        Finding { class, text: text.to_string(), sheets, targets, open: Vec::new(), devices: Vec::new(), no_power: false, no_target: ts.is_empty() };
    if class.device == "shunt" {
        let (device, power) = shunt_for(sp, ts, class, text);
        let relay = ts.iter().find_map(|&i| match relay_for(sp, i, class) {
            Device::ByRule(t) => Some(t),
            _ => None,
        });
        match device {
            Some(d) => {
                f.devices.push(d);
                f.no_power = power.is_none();
            }
            None if !ts.is_empty() => {
                f.open = ts.iter().filter_map(tag_level).map(|(t, l)| (t, l, relay.clone())).collect();
            }
            None => {}
        }
        return f;
    }
    let states: Vec<(usize, Device)> = ts.iter().map(|&i| (i, relay_for(sp, i, class))).collect();
    if class.per == "output" && states.iter().any(|(_, d)| matches!(d, Device::Documented(_))) {
        f.devices = states.iter().filter_map(|(_, d)| if let Device::Documented(x) = d { Some(x.clone()) } else { None }).collect();
        return f;
    }
    for (i, d) in states {
        match d {
            Device::Documented(x) => f.devices.push(x),
            Device::ByRule(t) => f.open.extend(tag_level(&i).map(|(a, b)| (a, b, Some(t)))),
            Device::Missing => f.open.extend(tag_level(&i).map(|(a, b)| (a, b, None))),
        }
    }
    f
}

// ─── Completing the building ─────────────────────────────────────────────────

/// What [`complete`] built: the modules' tags, and the outputs it could set none for (they act on
/// nothing the model has).
#[derive(Clone, Debug, Default)]
pub(crate) struct Completed {
    pub modules: Vec<String>,
    pub unplaced: Vec<String>,
}

/// The field's work where the drawings show none: a fire alarm control (relay) module beside each
/// object an output acts on with no relay for it (one per output for a shunt trip or a recall),
/// inferred, with its basis. The modules of an earlier pass go first.
pub(crate) fn complete(sp: &mut BuildingSpec) -> Completed {
    let r = rules();
    let mine = |e: &MepEquipment| e.kind == r.module.kind && e.properties.contains_key("FA_OUTPUT");
    sp.mep.equipment.retain(|e| !mine(e));
    let mut done = Completed::default();
    let findings = check(sp);
    let u = upf(sp);
    let mut tags: BTreeSet<String> = sp.mep.equipment.iter().map(|e| e.tag.to_ascii_uppercase()).collect();
    let mut new: Vec<MepEquipment> = Vec::new();
    for f in &findings {
        if f.no_target {
            done.unplaced.push(format!("{} ({})", f.text, f.class.name));
            continue;
        }
        let needing: Vec<&(String, String, Option<String>)> = f.open.iter().filter(|o| o.2.is_none()).collect();
        let needing = if f.class.per == "output" { needing.into_iter().take(1).collect() } else { needing };
        for (tag, _, _) in needing {
            let Some(t) = sp.mep.equipment.iter().find(|e| e.tag == *tag) else { continue };
            let hw = r.module.box_in[0] / 24.0 * u;
            let at = Vec2::new(t.max[0] + 0.5 * u + hw, (t.min[1] + t.max[1]) / 2.0);
            let base = format!("{tag}-FAR");
            let mut mtag = base.clone();
            let mut k = 2;
            while tags.contains(&mtag.to_ascii_uppercase()) && k < 1_000 {
                mtag = format!("{base}{k}");
                k += 1;
            }
            tags.insert(mtag.to_ascii_uppercase());
            let mut p = BTreeMap::new();
            p.insert("FOR".to_string(), tag.clone());
            p.insert("FA_OUTPUT".to_string(), f.class.id.clone());
            p.insert("SEQUENCE".to_string(), f.text.chars().take(240).collect());
            p.insert("STATUS".to_string(), "inferred".to_string());
            p.insert("PROVENANCE".to_string(), "inferred".to_string());
            p.insert(
                "BASIS".to_string(),
                format!("inferred: the fire alarm sequence calls for {} and no drawing shows its relay; {}", f.class.name, r.module.source),
            );
            if f.class.device == "shunt" {
                p.insert("NOT_DRAWN".to_string(), format!("the drawings show no shunt trip; proposed: {}", f.class.proposal));
            }
            let mut s = BTreeMap::new();
            s.insert("position".to_string(), format!("beside {tag} (rules data)"));
            s.insert("why".to_string(), format!("{}: {}", f.sheets.join(", "), f.text));
            new.push(MepEquipment {
                tag: mtag.clone(),
                kind: r.module.kind.clone(),
                level: t.level.clone(),
                min: [at.x - hw, at.y - hw],
                max: [at.x + hw, at.y + hw],
                bottom_ft: t.bottom_ft,
                height_ft: (r.module.box_in[2] / 12.0).max(0.05),
                systems: vec!["FA".into()],
                title: r.module.name.clone(),
                properties: p,
                sources: s,
                confidence: 0.4,
                ..Default::default()
            });
            done.modules.push(mtag);
        }
    }
    sp.mep.equipment.extend(new);
    done
}

// ─── Damper actuator loads ───────────────────────────────────────────────────

/// The number a value starts with (`100 VA` → 100).
fn lead_num(v: &str) -> Option<f64> {
    let t = v.trim();
    let end = t.find(|c: char| !(c.is_ascii_digit() || c == '.')).unwrap_or(t.len());
    t.get(..end).and_then(|n| n.parse::<f64>().ok()).filter(|n| n.is_finite())
}

/// A circuit feeding damper actuators whose schedule row doesn't carry their load.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct LoadFinding {
    pub panel: String,
    pub circuit: String,
    pub level: String,
    pub actuators: Vec<String>,
    pub row_va: Option<f64>,
    pub need_va: Option<f64>,
    pub sheet: String,
}

/// Each circuit the damper power boxes are on, against its schedule row: a row that carries no
/// load where the schedule prints loads, or less than the actuators' loads (when their data give
/// one). (circuits looked at, findings.)
pub(crate) fn actuator_loads(sp: &BuildingSpec) -> (usize, Vec<LoadFinding>) {
    let a = &rules().actuator_load;
    let mut by: BTreeMap<(String, String), (String, Vec<String>, Option<f64>)> = BTreeMap::new();
    for e in sp.mep.equipment.iter().take(MAX_EQUIPMENT).filter(|e| !a.box_kind.is_empty() && e.kind == a.box_kind) {
        let (Some(p), Some(c)) = (e.properties.get("PANEL"), e.properties.get("CIRCUIT")) else { continue };
        let fed = e.properties.get("FEEDS").and_then(|t| sp.mep.equipment.iter().find(|d| d.tag == *t));
        let va = fed.and_then(|d| a.va_keys.iter().find_map(|k| d.properties.get(k).and_then(|v| lead_num(v))));
        let entry = by.entry((p.trim().to_ascii_uppercase(), c.trim().to_string())).or_insert_with(|| (e.level.clone(), Vec::new(), Some(0.0)));
        entry.1.push(fed.map_or_else(|| e.tag.clone(), |d| d.tag.clone()));
        entry.2 = match (entry.2, va) {
            (Some(s), Some(v)) if v.is_finite() && v > 0.0 => Some(s + v),
            _ => None,
        };
    }
    let mut out = Vec::new();
    let mut looked = 0usize;
    for ((panel, circuit), (level, actuators, need)) in by {
        let Some(ps) = sp.mep.panel_schedules.iter().find(|ps| ps.panel.trim().eq_ignore_ascii_case(&panel)) else { continue };
        let first = circuit.split(',').next().unwrap_or("").trim().to_string();
        let Some(row) = ps.circuits.iter().find(|c| c.number.trim() == first) else { continue };
        looked += 1;
        let prints_loads = ps.circuits.iter().any(|c| c.load_va.is_some_and(|v| v.is_finite() && v > 0.0));
        let row_va = row.load_va.filter(|v| v.is_finite());
        let short = match (row_va.filter(|v| *v > 0.0), need) {
            (None, _) => prints_loads,
            (Some(have), Some(n)) => have < n * (1.0 - a.tolerance.clamp(0.0, 1.0)),
            (Some(_), None) => false,
        };
        if short {
            out.push(LoadFinding {
                panel: ps.panel.clone(),
                circuit: first,
                level,
                actuators,
                row_va,
                need_va: need.filter(|n| *n > 0.0),
                sheet: if row.source.is_empty() { ps.source.clone() } else { row.source.clone() },
            });
        }
    }
    (looked, out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f64::consts::FRAC_PI_2;

    fn t(v: &str, x: f64, y: f64) -> TextItem {
        TextItem { value: v.into(), at: Vec2::new(x, y), height: 2.0, rotation: 0.0 }
    }

    fn turned(v: &str, x: f64, y: f64) -> TextItem {
        TextItem { value: v.into(), at: Vec2::new(x, y), height: 2.0, rotation: FRAC_PI_2 }
    }

    fn eq(tag: &str, kind: &str, title: &str, props: &[(&str, &str)]) -> MepEquipment {
        MepEquipment {
            tag: tag.into(),
            kind: kind.into(),
            level: "L1".into(),
            min: [0.0, 0.0],
            max: [24.0, 24.0],
            title: title.into(),
            properties: props.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect(),
            ..Default::default()
        }
    }

    #[test]
    fn the_rules_data_reads_and_every_class_says_its_code_and_device() {
        let r = rules();
        assert!(r.outputs.len() >= 6, "{}", r.outputs.len());
        for c in &r.outputs {
            assert!(!c.id.is_empty() && !c.basis.is_empty() && !c.ask.is_empty() && !c.proposal.is_empty(), "{}", c.id);
            assert!(matches!(c.device.as_str(), "relay" | "shunt") && matches!(c.per.as_str(), "target" | "output"), "{}", c.id);
        }
        assert!(class_by_id(&r.tie.class).is_some());
        assert!(!r.module.kind.is_empty() && !r.actuator_load.box_kind.is_empty());
    }

    #[test]
    fn outputs_are_told_apart_by_their_words() {
        let id = |s: &str| class_of(s).map(|c| c.id.as_str());
        assert_eq!(id("SHUNT TRIP ELEVATOR POWER"), Some("shunt_trip"));
        assert_eq!(id("Recall elevators to the main level"), Some("elevator_recall"));
        assert_eq!(id("RELEASE ALL FIRE/SMOKE DAMPERS"), Some("damper_release"));
        assert_eq!(id("Close smoke dampers"), Some("damper_release"));
        assert_eq!(id("RELEASE MAGNETIC DOOR HOLDERS"), Some("door_release"));
        assert_eq!(id("SHUT DOWN AIR HANDLING UNITS"), Some("fan_control"));
        assert_eq!(id("Start stair pressurization fan"), Some("fan_control"));
        // Inputs, a monitor point and titles call for nothing.
        for s in ["TOP OF SHAFT SMOKE DETECTOR", "DUCT SMOKE DETECTOR AHU-1", "SMOKE DAMPER POSITION", "SEQUENCE OF OPERATIONS", "WATERFLOW SWITCH"] {
            assert_eq!(id(s), None, "{s}");
        }
    }

    /// A matrix: inputs down the left, outputs across the top (level or turned); an X marks each
    /// cell. Only the outputs an input marks count.
    #[test]
    fn a_matrix_counts_the_outputs_its_inputs_mark_in_either_layout() {
        let mut texts = vec![
            t("FIRE ALARM SEQUENCE OF OPERATIONS MATRIX", 0.0, 200.0),
            t("TOP OF SHAFT SMOKE DETECTOR", 0.0, 100.0),
            t("MANUAL PULL STATION", 0.0, 90.0),
            turned("SHUNT TRIP ELEVATOR POWER", 82.0, 110.0),
            turned("RELEASE DOOR HOLDERS", 92.0, 110.0),
            turned("SHUT DOWN SUPPLY FANS", 102.0, 110.0),
            // An output no input marks.
            turned("RECALL ELEVATORS", 112.0, 110.0),
        ];
        texts.push(t("X", 80.0, 100.0));
        texts.push(t("X", 90.0, 90.0));
        texts.push(t("X", 90.0, 100.0));
        texts.push(t("X", 100.0, 90.0));
        let got = outputs(&[("FA-001".into(), texts)]);
        let ids: Vec<(&str, u32)> = got.iter().map(|o| (o.class.as_str(), o.inputs)).collect();
        assert_eq!(ids, [("shunt_trip", 1), ("door_release", 2), ("fan_control", 1)], "{got:?}");
        // The other way round: outputs down the left, inputs across the top.
        let texts = vec![
            t("SEQUENCE OF OPERATION", 0.0, 200.0),
            turned("WATERFLOW", 80.0, 110.0),
            turned("ELEVATOR LOBBY SMOKE", 90.0, 110.0),
            t("RECALL ELEVATORS", 0.0, 100.0),
            t("CLOSE SMOKE DAMPERS", 0.0, 90.0),
            t("X", 90.0, 100.0),
            t("\u{25cf}", 80.0, 100.0),
        ];
        let got = outputs(&[("FA-002".into(), texts)]);
        assert_eq!(got.iter().map(|o| (o.class.as_str(), o.inputs)).collect::<Vec<_>>(), [("elevator_recall", 2)], "{got:?}");
        // A written sequence (no marks): what it says.
        let texts = vec![t("SEQUENCE OF OPERATIONS:", 0.0, 100.0), t("1. SHUNT TRIP ELEVATOR POWER ON TOP OF SHAFT SMOKE.", 0.0, 90.0)];
        assert_eq!(outputs(&[("E-001".into(), texts)]).len(), 1);
        // A page that names no sequence or matrix: nothing.
        assert!(outputs(&[("E-101".into(), vec![t("SHUNT TRIP ELEVATOR POWER", 0.0, 0.0)])]).is_empty());
    }

    #[test]
    fn a_shunt_trip_needs_a_marked_breaker_and_its_control_power() {
        let mut sp = BuildingSpec::default();
        sp.mep.fa_outputs.push(FaOutput { class: "shunt_trip".into(), text: "SHUNT TRIP ELEVATOR POWER".into(), sheet: "FA-001".into(), inputs: 1 });
        sp.mep.equipment.push(eq("DS-E", "disconnect", "ELEVATOR DISCONNECT", &[("RATING", "100A 3P")]));
        let f = check(&sp);
        assert_eq!(f.len(), 1);
        assert!(f[0].is_open() && f[0].devices.is_empty() && f[0].open.len() == 1, "{f:?}");
        // The panel schedule marks the elevator's breaker shunt trip: a device, but no control power.
        let mut ps: cadaddy_walk3d::PanelSchedule = serde_json::from_value(serde_json::json!({"panel": "PX", "source": "E-601"})).unwrap();
        ps.legend.insert("ST".into(), "SHUNT TRIP".into());
        let mut c: cadaddy_walk3d::PanelCircuit = serde_json::from_value(serde_json::json!({"number": "3", "description": "ELEVATOR"})).unwrap();
        c.protection = vec!["ST".into()];
        ps.circuits.push(c);
        sp.mep.panel_schedules.push(ps.clone());
        let f = check(&sp);
        assert!(f[0].is_open() && f[0].no_power && f[0].devices.len() == 1, "{f:?}");
        // And a circuit gives the shunt trip's control power: met.
        let p: cadaddy_walk3d::PanelCircuit = serde_json::from_value(serde_json::json!({"number": "5", "description": "SHUNT TRIP POWER"})).unwrap();
        sp.mep.panel_schedules[0].circuits.push(p);
        assert!(!check(&sp)[0].is_open(), "{:?}", check(&sp));
        // Or the disconnect's own schedule says shunt trip (power still asked).
        let mut sp2 = BuildingSpec::default();
        sp2.mep.fa_outputs = sp.mep.fa_outputs.clone();
        sp2.mep.equipment.push(eq("DS-E", "disconnect", "ELEVATOR DISCONNECT", &[("REMARKS", "WITH SHUNT TRIP")]));
        let f = check(&sp2);
        assert!(f[0].devices.len() == 1 && f[0].no_power, "{f:?}");
    }

    #[test]
    fn each_unit_an_output_or_a_schedule_note_ties_to_the_fire_alarm_needs_its_relay() {
        let mut sp = BuildingSpec::default();
        sp.mep.fa_outputs.push(FaOutput { class: "fan_control".into(), text: "SHUT DOWN SUPPLY FANS".into(), sheet: "FA-001".into(), inputs: 1 });
        sp.mep.equipment.push(eq("SF-1", "supply_fan", "SUPPLY FAN", &[]));
        sp.mep.equipment.push(eq("SF-2", "supply_fan", "SUPPLY FAN", &[]));
        // An exhaust fan whose schedule ties it to the fire alarm; another with no such note.
        sp.mep.equipment.push(eq("EF-1", "exhaust_fan", "EXHAUST FAN", &[("REMARKS", "INTERLOCK WITH THE FIRE ALARM CONTROL PANEL")]));
        sp.mep.equipment.push(eq("EF-2", "exhaust_fan", "EXHAUST FAN", &[]));
        // SF-1 has a relay module drawn for it.
        sp.mep.equipment.push(eq("CR-1", "fa_control_module", "CONTROL MODULE", &[("FOR", "SF-1")]));
        let f = check(&sp);
        assert_eq!(f.len(), 2, "{f:?}");
        let fans = &f[0];
        assert_eq!(fans.targets.len(), 2);
        assert_eq!(fans.open.iter().map(|o| o.0.as_str()).collect::<Vec<_>>(), ["SF-2"], "{fans:?}");
        let tie = &f[1];
        assert_eq!(tie.class.id, "equipment_interlock");
        assert_eq!(tie.targets.iter().map(|t| t.0.as_str()).collect::<Vec<_>>(), ["EF-1"]);
        assert!(tie.is_open());
        // The build completes them: a relay module beside each, inferred; then they are open by a
        // rule (still asked), and a second pass replaces its own modules.
        let done = complete(&mut sp);
        assert_eq!(done.modules.len(), 2, "{done:?}");
        let built: Vec<&MepEquipment> = sp.mep.equipment.iter().filter(|e| e.properties.contains_key("FA_OUTPUT")).collect();
        assert!(built.iter().all(|e| e.properties.get("STATUS").map(String::as_str) == Some("inferred") && e.properties.contains_key("BASIS")));
        let f = check(&sp);
        assert_eq!(f[0].open_counts(), (1, 0), "{f:?}");
        assert_eq!(f[1].open_counts(), (1, 0), "{f:?}");
        complete(&mut sp);
        assert_eq!(sp.mep.equipment.iter().filter(|e| e.properties.contains_key("FA_OUTPUT")).count(), 2);
        // An output naming a unit by its tag acts on that one only.
        let one = class_by_id("fan_control").unwrap();
        assert_eq!(targets(&sp, one, "SHUT DOWN EF-2 ON ALARM").len(), 1);
    }

    #[test]
    fn an_output_acting_on_nothing_in_the_model_is_open() {
        let mut sp = BuildingSpec::default();
        sp.mep.fa_outputs.push(FaOutput { class: "door_release".into(), text: "RELEASE DOOR HOLDERS".into(), sheet: "FA-001".into(), inputs: 2 });
        let f = check(&sp);
        assert!(f[0].no_target && f[0].is_open(), "{f:?}");
        assert_eq!(complete(&mut sp).unplaced.len(), 1);
    }

    #[test]
    fn a_circuit_feeding_damper_actuators_carries_their_load() {
        let mut sp = BuildingSpec::default();
        for (d, va) in [("FSD-1", "100"), ("FSD-2", "100")] {
            sp.mep.equipment.push(eq(d, "fire_smoke_damper", "damper", &[("ACTUATOR_VA", va)]));
            sp.mep.equipment.push(eq(&format!("{d}-PWR"), "damper_power_box", "box", &[("FEEDS", d), ("PANEL", "PQ"), ("CIRCUIT", "12")]));
        }
        let mut ps: cadaddy_walk3d::PanelSchedule = serde_json::from_value(serde_json::json!({"panel": "PQ", "source": "E-601"})).unwrap();
        for (n, va) in [("10", Some(500.0)), ("12", None)] {
            let mut c: cadaddy_walk3d::PanelCircuit = serde_json::from_value(serde_json::json!({"number": n, "description": "DAMPERS"})).unwrap();
            c.load_va = va;
            ps.circuits.push(c);
        }
        sp.mep.panel_schedules.push(ps);
        let (looked, f) = actuator_loads(&sp);
        assert_eq!((looked, f.len()), (1, 1), "{f:?}");
        assert_eq!(f[0].actuators.len(), 2);
        assert_eq!(f[0].need_va, Some(200.0));
        // A row carrying less than the actuators' loads; then enough.
        sp.mep.panel_schedules[0].circuits[1].load_va = Some(100.0);
        assert_eq!(actuator_loads(&sp).1.len(), 1);
        sp.mep.panel_schedules[0].circuits[1].load_va = Some(200.0);
        assert!(actuator_loads(&sp).1.is_empty());
    }
}
