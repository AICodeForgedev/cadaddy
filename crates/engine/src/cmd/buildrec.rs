//! What a saved model was built with, and what was done to it since (Q6403).
//!
//! Every `model.build` writes a record into the building spec (`build_record`, saved with the
//! drawing): the engine that built it ([`super::guided::source_stamp`]), the build's switches, a
//! hash of each part of the spec and one of the rest of the drawing the build reads (the plan's
//! objects, blocks, layers, BIM types, project rules and overrides), and how many elements of each
//! level and part it made. What changes the model's elements after the build is added to it as a
//! step, with what it changed: a guided job's later steps (the tag identity merge, the gear, the
//! conduit routes…) by name, and a command run on its own by its command.
//!
//! `model.build` reads the record first. When nothing the build reads has changed (the same
//! engine, switches, spec and drawing), the model is already what its spec builds plus the
//! recorded steps, so it is kept as it is: a rebuild would only drop those steps (`force: true`
//! lays it out again). When the only change is to some levels (their elevation, height or
//! ceiling) or to the roofs, parapets and roof framing of some levels, only the elements of those
//! levels (and, for a roof, of the storey under it) are replaced; every other level keeps its
//! elements as they were, steps included. Otherwise it builds the whole model again, and says
//! what the new model changes per level and part, and why: the engine, switches, spec parts or
//! drawing that changed, and the steps after the last build it doesn't repeat.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::sync::Arc;

use cadaddy_doc::{Drawing, Entity};
use cadaddy_walk3d::BuildingSpec;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::wallgen;
use crate::Session;

/// The spec key the record is kept under.
pub(crate) const KEY: &str = "build_record";
/// Steps a record keeps (the oldest are dropped, and counted).
const MAX_STEPS: usize = 200;
/// Changed categories a report lists at most.
const MAX_LISTED: usize = 300;
/// The spec parts a partial rebuild handles: the levels and the exterior's roof parts, and where
/// a level's values came from (or that they were guessed).
const PARTIAL_KEYS: [&str; 4] = ["levels", "exterior", "sources", "guess"];
/// The spec parts that say where values came from, entry by entry.
const PROVENANCE_KEYS: [&str; 2] = ["sources", "guess"];

/// One change to the model's elements after its build.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct Step {
    /// What did it: a job's step by name, else the command.
    pub step: String,
    /// The command it ran in.
    pub by: String,
    /// Elements added (+) and taken away (−), by `level|part`.
    pub changed: BTreeMap<String, i64>,
    /// Elements it added, changed or took away.
    pub touched: u64,
}

/// What a model was built with, and the steps after its build.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct BuildRecord {
    /// The engine that built it.
    pub engine: String,
    /// The build's switches (slab thickness, stacking, routing).
    pub switches: BTreeMap<String, Value>,
    /// A hash of each part of the spec, as the model stands.
    pub spec: BTreeMap<String, String>,
    /// A hash of each level's entry, by name.
    pub levels: BTreeMap<String, String>,
    /// The hashes of the exterior's entries, by `list|level`.
    pub exterior: BTreeMap<String, Vec<String>>,
    /// A hash of each entry of where the spec's values came from, by `part|path`.
    pub provenance: BTreeMap<String, String>,
    /// A hash of the rest of the drawing the build reads.
    pub drawing: String,
    /// The elements the build made, by `level|part`.
    pub built: BTreeMap<String, u64>,
    /// What changed the elements after the build, oldest first.
    pub steps: Vec<Step>,
    /// Steps dropped from the front of `steps` (over [`MAX_STEPS`]).
    pub steps_dropped: u64,
    /// The elements as the last step left them, by `level|part`.
    pub now: BTreeMap<String, u64>,
}

/// FNV-1a over formatted text (no copy of it is made).
struct Fnv(u64);

impl Fnv {
    fn new() -> Self {
        Fnv(0xcbf2_9ce4_8422_2325)
    }
    fn hex(&self) -> String {
        format!("{:016x}", self.0)
    }
}

impl std::fmt::Write for Fnv {
    fn write_str(&mut self, s: &str) -> std::fmt::Result {
        for b in s.bytes() {
            self.0 = (self.0 ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3);
        }
        Ok(())
    }
}

fn hash_value(v: &Value) -> String {
    let mut h = Fnv::new();
    let _ = write!(h, "{v}");
    h.hex()
}

/// A built element's `level|part` (no level: `|part`).
fn key_of(e: &Entity) -> Option<String> {
    let part = wallgen::made_as(e)?;
    Some(format!("{}|{part}", super::bspec::made_level(e).unwrap_or_default()))
}

/// The model's built elements, by `level|part`.
pub(crate) fn counts(d: &Drawing) -> BTreeMap<String, u64> {
    let mut out: BTreeMap<String, u64> = BTreeMap::new();
    for e in d.model.iter() {
        if let Some(k) = key_of(e) {
            *out.entry(k).or_default() += 1;
        }
    }
    out
}

/// A hash of everything in the drawing a build reads besides the spec: the plan's objects (not
/// the ones a build made), the blocks, layers, BIM types, project rules and overrides.
pub(crate) fn drawing_hash(d: &Drawing) -> String {
    let mut h = Fnv::new();
    for e in d.model.iter().filter(|e| wallgen::made_as(e).is_none()) {
        let _ = write!(h, "{e:?}");
    }
    // A block field by field and its objects one by one: its object store's own layout (its
    // index's order) differs from one reading of the same file to the next.
    for (name, b) in d.blocks.iter() {
        let _ = write!(
            h,
            "|block {name} {:?} {} {:?} {} {} {} {:?} {} {} {:?}",
            b.name, b.annotative, b.base, b.description, b.xref_overlay, b.anonymous, b.xref_path, b.explodable, b.units, b.array
        );
        for e in b.entities.iter() {
            let _ = write!(h, "|{e:?}");
        }
    }
    let _ = write!(h, "|{:?}|{:?}|{:?}|{:?}", d.layers, d.bim_types, d.project_rules, d.overrides);
    h.hex()
}

/// The build's switches as a record keeps them.
pub(crate) fn switches(slab_ft: f64, stack: bool, route: bool) -> BTreeMap<String, Value> {
    BTreeMap::from([("slab_ft".to_string(), json!(slab_ft)), ("stack".to_string(), json!(stack)), ("route".to_string(), json!(route))])
}

/// The spec's parts hashed: each part, its levels' entries by name, its exterior's entries by
/// list and level, and its provenance entries by path (the record itself left out).
#[derive(Default)]
struct Hashes {
    parts: BTreeMap<String, String>,
    levels: BTreeMap<String, String>,
    exterior: BTreeMap<String, Vec<String>>,
    provenance: BTreeMap<String, String>,
}

fn spec_hashes(sp: &BuildingSpec) -> Hashes {
    let mut out = Hashes::default();
    let Ok(Value::Object(mut o)) = serde_json::to_value(sp) else { return out };
    o.remove(KEY);
    for (k, v) in &o {
        out.parts.insert(k.clone(), hash_value(v));
    }
    for l in o.get("levels").and_then(Value::as_array).into_iter().flatten() {
        let name = l.get("name").and_then(Value::as_str).unwrap_or_default();
        out.levels.insert(name.to_string(), hash_value(l));
    }
    for (list, v) in o.get("exterior").and_then(Value::as_object).into_iter().flatten() {
        match v.as_array() {
            Some(items) => {
                for item in items {
                    let level = item.get("level").and_then(Value::as_str).unwrap_or_default();
                    out.exterior.entry(format!("{list}|{level}")).or_default().push(hash_value(item));
                }
            }
            None => out.exterior.entry(format!("{list}|")).or_default().push(hash_value(v)),
        }
    }
    for v in out.exterior.values_mut() {
        v.sort();
    }
    for part in PROVENANCE_KEYS {
        for (path, v) in o.get(part).and_then(Value::as_object).into_iter().flatten() {
            out.provenance.insert(format!("{part}|{path}"), hash_value(v));
        }
    }
    out
}

/// The drawing's stored spec, when it can be read.
fn stored(d: &Drawing) -> Option<BuildingSpec> {
    d.building_spec.as_deref().filter(|t| !t.trim().is_empty()).and_then(|t| BuildingSpec::from_json(t).ok())
}

/// A stored spec's text without its build record: what changed in the spec, not in how the model
/// was built (the record is written after every build).
pub(crate) fn without_record(text: &str) -> std::borrow::Cow<'_, str> {
    if !text.contains(KEY) {
        return std::borrow::Cow::Borrowed(text);
    }
    // Written back as the spec is written (its fields in their order), so a spec stored with no
    // record reads the same.
    let Ok(mut sp) = BuildingSpec::from_json(text) else { return std::borrow::Cow::Borrowed(text) };
    if sp.kept.remove(KEY).is_none() {
        return std::borrow::Cow::Borrowed(text);
    }
    std::borrow::Cow::Owned(sp.to_json())
}

/// The record a drawing's stored spec keeps.
pub(crate) fn read(d: &Drawing) -> Option<BuildRecord> {
    stored(d)?.kept.get(KEY).and_then(|v| serde_json::from_value(v.clone()).ok())
}

/// Write `rec` into the drawing's stored spec (none stored: nothing is written).
fn write(s: &mut Session, rec: &BuildRecord) {
    let Ok(d) = s.doc() else { return };
    let Some(mut sp) = stored(d) else { return };
    let Ok(v) = serde_json::to_value(rec) else { return };
    sp.kept.insert(KEY.into(), v);
    if let Ok(d) = s.doc_mut() {
        d.building_spec = Some(sp.to_json()).into();
    }
}

impl BuildRecord {
    /// A record of the build just made from `sp` with `switches`.
    pub(crate) fn new(d: &Drawing, sp: &BuildingSpec, switches: BTreeMap<String, Value>) -> Self {
        let h = spec_hashes(sp);
        let built = counts(d);
        BuildRecord {
            engine: super::guided::source_stamp().to_string(),
            switches,
            spec: h.parts,
            levels: h.levels,
            exterior: h.exterior,
            provenance: h.provenance,
            drawing: String::new(),
            now: built.clone(),
            built,
            steps: Vec::new(),
            steps_dropped: 0,
        }
    }

    /// Add a step that took the model from the counts it had to `now` (nothing when nothing changed).
    fn push(&mut self, step: &str, by: &str, now: BTreeMap<String, u64>, touched: u64) {
        let mut changed: BTreeMap<String, i64> = BTreeMap::new();
        for k in self.now.keys().chain(now.keys()) {
            let a = self.now.get(k).copied().unwrap_or(0);
            let b = now.get(k).copied().unwrap_or(0);
            if a != b {
                changed.insert(k.clone(), i64::try_from(b).unwrap_or(i64::MAX).saturating_sub(i64::try_from(a).unwrap_or(i64::MAX)));
            }
        }
        if changed.is_empty() && touched == 0 {
            return;
        }
        self.steps.push(Step { step: step.chars().take(200).collect(), by: by.to_string(), changed, touched });
        if self.steps.len() > MAX_STEPS {
            self.steps.remove(0);
            self.steps_dropped = self.steps_dropped.saturating_add(1);
        }
        self.now = now;
    }
}

/// What the build reads now: compared with a record, the reasons a rebuild is needed.
pub(crate) struct Inputs {
    pub engine: String,
    pub switches: BTreeMap<String, Value>,
    pub spec: BTreeMap<String, String>,
    pub levels: BTreeMap<String, String>,
    pub exterior: BTreeMap<String, Vec<String>>,
    pub provenance: BTreeMap<String, String>,
    pub drawing: String,
}

impl Inputs {
    pub(crate) fn of(d: &Drawing, sp: &BuildingSpec, switches: BTreeMap<String, Value>) -> Self {
        let h = spec_hashes(sp);
        Inputs {
            engine: super::guided::source_stamp().to_string(),
            switches,
            spec: h.parts,
            levels: h.levels,
            exterior: h.exterior,
            provenance: h.provenance,
            drawing: drawing_hash(d),
        }
    }

    /// The spec parts that differ from the record's.
    fn spec_changed(&self, rec: &BuildRecord) -> Vec<String> {
        let keys: BTreeSet<&String> = self.spec.keys().chain(rec.spec.keys()).collect();
        keys.into_iter().filter(|k| self.spec.get(*k) != rec.spec.get(*k)).cloned().collect()
    }

    /// Why the model isn't what the build would make from these inputs (empty: it is).
    pub(crate) fn differences(&self, rec: &BuildRecord) -> Vec<String> {
        let mut why = Vec::new();
        if self.engine != rec.engine {
            why.push(format!("built by engine {}, this is {}: the build's own code or rules data changed", rec.engine, self.engine));
        }
        for (k, v) in &self.switches {
            if rec.switches.get(k) != Some(v) {
                why.push(format!("switch {k}: {} at the build, {v} now", rec.switches.get(k).map_or_else(|| "unset".to_string(), Value::to_string)));
            }
        }
        let parts = self.spec_changed(rec);
        if !parts.is_empty() {
            why.push(format!("the spec changed since the build: {}", parts.join(", ")));
        }
        if self.drawing != rec.drawing {
            why.push("the plan (or its blocks, layers, BIM types, project rules or overrides) changed since the build".into());
        }
        why
    }

    /// The levels whose elements a rebuild must replace when the only change since `rec` is to
    /// some levels' entries or the exterior's entries of some levels: those levels, and the
    /// storey under a level whose roof parts changed. `None`: the whole model is rebuilt.
    pub(crate) fn partial(&self, rec: &BuildRecord, sp: &BuildingSpec) -> Option<BTreeSet<String>> {
        if self.engine != rec.engine || self.switches != rec.switches || self.drawing != rec.drawing || sp.buildings.several() {
            return None;
        }
        let parts = self.spec_changed(rec);
        if parts.is_empty() || parts.iter().any(|p| !PARTIAL_KEYS.contains(&p.as_str())) {
            return None;
        }
        let mut levels: BTreeSet<String> = BTreeSet::new();
        let names: BTreeSet<&String> = self.levels.keys().chain(rec.levels.keys()).collect();
        for n in names {
            if self.levels.get(n) != rec.levels.get(n) {
                levels.insert(n.to_ascii_lowercase());
            }
        }
        // Where a value came from: a level's (`levels/<name>`) goes with that level; any other
        // can change anything.
        let paths: BTreeSet<&String> = self.provenance.keys().chain(rec.provenance.keys()).collect();
        for p in paths {
            if self.provenance.get(p) == rec.provenance.get(p) {
                continue;
            }
            let path = p.split_once('|').map(|(_, x)| x).unwrap_or_default();
            let level = path.strip_prefix("levels/").filter(|l| !l.contains('/') && !l.trim().is_empty())?;
            levels.insert(level.trim().to_ascii_lowercase());
        }
        let keys: BTreeSet<&String> = self.exterior.keys().chain(rec.exterior.keys()).collect();
        for k in keys {
            if self.exterior.get(k) == rec.exterior.get(k) {
                continue;
            }
            let level = k.split_once('|').map(|(_, l)| l.trim()).unwrap_or_default();
            // An exterior part of no one level (a facade) can change any of them.
            if level.is_empty() {
                return None;
            }
            levels.insert(level.to_ascii_lowercase());
            // The storey under a roof carries it: its walls stop at the roof's bearing.
            let at = sp.levels.iter().find(|l| l.name.eq_ignore_ascii_case(level)).map(|l| l.elevation_ft);
            if let Some(below) =
                at.and_then(|z| sp.levels.iter().filter(|l| l.elevation_ft < z - 1e-6).max_by(|a, b| a.elevation_ft.total_cmp(&b.elevation_ft)))
            {
                levels.insert(below.name.to_ascii_lowercase());
            }
        }
        // Every level: the whole model is rebuilt anyway.
        let all: BTreeSet<String> = sp.levels.iter().map(|l| l.name.to_ascii_lowercase()).collect();
        if levels.is_empty() || all.is_subset(&levels) {
            return None;
        }
        Some(levels)
    }
}

/// After a whole rebuild into `d` (which replaced `old`'s model): every level not in `affected`
/// (lower case), and every element of no level, gets its elements from `old` back. (Rebuilt
/// elements replaced, old elements put back.)
pub(crate) fn keep_unaffected(d: &mut Drawing, old: &Drawing, affected: &BTreeSet<String>) -> (usize, usize) {
    let hit = |e: &Entity| super::bspec::made_level(e).is_some_and(|l| affected.contains(&l.to_ascii_lowercase()));
    let drop: Vec<cadaddy_doc::Handle> = d.model.iter().filter(|e| wallgen::made_as(e).is_some() && !hit(e)).map(|e| e.handle).collect();
    for h in &drop {
        d.model.remove(*h);
    }
    let mut back = 0;
    for e in old.model.iter().filter(|e| wallgen::made_as(e).is_some() && !hit(e)) {
        if d.model.get(e.handle).is_some() {
            continue;
        }
        d.bump_handseed(e.handle);
        d.model.push_shared(Arc::clone(e));
        back += 1;
    }
    let rebuilt = d.model.iter().filter(|e| wallgen::made_as(e).is_some() && hit(e)).count();
    (rebuilt, back)
}

/// What a rebuild changed: the categories (`level|part`) whose counts differ, before and after.
pub(crate) fn changes(before: &BTreeMap<String, u64>, after: &BTreeMap<String, u64>) -> (usize, Vec<Value>) {
    let keys: BTreeSet<&String> = before.keys().chain(after.keys()).collect();
    let total = keys.len();
    let changed: Vec<Value> = keys
        .into_iter()
        .filter_map(|k| {
            let (a, b) = (before.get(k).copied().unwrap_or(0), after.get(k).copied().unwrap_or(0));
            let (level, part) = k.split_once('|').unwrap_or(("", k));
            (a != b).then(|| json!({"level": level, "part": part, "before": a, "after": b}))
        })
        .collect();
    (total, changed)
}

/// The steps of `rec` a whole rebuild doesn't repeat, as a report lists them.
pub(crate) fn steps_json(rec: &BuildRecord) -> Vec<Value> {
    rec.steps.iter().map(|st| json!({"step": st.step, "by": st.by, "touched": st.touched, "changed": st.changed})).collect()
}

/// A rebuild's report: what changed against the model it replaced and why.
pub(crate) fn report(rec: Option<&BuildRecord>, why: Vec<String>, before: &BTreeMap<String, u64>, after: &BTreeMap<String, u64>) -> Value {
    let (total, changed) = changes(before, after);
    let mut why = why;
    match rec {
        None => why.insert(
            0,
            "the model has no build record (built before records were kept, or never built from this spec): what it was built with is unknown".into(),
        ),
        Some(r) => {
            if !r.steps.is_empty() {
                let names: Vec<&str> = r.steps.iter().map(|s| s.step.as_str()).collect();
                why.push(format!(
                    "{} step(s) after the last build changed its elements and are not repeated by a rebuild: {}",
                    r.steps.len() as u64 + r.steps_dropped,
                    names.join(", ")
                ));
            }
            if r.now != *before {
                why.push("the elements changed since the last recorded step (by hand, or by a command that made none)".into());
            }
        }
    }
    json!({
        "categories": total,
        "changed_categories": changed.len(),
        "changed": changed.into_iter().take(MAX_LISTED).collect::<Vec<_>>(),
        "why": why,
        "steps_not_repeated": rec.map(steps_json).unwrap_or_default(),
    })
}

/// A build in this command: its record waits until the command called from outside ends (a
/// job's later steps add to it), then is written with the spec and drawing as they stand.
pub(crate) fn pending(s: &mut Session, rec: BuildRecord) {
    let uid = s.state().map(|st| st.uid).unwrap_or_default();
    s.pending_build = Some((uid, rec));
}

/// A job's step after its build: what it changed in the model, under its name.
pub(crate) fn step(s: &mut Session, name: &str) {
    let Some((uid, _)) = &s.pending_build else { return };
    let Ok(st) = s.state() else { return };
    if st.uid != *uid {
        return;
    }
    let now = counts(&st.doc);
    let by = s.outer_command.clone();
    if let Some((_, rec)) = s.pending_build.as_mut() {
        rec.push(name, &by, now, 0);
    }
}

/// After a command: at the end of the command called from outside, a build it made is recorded
/// with the model, spec and drawing it left; a command that changed a built model's elements is
/// added to its record as a step.
pub(crate) fn after_command(s: &mut Session, before: Option<&Arc<Drawing>>) {
    if !s.journal.at_top() {
        return;
    }
    let by = s.outer_command.clone();
    if let Some((uid, mut rec)) = s.pending_build.take() {
        let Ok(st) = s.state() else { return };
        if st.uid != uid {
            return;
        }
        let d = Arc::clone(&st.doc);
        let now = counts(&d);
        if now != rec.now {
            rec.push(&format!("{by} (after its build)"), &by, now, 0);
        }
        if let Some(sp) = stored(&d) {
            let h = spec_hashes(&sp);
            (rec.spec, rec.levels, rec.exterior, rec.provenance) = (h.parts, h.levels, h.exterior, h.provenance);
        }
        rec.drawing = drawing_hash(&d);
        write(s, &rec);
        return;
    }
    let Some(before) = before else { return };
    let Ok(st) = s.state() else { return };
    let now = Arc::clone(&st.doc);
    if Arc::ptr_eq(before, &now) || now.model.unchanged_from(&before.model) {
        return;
    }
    let (added, modified, erased) = before.model.changes(&now.model);
    let made = |d: &Drawing, h: &cadaddy_doc::Handle| d.model.get(*h).and_then(|e| key_of(e));
    let touched = added.iter().chain(&modified).chain(&erased).filter(|h| made(before, h).is_some() || made(&now, h).is_some()).count();
    if touched == 0 {
        return;
    }
    let Some(mut rec) = read(&now) else { return };
    let mut counted = rec.now.clone();
    for h in added.iter().chain(&modified).chain(&erased) {
        if let Some(k) = made(before, h)
            && let Some(n) = counted.get_mut(&k)
        {
            *n = n.saturating_sub(1);
        }
        if let Some(k) = made(&now, h) {
            *counted.entry(k).or_default() += 1;
        }
    }
    counted.retain(|_, n| *n > 0);
    rec.push(&by, &by, counted, touched as u64);
    write(s, &rec);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_step_records_what_it_changed() {
        let mut r = BuildRecord { now: BTreeMap::from([("L1|wall".to_string(), 10), ("L1|door".to_string(), 4)]), ..BuildRecord::default() };
        r.push("tag identity", "job.start", BTreeMap::from([("L1|wall".to_string(), 9), ("L1|door".to_string(), 4), ("L2|box".to_string(), 2)]), 0);
        assert_eq!(r.steps.len(), 1);
        assert_eq!(r.steps[0].changed, BTreeMap::from([("L1|wall".to_string(), -1), ("L2|box".to_string(), 2)]));
        // Nothing changed: no step.
        r.push("again", "job.start", r.now.clone(), 0);
        assert_eq!(r.steps.len(), 1);
    }

    #[test]
    fn steps_are_capped() {
        let mut r = BuildRecord::default();
        for i in 0..(MAX_STEPS as u64 + 5) {
            r.push("x", "y", BTreeMap::from([("|wall".to_string(), i + 1)]), 0);
        }
        assert_eq!(r.steps.len(), MAX_STEPS);
        assert_eq!(r.steps_dropped, 5);
    }

    #[test]
    fn a_spec_without_its_record_reads_as_stored_before_it() {
        let sp =
            BuildingSpec::from_json(r#"{"version":1,"units_per_foot":12,"levels":[{"name":"L1","elevation_ft":0,"height_ft":10,"ceiling_ft":8}]}"#)
                .unwrap();
        let plain = sp.to_json();
        let mut with = sp.clone();
        with.kept.insert(KEY.into(), json!({"engine": "x"}));
        assert_eq!(without_record(&with.to_json()), plain);
        assert_eq!(without_record(&plain), plain);
    }

    #[test]
    fn changes_lists_differing_categories() {
        let a = BTreeMap::from([("L1|wall".to_string(), 3), ("L1|door".to_string(), 1)]);
        let b = BTreeMap::from([("L1|wall".to_string(), 4), ("L1|door".to_string(), 1), ("L2|window".to_string(), 2)]);
        let (total, changed) = changes(&a, &b);
        assert_eq!(total, 3);
        assert_eq!(changed.len(), 2);
        assert_eq!(changed[0], json!({"level": "L1", "part": "wall", "before": 3, "after": 4}));
    }
}
