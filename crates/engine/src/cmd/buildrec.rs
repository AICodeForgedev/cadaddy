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
    /// While the command that built runs: [`Self::drawing`] of the whole print set, taken before
    /// its sheets went out of memory, and the hash of what stayed in memory then (Q6577). Never
    /// saved.
    #[serde(skip)]
    pub whole_plan: Option<(String, String)>,
}

/// FNV-1a over formatted text (no copy of it is made).
#[derive(Clone, Copy)]
pub(crate) struct Fnv(u64);

impl Fnv {
    pub(crate) fn new() -> Self {
        Fnv(0xcbf2_9ce4_8422_2325)
    }
    fn hex(&self) -> String {
        format!("{:016x}", self.0)
    }
    /// The hash of the text written so far.
    pub(crate) fn value(&self) -> u64 {
        self.0
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
/// the ones a build made) whatever their order, the blocks, layers, BIM types, project rules and
/// overrides.
pub(crate) fn drawing_hash(d: &Drawing) -> String {
    finish(plan_objects(d).by_handle, d, &super::overrides::build_input(d))
}

/// [`drawing_hash`] of `d` as a whole: with sheets out of memory, the hash taken before they went
/// out (`whole`: it and the hash of what stayed in memory then) while what is in memory is as it
/// was then, else the hash of every sheet loaded (a copy: `d` keeps them out).
fn plan_hash(d: &Arc<Drawing>, whole: Option<(String, String)>) -> String {
    if !d.sheet_store.has_parked() {
        return drawing_hash(d);
    }
    match whole {
        Some((whole, rest)) if drawing_hash(d) == rest => whole,
        _ => drawing_hash(&crate::sheetstore::combined(d)),
    }
}

/// The plan's hash for a build this command made, while the whole print set is in memory
/// ([`crate::Session::put_out_sheets`] takes it before the sheets go out): `None` when the command
/// made no build of the active drawing, or some sheets are out already.
pub(crate) fn plan_before_sheets_out(s: &Session) -> Option<String> {
    let (uid, _) = s.pending_build.as_ref()?;
    let st = s.state().ok()?;
    (st.uid == *uid && !st.doc.sheet_store.has_parked()).then(|| drawing_hash(&st.doc))
}

/// The sheets went out: the build's record keeps the whole plan's hash (`plan`, from
/// [`plan_before_sheets_out`]) and the hash of what stayed in memory, which tells at the end of
/// the command whether the plan changed since ([`plan_hash`]).
pub(crate) fn plan_after_sheets_out(s: &mut Session, plan: Option<String>) {
    let Some(whole) = plan else { return };
    let Ok(st) = s.state() else { return };
    let rest = drawing_hash(&st.doc);
    if let Some((_, rec)) = s.pending_build.as_mut() {
        rec.whole_plan = Some((whole, rest));
    }
}

/// The plan's objects (the ones no build made) hashed: as a set, each object's hash by its handle
/// ([`drawing_hash`]: a print set's sheets put out of memory and loaded again come back in
/// another order in model space, and the plan is the same, Q6577), and in draw order, as records
/// written before that hold it.
struct PlanObjects {
    by_handle: Fnv,
    ordered: Fnv,
}

fn plan_objects(d: &Drawing) -> PlanObjects {
    let mut ordered = Fnv::new();
    let mut each: Vec<(u64, u64)> = Vec::new();
    let mut text = String::new();
    for e in d.model.iter().filter(|e| wallgen::made_as(e).is_none()) {
        text.clear();
        let _ = write!(text, "{e:?}");
        let _ = ordered.write_str(&text);
        let mut one = Fnv::new();
        let _ = one.write_str(&text);
        each.push((e.handle.0, one.0));
    }
    each.sort_unstable();
    let mut by_handle = Fnv::new();
    for (h, v) in &each {
        let _ = write!(by_handle, "{h:x}:{v:016x}|");
    }
    PlanObjects { by_handle, ordered }
}

/// The drawing's hash in the three forms a record may hold: [`drawing_hash`]; as records written
/// before Q6577 hold it (the plan's objects in draw order); and before Q6620 (in draw order, with
/// every override as stored). A record of any of them still matches the drawing it was made of.
fn drawing_hashes(d: &Drawing) -> (String, String, String) {
    let p = plan_objects(d);
    let input = super::overrides::build_input(d);
    (finish(p.by_handle, d, &input), finish(p.ordered, d, &input), finish(p.ordered, d, &d.overrides))
}

/// `h` (the plan's objects hashed) with the rest of what a build reads: the blocks, layers, BIM
/// types, project rules and `overrides`.
fn finish(mut h: Fnv, d: &Drawing, overrides: &dyn std::fmt::Debug) -> String {
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
    let _ = write!(h, "|{:?}|{:?}|{:?}|{overrides:?}", d.layers, d.bim_types, d.project_rules);
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
    #[cfg(test)]
    tests::SPEC_READS.with(|n| n.set(n.get() + 1));
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
        let text = sp.to_json();
        let span = top_level_value(&text, KEY);
        d.building_spec = Some(text).into();
        if let Some(span) = span {
            remember(d, span, rec);
        }
    }
}

thread_local! {
    /// The record last written into (or read from) a stored spec, and where its value sits in
    /// that spec's text (Q6567): the step a command adds replaces that span alone. Reading and
    /// writing the whole spec (tens of megabytes on a whole job) made a move or erase of one
    /// built element cost 100-150 ms on a large model. Keyed by the very copy of the text
    /// ([`cadaddy_doc::shared::Shared::is`]): a spec changed any other way, undo's or another
    /// drawing's, is read afresh.
    static KEPT: std::cell::RefCell<Option<Kept>> = const { std::cell::RefCell::new(None) };
}

struct Kept {
    text: std::sync::Weak<Option<String>>,
    span: std::ops::Range<usize>,
    rec: BuildRecord,
}

fn remember(d: &Drawing, span: std::ops::Range<usize>, rec: &BuildRecord) {
    let k = Kept { text: d.building_spec.downgrade(), span, rec: rec.clone() };
    KEPT.with(|c| *c.borrow_mut() = Some(k));
}

/// The record `d`'s stored spec keeps (`None`: none), and where its value sits in the spec's
/// text (`None`: not where a scan finds it). Read whole once per copy of the text, so the record
/// is the one [`read`] gives.
fn record_in_text(d: &Drawing) -> Option<(Option<std::ops::Range<usize>>, BuildRecord)> {
    let held = KEPT.with(|c| c.borrow().as_ref().filter(|k| d.building_spec.is(&k.text)).map(|k| (Some(k.span.clone()), k.rec.clone())));
    if held.is_some() {
        return held;
    }
    let rec = read(d)?;
    let span = d.building_spec.as_deref().and_then(|t| {
        let span = top_level_value(t, KEY)?;
        let at: BuildRecord = serde_json::from_str(t.get(span.clone())?).ok()?;
        (at == rec).then_some(span)
    });
    if let Some(span) = &span {
        remember(d, span.clone(), &rec);
    }
    Some((span, rec))
}

/// Write `rec` over the record at `span` of the drawing's stored spec text: the rest of the text
/// is kept as it is, not read and written again. The text as [`write`] makes it: the record's
/// value in the spec's own JSON form.
fn write_at(s: &mut Session, span: std::ops::Range<usize>, rec: &BuildRecord) {
    let Some(json) = serde_json::to_value(rec).ok().and_then(|v| serde_json::to_string(&v).ok()) else { return };
    let Ok(d) = s.doc_mut() else { return };
    let text = d.building_spec.as_deref().and_then(|t| Some((t.get(..span.start)?, t.get(span.end..)?)));
    let Some((head, tail)) = text else { return write(s, rec) };
    let mut t = String::with_capacity(head.len().saturating_add(json.len()).saturating_add(tail.len()));
    t.push_str(head);
    t.push_str(&json);
    t.push_str(tail);
    let end = span.start.saturating_add(json.len());
    d.building_spec = Some(t).into();
    remember(d, span.start..end, rec);
}

/// Where the value of the top-level member `key` sits in the JSON object `text` (its byte range),
/// by a scan that builds nothing; `None` when the text is no object, has no such member, or is
/// cut short. A key written with escapes is no match.
fn top_level_value(text: &str, key: &str) -> Option<std::ops::Range<usize>> {
    let b = text.as_bytes();
    let ws = |mut i: usize| {
        while b.get(i).is_some_and(u8::is_ascii_whitespace) {
            i += 1;
        }
        i
    };
    let mut i = ws(0);
    if b.get(i) != Some(&b'{') {
        return None;
    }
    i = ws(i + 1);
    loop {
        let end = string_end(b, i)?;
        let name = text.get(i + 1..end.checked_sub(1)?)?;
        i = ws(end);
        if b.get(i) != Some(&b':') {
            return None;
        }
        let start = ws(i + 1);
        let stop = value_end(b, start)?;
        if name == key {
            return Some(start..stop);
        }
        i = ws(stop);
        if b.get(i) != Some(&b',') {
            return None;
        }
        i = ws(i + 1);
    }
}

/// Just past the JSON string starting at `i`.
fn string_end(b: &[u8], i: usize) -> Option<usize> {
    if b.get(i) != Some(&b'"') {
        return None;
    }
    let mut j = i + 1;
    loop {
        match b.get(j)? {
            b'\\' => j += 2,
            b'"' => return Some(j + 1),
            _ => j += 1,
        }
    }
}

/// Just past the JSON value starting at `i`.
fn value_end(b: &[u8], i: usize) -> Option<usize> {
    match b.get(i)? {
        b'"' => string_end(b, i),
        b'{' | b'[' => {
            let mut depth = 0usize;
            let mut j = i;
            loop {
                match b.get(j)? {
                    b'"' => {
                        j = string_end(b, j)?;
                        continue;
                    }
                    b'{' | b'[' => depth += 1,
                    b'}' | b']' => {
                        depth = depth.checked_sub(1)?;
                        if depth == 0 {
                            return Some(j + 1);
                        }
                    }
                    _ => {}
                }
                j += 1;
            }
        }
        _ => {
            let mut j = i;
            while b.get(j).is_some_and(|c| !matches!(c, b',' | b'}' | b']') && !c.is_ascii_whitespace()) {
                j += 1;
            }
            (j > i).then_some(j)
        }
    }
}

/// Whether the drawing's model is built from its stored spec as it stands (its record's hashes
/// are the spec's): what [`spec_kept_in_model`] needs before a command edits both.
pub(crate) fn spec_in_sync(d: &Drawing) -> bool {
    let (Some(rec), Some(sp)) = (read(d), stored(d)) else { return false };
    let h = spec_hashes(&sp);
    rec.spec == h.parts && rec.levels == h.levels && rec.exterior == h.exterior && rec.provenance == h.provenance
}

/// A command changed the spec and made the same change to the built model itself (an approved
/// clash fix moves the pieces and keeps the move as a spec edit, Q6381): the record takes the
/// spec as it is now as built, so the next `model.build` keeps the model (the command stays a
/// recorded step) and a rebuild lays it out the same. Only when [`spec_in_sync`] held before
/// the edit: a spec change no build has made yet still rebuilds.
pub(crate) fn spec_kept_in_model(s: &mut Session) {
    let Ok(d) = s.doc() else { return };
    let (Some(mut rec), Some(sp)) = (read(d), stored(d)) else { return };
    let h = spec_hashes(&sp);
    (rec.spec, rec.levels, rec.exterior, rec.provenance) = (h.parts, h.levels, h.exterior, h.provenance);
    write(s, &rec);
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
            whole_plan: None,
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
    /// The drawing hashed as a record written before Q6577 did it (the plan's objects in order).
    drawing_ordered: String,
    /// The drawing hashed as a record written before Q6620 did it.
    drawing_before: String,
}

impl Inputs {
    pub(crate) fn of(d: &Drawing, sp: &BuildingSpec, switches: BTreeMap<String, Value>) -> Self {
        let h = spec_hashes(sp);
        let (drawing, drawing_ordered, drawing_before) = drawing_hashes(d);
        Inputs {
            engine: super::guided::source_stamp().to_string(),
            switches,
            spec: h.parts,
            levels: h.levels,
            exterior: h.exterior,
            provenance: h.provenance,
            drawing,
            drawing_ordered,
            drawing_before,
        }
    }

    /// Whether the drawing differs from the one `rec` was made of.
    fn drawing_changed(&self, rec: &BuildRecord) -> bool {
        self.drawing != rec.drawing && self.drawing_ordered != rec.drawing && self.drawing_before != rec.drawing
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
        if self.drawing_changed(rec) {
            why.push("the plan (or its blocks, layers, BIM types, project rules or overrides) changed since the build".into());
        }
        why
    }

    /// The levels whose elements a rebuild must replace when the only change since `rec` is to
    /// some levels' entries or the exterior's entries of some levels: those levels, and the
    /// storey under a level whose roof parts changed. `None`: the whole model is rebuilt.
    pub(crate) fn partial(&self, rec: &BuildRecord, sp: &BuildingSpec) -> Option<BTreeSet<String>> {
        if self.engine != rec.engine || self.switches != rec.switches || self.drawing_changed(rec) || sp.buildings.several() {
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
        // A riser's pieces on the levels round a changed one are cut at its floor (Q6493): they
        // are laid out again with it.
        let sharing = super::risercut::levels_sharing_risers(sp, &levels);
        levels.extend(sharing);
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

/// The names a built element answers to (lower case): its build part (`casework`, `ceiling`,
/// `mep`), its element kind (`light_fixture`, `equipment`) and its own kind as read
/// (`receptacle`); an elevator's hoistway (its shaft walls, pit and overrun) to `hoistway` too.
fn part_names(e: &Entity) -> Vec<String> {
    let Some(part) = wallgen::made_as(e) else { return Vec::new() };
    let mut out = vec![part.to_ascii_lowercase()];
    if let Some(kind) = super::elemprops::element_kind(e) {
        out.push(kind.to_ascii_lowercase());
        out.push(super::elemprops::reported_kind(e, kind).to_ascii_lowercase());
    }
    if part == super::hoistway::PIT || part == super::hoistway::OVERRUN || super::bimlook::made_tag(e, "hoistway").is_some() {
        out.push("hoistway".into());
    }
    out
}

/// A whole build of which only `parts` (lower case, [`part_names`]) are taken (Q6620): the
/// build's elements of other parts go and the model's own (`old`) come back as they stood, with
/// every correction made to them since their build. Returns (elements of the parts rebuilt,
/// elements kept, the parts no element of either model is of).
pub(crate) fn keep_parts(d: &mut Drawing, old: &Drawing, parts: &BTreeSet<String>) -> (usize, usize, Vec<String>) {
    let mut seen: BTreeSet<String> = BTreeSet::new();
    let mut hit = |e: &Entity| {
        let mine: Vec<String> = part_names(e).into_iter().filter(|n| parts.contains(n)).collect();
        let yes = !mine.is_empty();
        seen.extend(mine);
        yes
    };
    let mut rebuilt = 0;
    let mut drop: Vec<cadaddy_doc::Handle> = Vec::new();
    for e in d.model.iter().filter(|e| super::revise3d::built_object(e)) {
        if hit(e) {
            rebuilt += 1;
        } else {
            drop.push(e.handle);
        }
    }
    for h in &drop {
        d.model.remove(*h);
    }
    let mut back = 0;
    for e in old.model.iter().filter(|e| super::revise3d::built_object(e)) {
        if hit(e) || d.model.get(e.handle).is_some() {
            continue;
        }
        d.bump_handseed(e.handle);
        d.model.push_shared(Arc::clone(e));
        back += 1;
    }
    (rebuilt, back, parts.iter().filter(|p| !seen.contains(*p)).cloned().collect())
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
/// added to its record as a step. `switched`: the command started in another drawing (it made
/// no step: nothing to compare with it).
pub(crate) fn after_command(s: &mut Session, before: Option<&Arc<Drawing>>, switched: bool) {
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
        rec.drawing = plan_hash(&d, rec.whole_plan.take());
        write(s, &rec);
        return;
    }
    let Some(before) = before.filter(|_| !switched) else { return };
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
    // The record and where it sits in the spec's text: the step is written over it alone.
    let Some((span, mut rec)) = record_in_text(&now) else { return };
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
    match span {
        Some(span) => write_at(s, span, &rec),
        None => write(s, &rec),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    thread_local! {
        /// Stored specs read whole ([`stored`]) on this thread.
        pub(super) static SPEC_READS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    }

    /// Q6567: a command that changes built elements adds its step to the model's record without
    /// reading and writing the whole stored spec (tens of megabytes on a whole job; a move or
    /// erase of one built element took 100-150 ms on a large model). The spec is read whole once
    /// per copy of its text the session didn't write itself (the first edit, the edit after an
    /// undo), and the text each step leaves is the spec as writing it whole makes it.
    #[test]
    fn edits_of_built_elements_add_steps_without_reading_the_whole_spec() {
        let levels: Vec<Value> =
            (0..300).map(|i| json!({"name": format!("Level {i}"), "elevation_ft": i * 10, "height_ft": 10, "ceiling_ft": 8})).collect();
        let mut sp = BuildingSpec::from_json(&json!({"version": 1, "units_per_foot": 12, "levels": levels}).to_string()).unwrap();
        let mut s = Session::new();
        let mut handles = Vec::new();
        {
            let d = s.doc_mut().unwrap();
            for i in 0..8 {
                let mut common = cadaddy_doc::Common { layer: "3D-Devices".into(), ..Default::default() };
                common.xdata = vec![
                    cadaddy_doc::RawTag { code: 1001, value: wallgen::APP.into() },
                    cadaddy_doc::RawTag { code: 1000, value: "device".into() },
                    cadaddy_doc::RawTag { code: 1000, value: "Level 1".into() },
                    cadaddy_doc::RawTag { code: 1000, value: "0".into() },
                ]
                .into();
                let p = cadaddy_geom::Vec3::new(f64::from(i) * 24.0, 0.0, 0.0);
                handles
                    .push(d.add(&cadaddy_doc::Space::Model, common, cadaddy_doc::EntityKind::Point(cadaddy_doc::Point { p, angle: 0.0 })).unwrap());
            }
            let rec = BuildRecord { built: counts(d), now: counts(d), engine: "test".into(), ..BuildRecord::default() };
            sp.kept.insert(KEY.into(), serde_json::to_value(&rec).unwrap());
            d.building_spec = Some(sp.to_json()).into();
        }
        let hex = |h: &cadaddy_doc::Handle| h.hex();
        let steps = |s: &Session| read(s.doc().unwrap()).unwrap().steps;
        SPEC_READS.with(|n| n.set(0));
        for h in handles.iter().take(3) {
            s.execute("erase", &json!({"handles": [hex(h)]})).unwrap();
        }
        for h in handles.iter().skip(3).take(2) {
            s.execute("move", &json!({"handles": [hex(h)], "delta": [6, 0]})).unwrap();
        }
        let reads = SPEC_READS.with(std::cell::Cell::get);
        assert!(reads <= 1, "five edits read the whole spec {reads} times");
        let st = steps(&s);
        assert_eq!(st.len(), 5, "{st:?}");
        assert_eq!(st.iter().map(|x| x.by.as_str()).collect::<Vec<_>>(), ["erase", "erase", "erase", "move", "move"]);
        assert_eq!(st[2].changed, BTreeMap::from([("Level 1|device".to_string(), -1)]));
        assert_eq!(read(s.doc().unwrap()).unwrap().now, BTreeMap::from([("Level 1|device".to_string(), 5)]));
        // The text is the spec as written whole, byte for byte.
        let text = s.doc().unwrap().building_spec.as_deref().unwrap().to_string();
        assert_eq!(BuildingSpec::from_json(&text).unwrap().to_json(), text);
        // Undone, the drawing's spec is the one before the last step; the next edit adds its step
        // to that one.
        s.execute("undo", &json!({})).unwrap();
        assert_eq!(steps(&s).len(), 4);
        SPEC_READS.with(|n| n.set(0));
        s.execute("erase", &json!({"handles": [hex(&handles[6])]})).unwrap();
        s.execute("erase", &json!({"handles": [hex(&handles[7])]})).unwrap();
        assert!(SPEC_READS.with(std::cell::Cell::get) <= 1);
        let st = steps(&s);
        assert_eq!(st.len(), 6, "{st:?}");
        assert_eq!(read(s.doc().unwrap()).unwrap().now, BTreeMap::from([("Level 1|device".to_string(), 3)]));
    }

    #[test]
    fn a_top_level_member_is_found_by_a_scan() {
        let t = r#"{"a":"x}\"{","b":[1,{"build_record":2}],"n":-1.5e3,"build_record":{"s":"]}","v":[[]]},"z":null}"#;
        let span = top_level_value(t, "build_record").unwrap();
        assert_eq!(&t[span], r#"{"s":"]}","v":[[]]}"#);
        assert_eq!(&t[top_level_value(t, "n").unwrap()], "-1.5e3");
        assert_eq!(&t[top_level_value(t, "z").unwrap()], "null");
        assert_eq!(&t[top_level_value(t, "a").unwrap()], r#""x}\"{""#);
        let spaced = "{ \"k\" : [ 1 , 2 ] , \"build_record\" : { } }";
        assert_eq!(&spaced[top_level_value(spaced, "build_record").unwrap()], "{ }");
        assert!(top_level_value(t, "missing").is_none());
        assert!(top_level_value("[1]", "a").is_none());
        assert!(top_level_value(r#"{"a":{"b":1"#, "a").is_none());
        assert!(top_level_value("", "a").is_none());
        assert!(top_level_value("{}", "a").is_none());
    }

    /// Q6620: `model.build {parts: [hoistway]}` takes the shaft's walls, pit and overrun.
    #[test]
    fn a_hoistways_parts_answer_to_hoistway() {
        let el = |tags: &[&str]| {
            let mut e = Entity::new(
                cadaddy_doc::Handle(0x10),
                cadaddy_doc::EntityKind::Point(cadaddy_doc::Point { p: cadaddy_geom::Vec3::default(), angle: 0.0 }),
            );
            let mut x = vec![cadaddy_doc::RawTag { code: 1001, value: wallgen::APP.into() }];
            x.extend(tags.iter().map(|t| cadaddy_doc::RawTag { code: 1000, value: (*t).to_string() }));
            e.common.xdata = x.into();
            e
        };
        for tags in [
            &[super::super::hoistway::PIT, "Level 1", "0"][..],
            &[super::super::hoistway::OVERRUN, "Roof", "0"],
            &["wall", "Level 2", "0", "hoistway:ELEV 1"],
        ] {
            assert!(part_names(&el(tags)).contains(&"hoistway".to_string()), "{tags:?}");
        }
        assert!(!part_names(&el(&["wall", "Level 2", "0"])).contains(&"hoistway".to_string()));
        assert!(part_names(&el(&["wall", "Level 2", "0"])).contains(&"wall".to_string()));
    }

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
