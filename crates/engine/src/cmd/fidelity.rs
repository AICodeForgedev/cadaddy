//! Fidelity layers for the 3D view (Q6163): the built model shown like a map's zoom levels, not
//! every stick and coupling at every scale.
//!
//! Each built element gets a tier ([`Tier`]) from the rules data (`fidelity.json`, edited per
//! project with `view.fidelity.rules` and kept in the building spec): what it is (its kind,
//! trade, category, build part, words of its type and name) and how big (its size tag, its
//! largest side). Building: the shell, slabs, roof, structure, big gear and rooftop units, main
//! ducts and pipes. Floor: walls and openings, branch ducts, mid-size pipe, conduit runs,
//! panels, terminals. Room: devices, fixtures, luminaires, boxes, raceway and cable pieces,
//! framing and sheets. Detail: fittings, supports, fasteners, wire.
//!
//! The 3D view ([`cadaddy_render::model3::build_fid`]) draws the tiers its zoom calls for (Auto,
//! with hysteresis; or one fixed by `view.fidelity {level}`), streams the finer tiers' pieces in
//! by grid cell as a view needs them and lets them go when it zooms out, draws repeated pieces as
//! instances, and shows a run whose pieces are finer than the view as one line. A click on a
//! piece the view doesn't draw picks its whole run. `view.fidelity {show: {kind, level}}` marks
//! every element of a kind at any zoom.
//!
//! Fidelity only changes what is drawn: counts, the takeoff, Validate and `bim.element` read the
//! drawing, never the drawn subset.
//!
//! - `view.fidelity {level?, show?, of?}`: the view's fidelity, an element's tier.
//! - `view.fidelity.rules {rules?, auto?, default_tier?, reset?}`: the project's rules.
//! - `view.fidelity.report {width?, height?, room?}`: what each tier holds and draws, measured.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::AtomicUsize;

use cadaddy_doc::{Drawing, Entity, EntityKind, Handle, Space};
use cadaddy_geom::Vec3;
use cadaddy_render::model3::{self, AutoTiers, ElemClass, Force, Held, Model3, Stream, Tier};
use cadaddy_render::shade::ShadeView;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::*;
use crate::{Result, Session};

/// The built-in rules.
const RULES: &str = include_str!("fidelity.json");
/// The key of the project's rules in the building spec.
const KEY: &str = "fidelity";
/// Most objects looked at, rules kept, elements a click picks.
const MAX_SCAN: usize = 2_000_000;
const MAX_RULES: usize = 500;
const MAX_PICK: usize = 200_000;
/// Most handles `of` classifies.
const MAX_OF: usize = 1_000;
/// Milliseconds a frame of the 3D view spends streaming cells in (as the view does).
const FRAME_STREAM_MS: f64 = 12.0;
/// Bytes a vertex takes in the 3D view's GPU buffers (for the report's estimate).
const GPU_VERTEX: usize = 68;

const CMD: &str = "view.fidelity";
const RULES_CMD: &str = "view.fidelity.rules";
const REPORT: &str = "view.fidelity.report";

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new(CMD, "Model Fidelity", run_view)
            .menu(&["View", "Model Fidelity"])
            .params("{level?: auto|building|floor|room|detail (what the 3D view draws: auto follows the zoom), show?: {kind, level?} | kind | null (mark every element of a kind, on one level, at any zoom; null clears), of?: handle | [handle] (the tier of these elements)} → {level, shown (the tier the 3D view drew last), show, auto: {building_ft, floor_ft, room_ft, margin}, rules: {source, count}, elements?: [{handle, tier, kind, level, run, line_from, rule}]}: the 3D view's fidelity layers; counts and the takeoff never depend on it")
            .noundo(),
        CommandSpec::new(RULES_CMD, "Model Fidelity Rules", run_rules).params(
            "{rules?: [{kinds?, trades?, categories?, parts?, words?, atoms?: bool, min_size_in?, max_size_in?, min_extent_ft?, max_extent_ft?, tier: building|floor|room|detail, line_from?: tier (below its tier its run draws as a line from this one), why?}] (first match wins; replaces the project's), auto?: {building_ft?, floor_ft?, room_ft?, margin?} (view heights at which Auto changes tier, and the hysteresis), default_tier?, reset?: bool (back to the built-in rules)} → {source: project|default, auto, default_tier, rules}: which tier each element draws at, kept in the drawing",
        ),
        CommandSpec::new(REPORT, "Model Fidelity Report", run_report)
            .params("{width?: px (1600), height?: px (1000), room?: name or number (the room to zoom into, by its devices' room; default the most typical room: the commonest receptacle count), dir?: [x, y, z] (view direction; default from the south-west, above), compare?: bool (also build the model without fidelity and measure it)} → {elements, tiers: {tier: {elements, tris, base, instanced, streamed, streamed_tris}}, build_ms, model_mb, instances, prototypes, whole: {height_ft, auto, at: {tier: {tris_drawn, chunks, instances, stream_cells, stream_tris, held_mb, visible_ms, stream_ms, raster_ms}}}, room: {height_ft, auto, receptacles, receptacles_drawn_first_frame, receptacles_drawn, first_frame_ms, all_streamed_ms, frames_to_stream, stream_cells}, everything_tris, without?: {build_ms, model_mb, tris_drawn, visible_ms, raster_ms, gpu_mb_est}}: what each fidelity tier holds and draws on this model, measured")
            .noundo(),
    ]
}

// ─── Rules ───────────────────────────────────────────────────────────────────

/// One rule: every condition it gives must hold; the first rule that holds sets the tier.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Rule {
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub kinds: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub trades: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub categories: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub parts: Vec<String>,
    /// Any of these in the element's kind, category, type or name: a phrase (with a space) as
    /// text, a word as a whole word (five letters or more: its start, so plurals count).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub words: Vec<String>,
    /// Only pieces (atoms), or only not.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub atoms: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub min_size_in: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_size_in: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub min_extent_ft: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_extent_ft: Option<f64>,
    pub tier: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub line_from: Option<String>,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub why: String,
}

/// Auto's thresholds.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Auto {
    pub building_ft: f64,
    pub floor_ft: f64,
    pub room_ft: f64,
    pub margin: f64,
}

impl Default for Auto {
    fn default() -> Self {
        let a = AutoTiers::default();
        Auto { building_ft: a.building_ft, floor_ft: a.floor_ft, room_ft: a.room_ft, margin: a.margin }
    }
}

impl Auto {
    pub fn tiers(&self) -> AutoTiers {
        let ok = |v: f64, d: f64| if v.is_finite() && v > 0.0 { v } else { d };
        let d = AutoTiers::default();
        let building_ft = ok(self.building_ft, d.building_ft);
        let floor_ft = ok(self.floor_ft, d.floor_ft).min(building_ft);
        let room_ft = ok(self.room_ft, d.room_ft).min(floor_ft);
        AutoTiers { building_ft, floor_ft, room_ft, margin: if self.margin.is_finite() { self.margin.clamp(0.0, 0.9) } else { d.margin } }
    }
}

/// The rules: the built-in ones, or the project's.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Rules {
    pub version: u32,
    pub source: String,
    pub auto: Auto,
    pub default_tier: String,
    pub rules: Vec<Rule>,
}

impl Rules {
    /// The built-in rules.
    pub fn defaults() -> Rules {
        serde_json::from_str::<Rules>(RULES).unwrap_or_else(|_| Rules { default_tier: "floor".into(), ..Rules::default() })
    }

    /// The rules for `d`: the project's (in its building spec) over the built-in ones; whether
    /// the project has its own.
    pub fn of(d: &Drawing) -> (Rules, bool) {
        let mut r = Rules::defaults();
        let Some(own) = project(d) else { return (r, false) };
        if let Some(list) = own.get("rules").and_then(|v| serde_json::from_value::<Vec<Rule>>(v.clone()).ok()) {
            r.rules = list.into_iter().take(MAX_RULES).collect();
        }
        if let Some(a) = own.get("auto").and_then(|v| serde_json::from_value::<Auto>(v.clone()).ok()) {
            r.auto = a;
        }
        if let Some(t) = own.get("default_tier").and_then(Value::as_str).filter(|t| Tier::parse(t).is_some()) {
            r.default_tier = t.to_string();
        }
        r.source = "project".into();
        (r, true)
    }

    fn default_tier(&self) -> Tier {
        Tier::parse(&self.default_tier).unwrap_or(Tier::Floor)
    }

    /// The tier of an element with `f`, its run line's tier, and the rule that said (none: the
    /// default).
    pub fn class(&self, f: &Facts) -> (Tier, Option<Tier>, Option<usize>) {
        for (i, r) in self.rules.iter().enumerate() {
            if r.matches(f) {
                let tier = Tier::parse(&r.tier).unwrap_or_else(|| self.default_tier());
                let line = r.line_from.as_deref().and_then(Tier::parse).filter(|l| *l < tier);
                return (tier, line, Some(i));
            }
        }
        (self.default_tier(), None, None)
    }
}

/// The project's rules object in the building spec.
fn project(d: &Drawing) -> Option<Value> {
    d.building_spec.as_deref().and_then(|t| serde_json::from_str::<Value>(t).ok()).and_then(|v| v.get(KEY).cloned()).filter(Value::is_object)
}

/// What the rules see of an element.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Facts {
    pub kind: String,
    pub atom: bool,
    pub trade: String,
    pub category: String,
    pub part: String,
    /// Kind, category, type and name, lower case.
    pub text: String,
    /// Its size tag's larger side, inches.
    pub size_in: Option<f64>,
    /// Its largest side, feet.
    pub extent_ft: Option<f64>,
    pub level: String,
    /// The run it is a piece of (its level and run id).
    pub run: Option<String>,
}

fn any_eq(list: &[String], v: &str) -> bool {
    list.iter().any(|x| x.trim().eq_ignore_ascii_case(v.trim()))
}

/// Whether `word` is in `text`: a phrase as text; a short word as a whole word, a long one as a
/// word's start.
fn has_word(text: &str, word: &str) -> bool {
    let w = word.trim().to_ascii_lowercase();
    if w.is_empty() {
        return false;
    }
    if w.contains(' ') || w.contains('-') {
        return text.contains(&w);
    }
    text.split(|c: char| !c.is_ascii_alphanumeric()).any(|t| t == w || (w.len() >= 5 && t.starts_with(&w)))
}

fn within(v: Option<f64>, lo: Option<f64>, hi: Option<f64>) -> bool {
    if lo.is_none() && hi.is_none() {
        return true;
    }
    let Some(v) = v.filter(|v| v.is_finite()) else { return false };
    lo.is_none_or(|l| v >= l) && hi.is_none_or(|h| v <= h)
}

impl Rule {
    pub fn matches(&self, f: &Facts) -> bool {
        (self.kinds.is_empty() || any_eq(&self.kinds, &f.kind))
            && (self.trades.is_empty() || any_eq(&self.trades, &f.trade))
            && (self.categories.is_empty() || any_eq(&self.categories, &f.category))
            && (self.parts.is_empty() || any_eq(&self.parts, &f.part))
            && (self.words.is_empty() || self.words.iter().any(|w| has_word(&f.text, w)))
            && self.atoms.is_none_or(|a| a == f.atom)
            && within(f.size_in, self.min_size_in, self.max_size_in)
            && within(f.extent_ft, self.min_extent_ft, self.max_extent_ft)
    }
}

/// What `e` of `d` is for the rules (`None`: not a built element). `upf`: drawing units per foot.
pub fn facts(d: &Drawing, e: &Entity, upf: f64) -> Option<Facts> {
    let part = super::wallgen::made_as(e)?.to_ascii_lowercase();
    let class = super::bimlook::classify(d, e)?;
    let props = cadaddy_doc::props::read(&e.common.xdata);
    let prop = |k: &str| props.get(k).map(|p| p.text()).filter(|t| !t.trim().is_empty());
    let made = |k: &str| super::bimlook::made_tag(e, k).map(str::to_string).filter(|t| !t.trim().is_empty());
    let atom_kind = prop("atom").or_else(|| made("part").and_then(|p| p.strip_prefix("atom:").map(str::to_string)));
    let kind = match &atom_kind {
        Some(k) => k.to_ascii_lowercase(),
        None => super::elemprops::element_kind(e).map(|k| super::elemprops::reported_kind(e, k)).unwrap_or_else(|| part.clone()).to_ascii_lowercase(),
    };
    let mut text = [
        Some(kind.clone()),
        Some(class.category.clone()),
        Some(class.ty.clone()),
        super::bimlook::made_name(e).map(str::to_string),
        made("tag"),
        made("kind"),
        prop("type"),
        prop("kind"),
    ]
    .into_iter()
    .flatten()
    .collect::<Vec<String>>()
    .join(" ");
    text.make_ascii_lowercase();
    let size_in = made("size").and_then(|z| size_inches(&z));
    let extent_ft = match &e.kind {
        EntityKind::Mesh(m) if !m.vertices.is_empty() => {
            let (mut lo, mut hi) = (Vec3::new(f64::MAX, f64::MAX, f64::MAX), Vec3::new(f64::MIN, f64::MIN, f64::MIN));
            for v in m.vertices.iter().filter(|v| v.is_finite()) {
                lo = Vec3::new(lo.x.min(v.x), lo.y.min(v.y), lo.z.min(v.z));
                hi = Vec3::new(hi.x.max(v.x), hi.y.max(v.y), hi.z.max(v.z));
            }
            let s = hi - lo;
            let m = s.x.max(s.y).max(s.z) / upf;
            (m.is_finite() && m >= 0.0).then_some(m)
        }
        _ => None,
    };
    let run = prop("run").or_else(|| made("mepid").and_then(|id| id.rsplit_once('/').map(|(r, _)| r.to_string()))).filter(|r| !r.trim().is_empty());
    let run = run.map(|r| format!("{}|{r}", class.level));
    Some(Facts {
        kind,
        atom: atom_kind.is_some(),
        trade: class.key.to_string(),
        category: class.category.to_ascii_lowercase(),
        part,
        text,
        size_in,
        extent_ft,
        level: class.level,
        run,
    })
}

/// A size tag's larger side in inches: `24x12`, `3/4"`, `1 1/2"`, `4" square`, or a bare number
/// or fraction (the build writes a round run's size as `2`, `0.75`).
fn size_inches(t: &str) -> Option<f64> {
    let v = super::mep::size_tag(t).map(|[a, b]| a.max(b)).or_else(|| {
        let s = t.trim().trim_end_matches('"').trim();
        match s.split_once('/') {
            Some((a, b)) => {
                let (whole, a) = match a.trim().rsplit_once(' ') {
                    Some((w, a)) => (w.trim().parse::<f64>().ok()?, a),
                    None => (0.0, a),
                };
                Some(whole + a.trim().parse::<f64>().ok()? / b.trim().parse::<f64>().ok().filter(|b| *b != 0.0)?)
            }
            None => s.parse::<f64>().ok(),
        }
    })?;
    (v.is_finite() && v > 0.0).then_some(v)
}

/// The classifier the 3D view builds `d`'s model with (the rules of `d`).
pub fn classifier(d: &Drawing) -> impl Fn(&Drawing, &Entity) -> Option<ElemClass> + Sync + use<> {
    let rules = Rules::of(d).0;
    let upf = super::elemprops::units_per_foot(d);
    move |d: &Drawing, e: &Entity| {
        let f = facts(d, e, upf)?;
        let (tier, line_from, _) = rules.class(&f);
        Some(ElemClass { tier, kind: f.kind, level: f.level, group: f.run, line_from })
    }
}

/// The units per foot of `d` (the building spec's, else inches).
pub fn units_per_foot(d: &Drawing) -> f64 {
    super::elemprops::units_per_foot(d)
}

// ─── The view's state ────────────────────────────────────────────────────────

/// The 3D view's fidelity, per open drawing (not saved with it).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct View {
    /// A tier fixed by the user (`None`: Auto, by the zoom).
    pub mode: Option<Tier>,
    /// The tier the 3D view drew last (the view reports it; a fixed mode sets it).
    pub shown: Option<Tier>,
    /// Elements marked at any zoom.
    pub force: Option<Force>,
}

/// What a click on `h` picks: its whole run when the 3D view is showing a tier coarser than the
/// piece (it draws the run as a line, not the piece); else `h`.
pub fn pick(s: &Session, h: Handle) -> Vec<Handle> {
    let Ok(st) = s.state() else { return vec![h] };
    if st.visual_style == cadaddy_render::shade::VisualStyle::Wireframe2d {
        return vec![h];
    }
    let Some(shown) = st.fidelity.mode.or(st.fidelity.shown) else { return vec![h] };
    let d = &st.doc;
    let Some(e) = d.model.get(h) else { return vec![h] };
    let rules = Rules::of(d).0;
    let upf = units_per_foot(d);
    let Some(f) = facts(d, e, upf) else { return vec![h] };
    let (tier, _, _) = rules.class(&f);
    let Some(run) = f.run.filter(|_| tier > shown) else { return vec![h] };
    let mut out = Vec::new();
    for x in d.model.iter().take(MAX_SCAN) {
        if out.len() >= MAX_PICK {
            break;
        }
        // Cheap first: only pieces with a run id of their own.
        if super::bimlook::made_tag(x, "mepid").is_none() && cadaddy_doc::props::read(&x.common.xdata).get("run").is_none() {
            continue;
        }
        if facts(d, x, upf).and_then(|g| g.run).as_deref() == Some(run.as_str()) {
            out.push(x.handle);
        }
    }
    if out.is_empty() { vec![h] } else { out }
}

fn tier_param(cmd: &str, v: &Value) -> Result<Option<Tier>> {
    match v.as_str().map(str::trim) {
        Some(t) if t.eq_ignore_ascii_case("auto") => Ok(None),
        Some(t) => Tier::parse(t).map(Some).ok_or_else(|| bad(cmd, format!("`level` must be auto, building, floor, room or detail, got {t}"))),
        None => Err(bad(cmd, format!("`level` must be auto, building, floor, room or detail, got {v}"))),
    }
}

fn handle_of(v: &Value) -> Option<Handle> {
    let t = v.as_str()?.trim();
    u64::from_str_radix(t.trim_start_matches("0x"), 16).ok().filter(|h| *h != 0).map(Handle)
}

fn view_json(s: &Session) -> Result<Value> {
    let st = s.state()?;
    let (rules, own) = Rules::of(&st.doc);
    let f = &st.fidelity;
    Ok(json!({
        "level": f.mode.map_or("auto", Tier::name),
        "shown": f.shown.map(Tier::name),
        "show": f.force.as_ref().map(|x| json!({"kind": x.kind, "level": x.level})),
        "auto": rules.auto.tiers().as_json(),
        "rules": {"source": if own { "project" } else { "default" }, "count": rules.rules.len()},
    }))
}

trait AsJson {
    fn as_json(&self) -> Value;
}

impl AsJson for AutoTiers {
    fn as_json(&self) -> Value {
        json!({"building_ft": self.building_ft, "floor_ft": self.floor_ft, "room_ft": self.room_ft, "margin": self.margin})
    }
}

fn run_view(s: &mut Session, p: &Value) -> Result<Value> {
    if let Some(v) = p.get("level").filter(|v| !v.is_null()) {
        let mode = tier_param(CMD, v)?;
        let st = s.state_mut()?;
        st.fidelity.mode = mode;
        if mode.is_some() {
            st.fidelity.shown = mode;
        }
        s.touch();
    }
    match p.get("show") {
        None => {}
        Some(Value::Null) | Some(Value::Bool(false)) => {
            s.state_mut()?.fidelity.force = None;
            s.touch();
        }
        Some(Value::String(k)) if !k.trim().is_empty() => {
            s.state_mut()?.fidelity.force = Some(Force { kind: k.trim().to_string(), level: None });
            s.touch();
        }
        Some(Value::Object(o)) => {
            let kind =
                o.get("kind").and_then(Value::as_str).map(str::trim).filter(|k| !k.is_empty()).ok_or_else(|| bad(CMD, "`show` needs a `kind`"))?;
            let level = o.get("level").and_then(Value::as_str).map(str::trim).filter(|l| !l.is_empty()).map(str::to_string);
            s.state_mut()?.fidelity.force = Some(Force { kind: kind.to_string(), level });
            s.touch();
        }
        Some(v) => return Err(bad(CMD, format!("`show` must be {{kind, level?}}, a kind or null, got {v}"))),
    }
    let mut out = view_json(s)?;
    if let Some(of) = p.get("of").filter(|v| !v.is_null()) {
        let list: Vec<Handle> = match of {
            Value::Array(a) => a.iter().take(MAX_OF).filter_map(handle_of).collect(),
            v => handle_of(v).into_iter().collect(),
        };
        if list.is_empty() {
            return Err(bad(CMD, "`of` must be a handle or a list of handles"));
        }
        let d = s.doc()?;
        let rules = Rules::of(d).0;
        let upf = units_per_foot(d);
        let mut els = Vec::new();
        for h in list {
            let Some(e) = d.model.get(h) else {
                els.push(json!({"handle": format!("{:X}", h.0), "error": "no such object"}));
                continue;
            };
            match facts(d, e, upf) {
                Some(f) => {
                    let (tier, line, rule) = rules.class(&f);
                    els.push(json!({
                        "handle": format!("{:X}", h.0), "tier": tier.name(), "kind": f.kind, "level": f.level,
                        "run": f.run, "line_from": line.map(Tier::name), "size_in": f.size_in, "extent_ft": f.extent_ft,
                        "rule": rule.and_then(|i| rules.rules.get(i)).map(|r| if r.why.is_empty() { r.tier.clone() } else { r.why.clone() }),
                    }));
                }
                None => els.push(json!({"handle": format!("{:X}", h.0), "tier": Value::Null, "note": "not a built element: always drawn"})),
            }
        }
        if let Some(o) = out.as_object_mut() {
            o.insert("elements".into(), Value::Array(els));
        }
    }
    Ok(out)
}

fn rules_json(d: &Drawing) -> Value {
    let (r, own) = Rules::of(d);
    json!({"source": if own { "project" } else { "default" }, "auto": r.auto.tiers().as_json(), "default_tier": r.default_tier().name(), "rules": r.rules})
}

fn run_rules(s: &mut Session, p: &Value) -> Result<Value> {
    let reset = p.get("reset").and_then(Value::as_bool) == Some(true);
    let mut own = if reset { None } else { project(s.doc()?) };
    let mut changed = reset;
    if let Some(v) = p.get("rules").filter(|v| !v.is_null()) {
        let list: Vec<Rule> = serde_json::from_value(v.clone()).map_err(|e| bad(RULES_CMD, format!("`rules`: {e}")))?;
        if list.len() > MAX_RULES {
            return Err(bad(RULES_CMD, format!("at most {MAX_RULES} rules")));
        }
        for (i, r) in list.iter().enumerate() {
            if Tier::parse(&r.tier).is_none() {
                return Err(bad(RULES_CMD, format!("rule {}: `tier` must be building, floor, room or detail, got {:?}", i + 1, r.tier)));
            }
            if let Some(l) = &r.line_from
                && Tier::parse(l).is_none()
            {
                return Err(bad(RULES_CMD, format!("rule {}: `line_from` must be a tier, got {l:?}", i + 1)));
            }
        }
        own.get_or_insert_with(|| json!({}))["rules"] = serde_json::to_value(&list).unwrap_or(Value::Null);
        changed = true;
    }
    if let Some(v) = p.get("auto").filter(|v| !v.is_null()) {
        let o = v.as_object().ok_or_else(|| bad(RULES_CMD, "`auto` must be {building_ft?, floor_ft?, room_ft?, margin?}"))?;
        let mut a = own
            .as_ref()
            .and_then(|x| x.get("auto"))
            .and_then(|x| serde_json::from_value::<Auto>(x.clone()).ok())
            .unwrap_or_else(|| Rules::defaults().auto);
        for (k, slot) in [("building_ft", &mut a.building_ft), ("floor_ft", &mut a.floor_ft), ("room_ft", &mut a.room_ft), ("margin", &mut a.margin)]
        {
            if let Some(x) = o.get(k).filter(|x| !x.is_null()) {
                *slot = x
                    .as_f64()
                    .filter(|x| x.is_finite() && *x >= 0.0)
                    .ok_or_else(|| bad(RULES_CMD, format!("`auto.{k}` must be a number of 0 or more")))?;
            }
        }
        own.get_or_insert_with(|| json!({}))["auto"] = serde_json::to_value(a).unwrap_or(Value::Null);
        changed = true;
    }
    if let Some(v) = p.get("default_tier").filter(|v| !v.is_null()) {
        let t = v.as_str().and_then(Tier::parse).ok_or_else(|| bad(RULES_CMD, "`default_tier` must be building, floor, room or detail"))?;
        own.get_or_insert_with(|| json!({}))["default_tier"] = json!(t.name());
        changed = true;
    }
    if changed {
        let d = s.doc()?;
        let mut spec: Value = d.building_spec.as_deref().and_then(|t| serde_json::from_str(t).ok()).unwrap_or(json!({}));
        if let Some(o) = spec.as_object_mut() {
            match own {
                Some(v) => {
                    o.insert(KEY.into(), v);
                }
                None => {
                    o.remove(KEY);
                }
            }
        }
        s.doc_mut()?.building_spec = Some(spec.to_string());
    }
    Ok(rules_json(s.doc()?))
}

// ─── The report ──────────────────────────────────────────────────────────────

fn ms(t: web_time::Instant) -> f64 {
    (t.elapsed().as_secs_f64() * 1e6).round() / 1e3
}

fn mb(bytes: usize) -> f64 {
    (bytes as f64 / 1048576.0 * 10.0).round() / 10.0
}

/// A box's plan area.
fn area(b: &model3::Aabb) -> f64 {
    if b.is_empty() { 0.0 } else { (b.hi.x - b.lo.x) * (b.hi.y - b.lo.y) }
}

/// Receptacles: elements of a receptacle (or outlet) kind.
fn receptacle(kind: &str) -> bool {
    let k = kind.to_ascii_lowercase();
    k.contains("receptacle") || k.contains("outlet")
}

/// Whether element `i` is drawn by a view at `tier` given what it sees (`vis`) and streamed.
fn drawn(m: &Model3, st: &Stream, vis: &model3::Visible, v: &ShadeView, i: usize) -> bool {
    let Some(f) = &m.fid else { return false };
    let Some(e) = f.elems.get(i) else { return false };
    if e.tier > vis.tier || !model3::in_view(&e.bounds, v) {
        return false;
    }
    match e.held {
        Held::Base => {
            vis.chunks.iter().filter_map(|c| m.chunks.get(*c)).any(|c| m.base.owners.get(c.tris.clone()).is_some_and(|o| o.contains(&e.handle)))
        }
        Held::Instance => vis.instances.iter().any(|(_, _, list)| {
            list.iter().filter_map(|k| m.instances.get(*k as usize)).any(|x| (x.bounds.center() - e.bounds.center()).len() < 1e-6)
        }),
        Held::Streamed => {
            let cell = f.grids.get(e.tier.index()).map(|g| g.cell_of(e.bounds.center()));
            cell.is_some_and(|c| st.cells.contains_key(&(e.tier.index() as u8, c as u32)))
        }
        Held::Outside => false,
    }
}

fn run_report(s: &mut Session, p: &Value) -> Result<Value> {
    let doc = s.doc()?;
    let width = p.get("width").and_then(Value::as_u64).unwrap_or(1600).clamp(64, 8192) as u32;
    let height = p.get("height").and_then(Value::as_u64).unwrap_or(1000).clamp(64, 8192) as u32;
    let dir = match p.get("dir").and_then(Value::as_array) {
        Some(a) => {
            let n: Vec<f64> = a.iter().filter_map(Value::as_f64).collect();
            match n[..] {
                [x, y, z] if [x, y, z].iter().all(|c| c.is_finite()) => Vec3::new(x, y, z),
                _ => return Err(bad(REPORT, "`dir` must be [x, y, z]")),
            }
        }
        None => Vec3::new(-1.0, -1.0, 1.0),
    };
    let proj = cadaddy_geom::view3::Projection::new(dir);
    // As the 3D view builds it: the BIM view's drawing (else the drawing itself).
    let md = Arc::new(super::bimlook::display(doc).unwrap_or_else(|| doc.clone()));
    let upf = units_per_foot(&md);
    let ext = md.extents(&Space::Model);
    let size = ext.width().max(ext.height());
    let tol = if size.is_finite() && size > 0.0 { size / 1500.0 } else { 1.0 };
    let bg = cadaddy_color::Rgb(255, 255, 255);
    let (rules, _) = Rules::of(&md);
    let classify = classifier(&md);
    let (progress, total) = (AtomicUsize::new(0), AtomicUsize::new(0));
    let t0 = web_time::Instant::now();
    let m = model3::build_fid(Arc::clone(&md), &Space::Model, tol, bg, &progress, &total, &[], &classify, model3::FidOptions::default());
    let build_ms = ms(t0);
    let Some(f) = m.fid.clone() else { return Err(bad(REPORT, "the model has no elements")) };
    let per = f.per_tier();
    let mut tiers = serde_json::Map::new();
    for t in Tier::ALL {
        let c = per.get(t.index()).copied().unwrap_or_default();
        tiers.insert(
            t.name().into(),
            json!({"elements": c.elements, "tris": c.tris, "base": c.base, "instanced": c.instanced, "streamed": c.streamed, "streamed_tris": c.streamed_tris, "outside": c.outside}),
        );
    }
    let everything: usize = m.total_tris() + per.iter().map(|c| c.streamed_tris).sum::<usize>();
    let auto = rules.auto.tiers();
    // One view at each tier: what it draws, holds and costs.
    let at = |v: &ShadeView, tier: Tier| -> (Value, Stream) {
        let mut st = Stream::default();
        let t1 = web_time::Instant::now();
        let step = st.update(&m, v, tier, f64::INFINITY);
        let stream_ms = ms(t1);
        let t2 = web_time::Instant::now();
        let vis = m.visible_at(v, tier, None);
        let visible_ms = ms(t2);
        let stream_tris: usize = st.visible(v).iter().filter_map(|k| st.cells.get(k)).map(|c| c.scene.tris.len()).sum();
        // The CPU rasterizer drawing it (a proxy for the frame's load), small.
        let t3 = web_time::Instant::now();
        let mut flat = m.flatten_visible(&vis);
        for k in st.visible(v) {
            if let Some(c) = st.cells.get(&k) {
                flat.tris.extend_from_slice(&c.scene.tris);
                flat.lines.extend_from_slice(&c.scene.lines);
            }
        }
        let small = ShadeView { width_px: 640, height_px: 400, ..*v };
        let _ = cadaddy_render::shade::rasterize(&flat, &small, cadaddy_render::shade::VisualStyle::ShadedEdges, bg);
        let raster_ms = ms(t3);
        let (lt, ll) = m.loaded_at(tier);
        let gpu = (lt + st.tris()) * 3 * GPU_VERTEX + (ll + st.cells.values().map(|c| c.scene.lines.len()).sum::<usize>()) * 2 * GPU_VERTEX;
        let out = json!({
            "tris_drawn": vis.tris + stream_tris,
            "gpu_mb_est": mb(gpu),
            "chunks": vis.chunks.len(),
            "instances": vis.instances.iter().map(|g| g.2.len()).sum::<usize>(),
            "stream_cells": st.cells.len(),
            "stream_added": step.added,
            "stream_tris": stream_tris,
            "held_mb": mb(m.bytes() + st.bytes),
            "visible_ms": visible_ms,
            "stream_ms": stream_ms,
            "raster_ms": raster_ms,
        });
        (out, st)
    };
    let whole_view = cadaddy_render::shade::fit_bounds(&m.bounds, proj, width, height).ok_or_else(|| bad(REPORT, "the model is empty"))?;
    let whole_ft = whole_view.height / upf;
    let mut whole_at = serde_json::Map::new();
    for t in Tier::ALL {
        whole_at.insert(t.name().into(), at(&whole_view, t).0);
    }
    // A room: its receptacles grouped by the room they say they are in (on their level); the one
    // named, else the commonest kind of room (the receptacle count most rooms have).
    let want = p.get("room").and_then(Value::as_str).map(|r| r.trim().to_ascii_lowercase()).filter(|r| !r.is_empty());
    let recs: Vec<usize> = f.elems.iter().enumerate().filter(|(_, e)| receptacle(f.kind_of(e))).map(|(i, _)| i).collect();
    let mut rooms: BTreeMap<(String, String), (Vec<usize>, model3::Aabb)> = BTreeMap::new();
    for &i in &recs {
        let Some(e) = f.elems.get(i) else { continue };
        let Some(room) = md.model.get(e.handle).and_then(|x| cadaddy_doc::props::read(&x.common.xdata).get("room").map(|v| v.text())) else {
            continue;
        };
        let room = room.trim().to_ascii_lowercase();
        if room.is_empty() {
            continue;
        }
        let slot = rooms.entry((f.level_of(e).to_string(), room)).or_insert((Vec::new(), model3::Aabb::EMPTY));
        slot.0.push(i);
        slot.1.grow(e.bounds.lo);
        slot.1.grow(e.bounds.hi);
    }
    let pick_room = |list: &[(usize, model3::Aabb)]| list.iter().min_by(|a, b| area(&a.1).total_cmp(&area(&b.1))).copied();
    let best: Option<(usize, model3::Aabb)> = match &want {
        Some(w) => {
            let list: Vec<(usize, model3::Aabb)> =
                rooms.iter().filter(|((_, r), _)| r == w || r.split_whitespace().any(|x| x == w)).map(|(_, (v, b))| (v.len(), *b)).collect();
            pick_room(&list)
        }
        None => {
            let mut counts: BTreeMap<usize, usize> = BTreeMap::new();
            for (v, _) in rooms.values().filter(|(v, _)| v.len() >= 3) {
                *counts.entry(v.len()).or_insert(0) += 1;
            }
            let mode = counts.iter().max_by_key(|(n, c)| (**c, std::cmp::Reverse(**n))).map(|(n, _)| *n);
            let list: Vec<(usize, model3::Aabb)> = rooms.values().filter(|(v, _)| Some(v.len()) == mode).map(|(v, b)| (v.len(), *b)).collect();
            // The middle-sized one of them.
            let mut list = list;
            list.sort_by(|a, b| area(&a.1).total_cmp(&area(&b.1)));
            list.get(list.len() / 2).copied().or_else(|| pick_room(&list))
        }
    };
    let inside = |r: &model3::Aabb, e: &model3::Elem| {
        let c = e.bounds.center();
        c.x >= r.lo.x && c.x <= r.hi.x && c.y >= r.lo.y && c.y <= r.hi.y && c.z >= r.lo.z && c.z <= r.hi.z
    };
    let room = match best {
        Some((n, b)) => {
            // The room's box from its receptacles, out to its walls (about), floor to ceiling.
            let pad = Vec3::new(3.0 * upf, 3.0 * upf, 0.0);
            let b = model3::Aabb { lo: b.lo - pad - Vec3::new(0.0, 0.0, b.lo.z.min(1.5 * upf)), hi: b.hi + pad + Vec3::new(0.0, 0.0, 7.0 * upf) };
            let v = cadaddy_render::shade::fit_bounds(&b, proj, width, height).ok_or_else(|| bad(REPORT, "the room has no size"))?;
            let ft = v.height / upf;
            // Zoomed in from the whole building: Auto steps down to the room's tier.
            let tier = auto.tier(ft, Some(auto.tier(whole_ft, None)));
            // As the 3D view does it: the first frame streams for at most its budget (the
            // receptacles are instances, already loaded), the next frames bring the rest.
            let mut st = Stream::default();
            let t1 = web_time::Instant::now();
            let first = st.update(&m, &v, tier, FRAME_STREAM_MS);
            let first_vis = m.visible_at(&v, tier, None);
            let first_ms = ms(t1);
            let in_room: Vec<usize> = recs.iter().copied().filter(|i| f.elems.get(*i).is_some_and(|e| inside(&b, e))).collect();
            let drawn_first = in_room.iter().filter(|i| drawn(&m, &st, &first_vis, &v, **i)).count();
            let mut frames = 1;
            let mut pending = first.pending;
            while pending > 0 && frames < 10_000 {
                pending = st.update(&m, &v, tier, FRAME_STREAM_MS).pending;
                frames += 1;
            }
            let vis = m.visible_at(&v, tier, None);
            let all_ms = ms(t1);
            let shown = in_room.iter().filter(|i| drawn(&m, &st, &vis, &v, **i)).count();
            let (lt, ll) = m.loaded_at(tier);
            let gpu = (lt + st.tris()) * 3 * GPU_VERTEX + (ll + st.cells.values().map(|c| c.scene.lines.len()).sum::<usize>()) * 2 * GPU_VERTEX;
            json!({
                "height_ft": (ft * 10.0).round() / 10.0, "auto": tier.name(),
                "receptacles": n, "receptacles_in_view_box": in_room.len(), "receptacles_drawn_first_frame": drawn_first, "receptacles_drawn": shown,
                "first_frame_ms": first_ms, "all_streamed_ms": all_ms, "frames_to_stream": frames, "stream_cells": st.cells.len(),
                "tris_drawn": vis.tris + st.tris(), "held_mb": mb(m.bytes() + st.bytes), "gpu_mb_est": mb(gpu),
                "rooms_grouped": rooms.len(),
            })
        }
        None => Value::Null,
    };
    // The same model without fidelity, as the view built it before.
    let without = if p.get("compare").and_then(Value::as_bool) == Some(true) {
        let t0 = web_time::Instant::now();
        let plain = model3::build(&md, &Space::Model, tol, bg, &progress, &total);
        let build_ms = ms(t0);
        let t1 = web_time::Instant::now();
        let vis = plain.visible(&whole_view);
        let visible_ms = ms(t1);
        let t2 = web_time::Instant::now();
        let flat = plain.flatten_visible(&vis);
        let small = ShadeView { width_px: 640, height_px: 400, ..whole_view };
        let _ = cadaddy_render::shade::rasterize(&flat, &small, cadaddy_render::shade::VisualStyle::ShadedEdges, bg);
        let raster_ms = ms(t2);
        let (lt, ll) = plain.loaded_at(Tier::Detail);
        json!({"build_ms": build_ms, "model_mb": mb(plain.bytes()), "tris_drawn": vis.tris, "visible_ms": visible_ms, "raster_ms": raster_ms, "gpu_mb_est": mb(lt * 3 * GPU_VERTEX + ll * 2 * GPU_VERTEX), "instances": plain.instances.len()})
    } else {
        Value::Null
    };
    let kinds: BTreeMap<&str, usize> = f.elems.iter().fold(BTreeMap::new(), |mut acc, e| {
        *acc.entry(f.kind_of(e)).or_insert(0) += 1;
        acc
    });
    let mut by_tier_kind: BTreeMap<&str, BTreeMap<&str, usize>> = BTreeMap::new();
    for e in &f.elems {
        *by_tier_kind.entry(e.tier.name()).or_default().entry(f.kind_of(e)).or_insert(0) += 1;
    }
    Ok(json!({
        "elements": f.elems.len(),
        "tiers": tiers,
        "kinds": kinds.len(),
        "tier_kinds": by_tier_kind,
        "build_ms": build_ms,
        "model_mb": mb(m.bytes()),
        "base_tris": m.base.tris.len(),
        "instances": m.instances.len(),
        "prototypes": m.protos.len(),
        "everything_tris": everything,
        "whole": {"height_ft": (whole_ft * 10.0).round() / 10.0, "auto": auto.tier(whole_ft, None).name(), "at": whole_at},
        "room": room,
        "without": without,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes_and_words() {
        assert_eq!(size_inches("24x12"), Some(24.0));
        assert_eq!(size_inches("2"), Some(2.0));
        assert_eq!(size_inches("0.75"), Some(0.75));
        assert_eq!(size_inches("3/4\""), Some(0.75));
        assert_eq!(size_inches("1 1/2\""), Some(1.5));
        assert_eq!(size_inches("4\" square"), Some(4.0));
        assert_eq!(size_inches("#12"), None);
        assert_eq!(size_inches("1/0"), None);
        assert_eq!(size_inches("-3"), None);
        assert!(has_word("unit heater exhaust fan ef-2", "exhaust fan"));
        assert!(has_word("pad mounted transformers", "transformer"));
        assert!(has_word("ahu 3", "ahu"));
        assert!(!has_word("ahuja room", "ahu"));
        assert!(!has_word("anything", ""));
    }

    #[test]
    fn first_matching_rule_wins_and_sizes_bound_it() {
        let r = Rules::defaults();
        assert!(r.rules.len() > 10 && !r.source.is_empty());
        let pipe = |size: f64| Facts { kind: "pipe".into(), trade: "pipe_cold".into(), size_in: Some(size), ..Facts::default() };
        assert_eq!(r.class(&pipe(4.0)).0, Tier::Building);
        assert_eq!(r.class(&pipe(2.0)).0, Tier::Floor);
        let (t, line, _) = r.class(&pipe(0.75));
        assert_eq!((t, line), (Tier::Room, Some(Tier::Floor)));
        let duct = |size: f64| Facts { kind: "duct".into(), trade: "duct_supply".into(), size_in: Some(size), ..Facts::default() };
        assert_eq!(r.class(&duct(30.0)).0, Tier::Building);
        assert_eq!(r.class(&duct(10.0)).0, Tier::Floor);
        let atom = |kind: &str| Facts { kind: kind.into(), atom: true, trade: "conduit".into(), ..Facts::default() };
        assert_eq!(r.class(&atom("coupling")).0, Tier::Detail);
        assert_eq!(r.class(&atom("strap")).0, Tier::Detail);
        assert_eq!(r.class(&atom("wire")).0, Tier::Detail);
        assert_eq!(r.class(&atom("conduit")), (Tier::Room, Some(Tier::Floor), r.class(&atom("conduit")).2));
        assert_eq!(r.class(&atom("box")).0, Tier::Room);
        // A conduit run as one element (not pieces) is a run: floor.
        assert_eq!(r.class(&Facts { kind: "conduit".into(), trade: "conduit".into(), ..Facts::default() }).0, Tier::Floor);
        let eq = |text: &str, extent: f64| Facts {
            kind: "equipment".into(),
            trade: "equipment".into(),
            text: text.into(),
            extent_ft: Some(extent),
            ..Facts::default()
        };
        assert_eq!(r.class(&eq("switchboard msb", 3.0)).0, Tier::Building);
        assert_eq!(r.class(&eq("exhaust fan", 1.5)).0, Tier::Building);
        assert_eq!(r.class(&eq("panelboard", 2.0)).0, Tier::Floor);
        assert_eq!(r.class(&eq("cabinet", 6.0)).0, Tier::Building);
        let dev = |kind: &str, trade: &str| Facts { kind: kind.into(), trade: trade.into(), ..Facts::default() };
        assert_eq!(r.class(&dev("receptacle", "devices")).0, Tier::Room);
        assert_eq!(r.class(&dev("light_fixture", "lights")).0, Tier::Room);
        assert_eq!(r.class(&dev("air_terminal", "terminals")).0, Tier::Floor);
        let part =
            |p: &str, text: &str| Facts { kind: p.into(), part: p.into(), trade: "architecture".into(), text: text.into(), ..Facts::default() };
        assert_eq!(r.class(&part("floor", "floor slab")).0, Tier::Building);
        assert_eq!(r.class(&part("roof", "roof")).0, Tier::Building);
        assert_eq!(r.class(&part("wall", "wall exterior 1")).0, Tier::Building);
        assert_eq!(r.class(&part("wall", "wall type 3")).0, Tier::Floor);
        assert_eq!(r.class(&part("door", "door")).0, Tier::Floor);
        // Nothing matches: the default tier.
        assert_eq!(r.class(&Facts::default()), (Tier::Floor, None, None));
        // A line_from not below the tier is ignored.
        let odd = Rules { rules: vec![Rule { tier: "floor".into(), line_from: Some("room".into()), ..Rule::default() }], ..Rules::defaults() };
        assert_eq!(odd.class(&Facts::default()).1, None);
    }

    #[test]
    fn auto_thresholds_stay_ordered() {
        let a = Auto { building_ft: 100.0, floor_ft: 300.0, room_ft: -2.0, margin: 5.0 }.tiers();
        assert!(a.building_ft >= a.floor_ft && a.floor_ft >= a.room_ft && a.room_ft > 0.0);
        assert!(a.margin <= 0.9);
    }
}
