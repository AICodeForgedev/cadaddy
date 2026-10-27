//! Takeoff, schedules and checks use the elements' properties (Q6003).
//!
//! Every element of the built model carries its own property store ([`super::elemprops`]); this
//! module reads them:
//!
//! - **Grouping and filtering by any property**: `bim.takeoff` and `bim.bom` take
//!   `group_by: ["level", "circuit"]` (any keys) and `filter: {kind: "device", type: "duplex"}`
//!   (any keys; `where: {key: value | "exists" | "missing"}` as `bim.props.find`). Each line counts
//!   its elements and measures them by their kind: length for ducts, pipes, conduit and walls;
//!   area for rooms, floors, ceilings and roofs; sheet metal and weight for ducts (by gauge);
//!   hangers, insulation and fittings for pipes.
//! - **Schedules from the properties** (`bim.schedule {kind}`): device (by room or circuit), door,
//!   window, room finish, lighting fixture, equipment, plumbing fixture, air distribution (unit →
//!   terminals, cfm), panel (circuits, breakers, wire, connected load per phase), wire and conduit
//!   by circuit, duct and pipe; to CSV or XLSX.
//! - **Checks** (`bim.props.check`, [`checks`]): findings on what the properties say — devices
//!   with no circuit, assumed mounting heights, doors in rated walls with no rating, rooms with no
//!   number, receptacles in wet rooms without GFCI, circuit load, wire against breaker, conduit
//!   fill, voltage drop, equipment MOCP; terminal airflow against the unit, duct velocity, fire
//!   dampers, unserved terminals, unpowered equipment; drainage fixture units against pipe size,
//!   drain slope, fixtures with no waste or vent, water heater recovery, gas pipe size.
//!
//! What an assumption gives (a default load, gauge, demand factor) is marked as one (`rule`), as
//! the properties are.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use cadaddy_doc::props::{Prop, SourceKind};
use cadaddy_doc::{Drawing, Entity, EntityKind, Handle};
use serde_json::{Value, json};

use super::*;
use crate::{Result, Session};

pub mod checks;
pub mod codes;

use codes::Section;

pub(crate) const SCHEDULE: &str = "bim.schedule";
/// Most elements a line or schedule row lists.
const MAX_LINE_ELEMENTS: usize = 5_000;
/// Most rows a schedule has.
const MAX_ROWS: usize = 100_000;
/// Most devices one circuit's estimated route goes through.
const MAX_ROUTED: usize = 2_000;
/// Most mesh vertices measured per element.
const MAX_VERTS: usize = 200_000;

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new(SCHEDULE, "Schedule from Element Properties", run_schedule)
            .params("{kind: device|door|window|room_finish|light_fixture|equipment|plumbing_fixture|air_distribution|panel|wire|duct|pipe, by?: room|circuit (device schedule; default room), level?, limits?: {drop_ft? (10), makeup_pct? (10)}, file?: .csv|.xlsx} → {schedule, columns, rows: [{column: value}], count, notes, csv, path?}: a schedule made from the elements' properties; panels: circuits, breakers, wire, connected load per phase")
            .noundo(),
        CommandSpec::new(checks::CHECK, "Check Element Properties", checks::run)
            .params("{checks?: [name] (default all: no_circuit, no_fed_from, default_height, rated_door, room_number, gfci_wet, circuit_load, wire_breaker, conduit_fill, voltage_drop, equipment_mocp, terminal_cfm, duct_velocity, fire_damper, terminal_unserved, equipment_power, drain_dfu, pipe_slope, fixture_waste_vent, water_heater, gas_size), level?, limits?: {cfm_tolerance_pct? (10), duct_velocity_fpm? (2000), voltage_drop_pct? (3), load_pct? (80), hw_demand_factor? (0.25), gas_btu_per_cf? (1000)}} → {findings: [{check, severity: error|warning|info, message, handle?, kind?, level?, values?}], counts: {check: n}, checked: {check: n}, totals: {dfu, gas}, limits}")
            .noundo(),
    ]
}

// ─── The model as elements with properties ─────────────────────────────────

/// One element of the model with its properties (stored and computed).
pub(crate) struct El<'a> {
    pub e: &'a Arc<Entity>,
    pub kind: String,
    pub props: Vec<(String, Prop, bool)>,
    /// 3D box, drawing units: min, max.
    pub lo: [f64; 3],
    pub hi: [f64; 3],
}

impl El<'_> {
    pub fn handle(&self) -> Handle {
        self.e.handle
    }

    /// A property with a value.
    pub fn get(&self, key: &str) -> Option<&Prop> {
        self.props.iter().find(|(k, p, _)| k.eq_ignore_ascii_case(key) && p.has_value()).map(|(_, p, _)| p)
    }

    /// A property's text, trimmed, when it has one.
    pub fn text(&self, key: &str) -> Option<String> {
        self.get(key).map(|p| p.text().trim().to_string()).filter(|t| !t.is_empty())
    }

    /// The first of `keys` with a value.
    pub fn first(&self, keys: &[&str]) -> Option<String> {
        keys.iter().find_map(|k| self.text(k))
    }

    /// The first of `keys` that reads as a number.
    pub fn num(&self, keys: &[&str]) -> Option<f64> {
        keys.iter().find_map(|k| self.get(k).and_then(|p| p.value.as_f64().or_else(|| codes::lead_num(&p.text()))))
    }

    pub fn level(&self) -> String {
        self.text("level").unwrap_or_default()
    }

    /// What the element is, in words: its type, type mark, model, name and build part.
    pub fn words(&self) -> String {
        let mut w: Vec<String> = ["type", "type_mark", "model", "name", "description", "tag"].iter().filter_map(|k| self.text(k)).collect();
        if let Some(p) = super::bimlook::made_tag(self.e, "part") {
            w.push(p.to_string());
        }
        if let Some(p) = super::wallgen::made_as(self.e) {
            w.push(p);
        }
        w.join(" ").to_ascii_lowercase()
    }

    /// Its own name as other elements refer to it: tag, number, or (equipment) type mark.
    pub fn tag(&self) -> Option<String> {
        self.first(&["tag", "number", "id"])
            .or_else(|| super::bimlook::made_tag(self.e, "tag").map(str::to_string))
            .or_else(|| if self.kind == "equipment" { self.text("type_mark") } else { None })
    }

    /// How a finding names it.
    pub fn label(&self) -> String {
        let what = self.tag().or_else(|| self.first(&["type_mark", "type", "name"])).unwrap_or_default();
        if what.is_empty() { format!("{} {}", self.kind, self.handle().hex()) } else { format!("{} {what} ({})", self.kind, self.handle().hex()) }
    }

    /// Plan box.
    pub fn plan(&self) -> cadaddy_geom::Bounds2 {
        cadaddy_geom::Bounds2::new(cadaddy_geom::Vec2::new(self.lo[0], self.lo[1]), cadaddy_geom::Vec2::new(self.hi[0], self.hi[1]))
    }

    pub fn center(&self) -> [f64; 3] {
        [(self.lo[0] + self.hi[0]) / 2.0, (self.lo[1] + self.hi[1]) / 2.0, (self.lo[2] + self.hi[2]) / 2.0]
    }
}

/// The model's elements with their properties.
pub(crate) struct Model<'a> {
    pub upf: f64,
    pub els: Vec<El<'a>>,
    by_handle: BTreeMap<String, usize>,
    by_tag: BTreeMap<String, usize>,
}

fn bounds3(d: &Drawing, e: &Entity) -> ([f64; 3], [f64; 3]) {
    let (mut lo, mut hi) = ([f64::INFINITY; 3], [f64::NEG_INFINITY; 3]);
    if let EntityKind::Mesh(m) = &e.kind {
        for v in m.vertices.iter().take(MAX_VERTS).filter(|v| v.x.is_finite() && v.y.is_finite() && v.z.is_finite()) {
            for (i, c) in [v.x, v.y, v.z].into_iter().enumerate() {
                if let (Some(l), Some(h)) = (lo.get_mut(i), hi.get_mut(i)) {
                    *l = l.min(c);
                    *h = h.max(c);
                }
            }
        }
    }
    if !lo.iter().chain(hi.iter()).all(|v| v.is_finite()) {
        let b = cadaddy_doc::entity_bounds(d, e, 0);
        if b.is_empty() || !(b.min.x.is_finite() && b.max.x.is_finite() && b.min.y.is_finite() && b.max.y.is_finite()) {
            return ([0.0; 3], [0.0; 3]);
        }
        return ([b.min.x, b.min.y, 0.0], [b.max.x, b.max.y, 0.0]);
    }
    (lo, hi)
}

impl<'a> Model<'a> {
    pub fn new(d: &'a Drawing) -> Model<'a> {
        let upf = super::elemprops::units_per_foot(d);
        let mut els = Vec::new();
        for (e, kind) in super::elemprops::elements(d) {
            let props = super::elemprops::effective(e, &kind, upf);
            let (lo, hi) = bounds3(d, e);
            els.push(El { e, kind, props, lo, hi });
        }
        let mut by_handle = BTreeMap::new();
        let mut by_tag = BTreeMap::new();
        for (i, el) in els.iter().enumerate() {
            by_handle.insert(el.handle().hex(), i);
            if let Some(t) = el.tag() {
                by_tag.entry(t.to_ascii_uppercase()).or_insert(i);
            }
        }
        Model { upf, els, by_handle, by_tag }
    }

    /// The element a link value names: a handle, or another element's tag (`AHU-1`), or the
    /// panel in `panel LP-1, circuit 12`.
    pub fn resolve(&self, value: &str) -> Option<usize> {
        let v = value.trim();
        if let Some(i) = self.by_handle.get(&v.to_ascii_uppercase()).or_else(|| self.by_handle.get(v)) {
            return Some(*i);
        }
        if let Some(i) = self.by_tag.get(&v.to_ascii_uppercase()) {
            return Some(*i);
        }
        let panel = v.to_ascii_lowercase().strip_prefix("panel ").map(|r| r.split(',').next().unwrap_or("").trim().to_ascii_uppercase());
        panel.and_then(|p| self.by_tag.get(&p)).copied()
    }

    /// The element `el` links to through the first of `keys` that names one.
    pub fn link(&self, el: &El, keys: &[&str]) -> Option<usize> {
        keys.iter().filter_map(|k| el.text(k)).find_map(|v| self.resolve(&v))
    }

    pub fn get(&self, i: usize) -> Option<&El<'a>> {
        self.els.get(i)
    }

    /// A length property in feet, or the element's length along its run.
    pub fn length_ft(&self, el: &El) -> f64 {
        if let Some(p) = el.get("length") {
            let v = p.value.as_f64().or_else(|| codes::lead_num(&p.text())).unwrap_or(f64::NAN);
            let unit = p.unit.as_deref().unwrap_or("ft").to_ascii_lowercase();
            let ft = match unit.as_str() {
                "in" | "inch" | "inches" | "\"" => v / 12.0,
                "m" => v * 3.280_84,
                "mm" => v / 304.8,
                _ => v,
            };
            if ft.is_finite() && ft >= 0.0 {
                return ft;
            }
        }
        let ext = [el.hi[0] - el.lo[0], el.hi[1] - el.lo[1], el.hi[2] - el.lo[2]];
        let len = if el.kind == "wall" { ext[0].hypot(ext[1]) } else { ext.iter().copied().fold(0.0, f64::max) };
        if len.is_finite() && self.upf > 0.0 { len / self.upf } else { 0.0 }
    }

    /// An area property in square feet, or the element's top area.
    pub fn area_sf(&self, el: &El) -> f64 {
        if let Some(p) = el.get("area") {
            let v = p.value.as_f64().or_else(|| codes::lead_num(&p.text())).unwrap_or(f64::NAN);
            let unit = p.unit.as_deref().unwrap_or("sf").to_ascii_lowercase();
            let sf = if unit.starts_with('m') { v * 10.763_9 } else { v };
            if sf.is_finite() && sf >= 0.0 {
                return sf;
            }
        }
        let a = super::elemprops::up_area(el.e).unwrap_or(0.0);
        if self.upf > 0.0 { a / (self.upf * self.upf) } else { 0.0 }
    }

    /// Plan distance between two elements' centres, feet, going along the axes (how runs go).
    pub fn run_ft(&self, a: [f64; 3], b: [f64; 3]) -> f64 {
        let d = (a[0] - b[0]).abs() + (a[1] - b[1]).abs() + (a[2] - b[2]).abs();
        if d.is_finite() && self.upf > 0.0 { d / self.upf } else { 0.0 }
    }
}

/// How an element is measured in a takeoff.
fn measured_by(kind: &str) -> &'static str {
    match kind {
        "duct" | "pipe" | "conduit" | "wall" => "length",
        "room" | "floor" | "ceiling" | "roof" => "area",
        _ => "count",
    }
}

/// The trade an element kind belongs to.
pub(crate) fn trade_of(kind: &str) -> &'static str {
    match kind {
        "device" | "light_fixture" | "conduit" => "electrical",
        "duct" | "equipment" => "mechanical",
        "pipe" | "fixture" => "plumbing",
        _ => "architectural",
    }
}

// ─── Element classes the schedules and checks go by ────────────────────────

fn has_any(w: &str, words: &[&str]) -> bool {
    words.iter().any(|x| w.contains(x))
}

/// An air terminal: a diffuser, grille, register (an airflow or the build's terminal part).
pub(crate) fn is_terminal(el: &El) -> bool {
    if !matches!(el.kind.as_str(), "fixture" | "device" | "duct") {
        return false;
    }
    let w = el.words();
    el.get("cfm").is_some() && el.kind != "duct"
        || has_any(&w, &["diffuser", "grille", "register", "air terminal", "terminal", "louver"]) && !has_any(&w, &["plumb", "sanitary"])
}

pub(crate) fn is_sprinkler(el: &El) -> bool {
    has_any(&el.words(), &["sprinkler", "fire protection head"])
}

/// A plumbing fixture (a fixture that is not an air terminal or a sprinkler head).
pub(crate) fn is_plumbing_fixture(el: &El) -> bool {
    el.kind == "fixture" && !is_terminal(el) && !is_sprinkler(el)
}

pub(crate) fn is_receptacle(el: &El) -> bool {
    if el.kind != "device" {
        return false;
    }
    let w = el.words();
    has_any(&w, &["recept", "duplex", "quad", "outlet", "gfci", "gfi"]) && !has_any(&w, &["data", "tel", "voice", "cat6", "tv", "floor box"])
}

pub(crate) fn is_panel(el: &El) -> bool {
    if el.kind != "equipment" {
        return false;
    }
    let w = el.words();
    if has_any(&w, &["panel", "switchboard", "switchgear", "distribution board", "mcc", "motor control"]) {
        return true;
    }
    let tag = el.tag().unwrap_or_default().to_ascii_uppercase();
    let head: String = tag.chars().take_while(char::is_ascii_alphabetic).collect();
    matches!(head.as_str(), "LP" | "PP" | "DP" | "MDP" | "RP" | "EP" | "PNL" | "SWBD" | "MSB" | "LPA" | "LPB" | "HP" | "HDP" | "EM")
        && !has_any(&w, &["heat pump"])
}

pub(crate) fn is_water_heater(el: &El) -> bool {
    if !matches!(el.kind.as_str(), "equipment" | "fixture") {
        return false;
    }
    let tag = el.tag().unwrap_or_default().to_ascii_uppercase();
    has_any(&el.words(), &["water heater"])
        || ["WH", "EWH", "GWH", "HWH"]
            .iter()
            .any(|p| tag.strip_prefix(p).is_some_and(|r| r.is_empty() || r.starts_with(['-', ' ']) || r.starts_with(|c: char| c.is_ascii_digit())))
}

/// A device's circuit as one key (`LP1-12`), and its panel and number.
pub(crate) struct CircuitId {
    pub key: String,
    pub panel: String,
    pub number: String,
}

pub(crate) fn circuit_of(el: &El) -> Option<CircuitId> {
    let c = el.text("circuit")?;
    let panel = el.text("panel").or_else(|| super::propfill::panel_of(&c)).unwrap_or_default().to_ascii_uppercase();
    let number = if panel.is_empty() {
        c.clone()
    } else {
        let up = c.to_ascii_uppercase();
        up.strip_prefix(&panel).map(|r| r.trim_start_matches(['-', '/', '.', ' ']).to_string()).unwrap_or(up)
    };
    let key = if panel.is_empty() { number.to_ascii_uppercase() } else { format!("{panel}-{}", number.to_ascii_uppercase()) };
    Some(CircuitId { key, panel, number })
}

/// A device's connected load, volt-amperes, and whether it is assumed: its own (`load_va`, `va`,
/// `load`, `watts`, `kw`), else 180 VA per receptacle yoke (NEC 220.14(I)).
pub(crate) fn load_va(el: &El) -> Option<(f64, bool)> {
    for k in ["load_va", "va", "load", "watts", "wattage", "kva", "kw"] {
        let Some(p) = el.get(k) else { continue };
        let Some(v) = p.value.as_f64().or_else(|| codes::lead_num(&p.text())) else { continue };
        let text = format!("{} {}", p.text(), p.unit.as_deref().unwrap_or("")).to_ascii_lowercase();
        let v = if k.starts_with('k') || text.contains("kw") || text.contains("kva") { v * 1000.0 } else { v };
        if v.is_finite() && v >= 0.0 {
            return Some((v, false));
        }
    }
    if is_receptacle(el) {
        let quad = el.words().contains("quad");
        return Some((if quad { 360.0 } else { 180.0 }, true));
    }
    None
}

// ─── Grouping and filtering by property ────────────────────────────────────

/// What a filter asks of one key.
#[derive(Clone, Debug, PartialEq)]
enum Want {
    Exists,
    Missing,
    Any(Vec<String>),
}

/// A takeoff filter by kind and properties.
#[derive(Clone, Debug, Default)]
pub(crate) struct PropFilter {
    kinds: Vec<String>,
    wants: Vec<(String, Want)>,
    text: Option<String>,
    trade: Option<String>,
}

fn want_of(cmd: &str, key: &str, v: &Value) -> Result<Want> {
    let s = |x: &Value| match x {
        Value::String(t) => Some(t.trim().to_string()),
        Value::Number(_) | Value::Bool(_) => Some(x.to_string()),
        _ => None,
    };
    Ok(match v {
        Value::String(t) if t.trim().eq_ignore_ascii_case("exists") => Want::Exists,
        Value::String(t) if t.trim().eq_ignore_ascii_case("missing") => Want::Missing,
        Value::Object(o) if o.get("exists").and_then(Value::as_bool) == Some(true) => Want::Exists,
        Value::Object(o) if o.get("missing").and_then(Value::as_bool) == Some(true) => Want::Missing,
        Value::Array(a) => Want::Any(a.iter().take(256).filter_map(s).collect()),
        _ => Want::Any(vec![s(v).ok_or_else(|| bad(cmd, format!("`filter.{key}`: a value, a list of values, \"exists\" or \"missing\"")))?]),
    })
}

impl PropFilter {
    pub fn parse(cmd: &str, f: &Value) -> Result<PropFilter> {
        let mut out = PropFilter::default();
        match f {
            Value::Null => {}
            Value::String(t) => out.text = Some(t.trim().to_ascii_lowercase()).filter(|t| !t.is_empty()),
            Value::Object(o) => {
                for (k, v) in o.iter().take(64) {
                    match k.as_str() {
                        "kind" => {
                            out.kinds = match v {
                                Value::String(t) => vec![t.trim().to_ascii_lowercase()],
                                Value::Array(a) => a.iter().filter_map(Value::as_str).map(|t| t.trim().to_ascii_lowercase()).collect(),
                                _ => return Err(bad(cmd, "`filter.kind`: a kind or a list of kinds")),
                            };
                            out.kinds.retain(|k| !k.is_empty());
                        }
                        "text" => out.text = v.as_str().map(|t| t.trim().to_ascii_lowercase()).filter(|t| !t.is_empty()),
                        "trade" => out.trade = v.as_str().map(|t| t.trim().to_ascii_lowercase()).filter(|t| !t.is_empty()),
                        "where" => match v {
                            Value::Object(w) => {
                                for (wk, wv) in w.iter().take(64) {
                                    out.wants.push((wk.trim().to_string(), want_of(cmd, wk, wv)?));
                                }
                            }
                            Value::Null => {}
                            _ => return Err(bad(cmd, "`filter.where` must be an object {key: value | \"exists\" | \"missing\"}")),
                        },
                        _ => out.wants.push((k.trim().to_string(), want_of(cmd, k, v)?)),
                    }
                }
            }
            _ => return Err(bad(cmd, "`filter` must be an object {kind?, key: value…} or a text")),
        }
        Ok(out)
    }

    pub fn matches(&self, el: &El) -> bool {
        if !self.kinds.is_empty() && !self.kinds.contains(&el.kind) {
            return false;
        }
        if let Some(t) = &self.trade
            && !trade_of(&el.kind).contains(t.as_str())
        {
            return false;
        }
        if let Some(t) = &self.text
            && !el.kind.contains(t.as_str())
            && !el.props.iter().any(|(_, p, _)| p.has_value() && p.text().to_ascii_lowercase().contains(t.as_str()))
        {
            return false;
        }
        self.wants.iter().all(|(key, want)| {
            let v = el.text(key);
            match want {
                Want::Exists => v.is_some(),
                Want::Missing => v.is_none(),
                Want::Any(xs) => v.is_some_and(|v| xs.iter().any(|x| value_is(&v, x))),
            }
        })
    }
}

/// A property value matches what a filter asks: the same text or number, or it contains it
/// (`type: duplex` matches "Duplex receptacle").
fn value_is(v: &str, want: &str) -> bool {
    let (v, w) = (v.trim().to_ascii_lowercase(), want.trim().to_ascii_lowercase());
    if v == w {
        return true;
    }
    if let (Ok(a), Ok(b)) = (v.parse::<f64>(), w.parse::<f64>()) {
        return (a - b).abs() < 1e-9;
    }
    !w.is_empty() && v.contains(&w)
}

/// The keys a `group_by` names (a key or a list).
fn group_keys(cmd: &str, g: &Value) -> Result<Vec<String>> {
    let keys: Vec<String> = match g {
        Value::Null => vec!["level".into()],
        Value::String(t) => t.split([',', '+']).map(|k| k.trim().to_string()).filter(|k| !k.is_empty()).collect(),
        Value::Array(a) => a.iter().take(16).filter_map(Value::as_str).map(|k| k.trim().to_string()).filter(|k| !k.is_empty()).collect(),
        _ => return Err(bad(cmd, "`group_by` is a property key or a list of keys")),
    };
    if keys.is_empty() {
        return Err(bad(cmd, "`group_by` names no property"));
    }
    Ok(keys)
}

/// Whether a takeoff or BOM call groups or filters by the elements' properties (a list, a key
/// the line-based takeoff has no column for, or a filter by kind or property).
pub(crate) fn by_properties(p: &Value, line_groups: &[&str], line_filters: &[&str]) -> bool {
    let g = match p.get("group_by") {
        Some(Value::Array(_)) => true,
        Some(Value::String(t)) => !line_groups.iter().any(|x| x.eq_ignore_ascii_case(t.trim())),
        _ => false,
    };
    let f = match p.get("filter") {
        Some(Value::Object(o)) => o.keys().any(|k| !line_filters.contains(&k.as_str())),
        _ => false,
    };
    g || f
}

/// Extra measures of a line by its kind.
#[derive(Default)]
struct Extra {
    sheet_metal_sf: f64,
    weight_lb: f64,
    gauges: BTreeSet<u32>,
    assumed_gauge: usize,
    hangers: f64,
    insulation_lf: f64,
    fittings: usize,
}

fn is_fitting(el: &El) -> bool {
    has_any(&el.words(), &["fitting", "elbow", "tee", "wye", "reducer", "transition", "coupling"])
}

fn insulated(el: &El) -> bool {
    el.text("insulation").is_some_and(|t| !matches!(t.to_ascii_lowercase().as_str(), "none" | "no" | "0" | "-" | "n/a" | "false"))
}

/// The property takeoff: lines by `group_by`'s keys (and kind), of the elements `filter` lets
/// through.
pub(crate) fn grouped(s: &Session, cmd: &str, p: &Value) -> Result<Value> {
    let keys = group_keys(cmd, p.get("group_by").unwrap_or(&Value::Null))?;
    let filter = PropFilter::parse(cmd, p.get("filter").unwrap_or(&Value::Null))?;
    let d = s.doc()?;
    let m = Model::new(d);
    struct Acc {
        group: Vec<String>,
        kind: String,
        count: usize,
        length_ft: f64,
        area_sf: f64,
        elements: Vec<String>,
        levels: BTreeSet<String>,
        extra: Extra,
    }
    let mut lines: BTreeMap<(String, Vec<String>), Acc> = BTreeMap::new();
    let mut seen = 0usize;
    let mut carried: BTreeSet<String> = BTreeSet::new();
    for el in m.els.iter().filter(|el| filter.matches(el)) {
        seen += 1;
        for (k, p, _) in &el.props {
            if p.has_value() && carried.len() < 4_096 {
                carried.insert(k.to_ascii_lowercase());
            }
        }
        let values: Vec<String> =
            keys.iter().map(|k| if k == "kind" { el.kind.clone() } else { el.text(k).unwrap_or_else(|| "(none)".into()) }).collect();
        let id = (el.kind.clone(), values.iter().map(|v| v.to_ascii_lowercase()).collect::<Vec<_>>());
        let a = lines.entry(id).or_insert_with(|| Acc {
            group: values.clone(),
            kind: el.kind.clone(),
            count: 0,
            length_ft: 0.0,
            area_sf: 0.0,
            elements: Vec::new(),
            levels: BTreeSet::new(),
            extra: Extra::default(),
        });
        a.count += 1;
        if a.elements.len() < MAX_LINE_ELEMENTS {
            a.elements.push(el.handle().hex());
        }
        let lv = el.level();
        if !lv.is_empty() {
            a.levels.insert(lv);
        }
        match measured_by(&el.kind) {
            "length" => {
                let len = m.length_ft(el);
                a.length_ft += len;
                let sec = el.text("size").and_then(|t| Section::parse(&t));
                if el.kind == "duct" {
                    if let Some(sec) = sec {
                        let sf = sec.perimeter_ft() * len;
                        let gauge = el.num(&["gauge", "ga"]).map(|g| g.round() as u32).filter(|g| codes::gauge_lb_sf(*g).is_some());
                        if gauge.is_none() {
                            a.extra.assumed_gauge += 1;
                        }
                        let g = gauge.unwrap_or_else(|| codes::gauge_for(sec.largest_in()));
                        a.extra.gauges.insert(g);
                        a.extra.sheet_metal_sf += sf;
                        a.extra.weight_lb += sf * codes::gauge_lb_sf(g).unwrap_or(0.0);
                    }
                    if insulated(el) {
                        a.extra.insulation_lf += len;
                    }
                    if is_fitting(el) {
                        a.extra.fittings += 1;
                    }
                }
                if el.kind == "pipe" {
                    let size = el.text("size").and_then(|t| codes::inches(&t));
                    let mat = el.text("material").unwrap_or_default();
                    if !is_fitting(el) && len > 0.0 {
                        a.extra.hangers += (len / codes::hanger_spacing_ft(&mat, size)).ceil();
                    }
                    if insulated(el) {
                        a.extra.insulation_lf += len;
                    }
                    if is_fitting(el) {
                        a.extra.fittings += 1;
                    }
                }
            }
            "area" => a.area_sf += m.area_sf(el),
            _ => {}
        }
    }
    let r2 = |v: f64| (v * 100.0).round() / 100.0;
    let mut out_lines = Vec::new();
    for a in lines.into_values() {
        let mut group = serde_json::Map::new();
        for (k, v) in keys.iter().zip(&a.group) {
            group.insert(k.clone(), json!(v));
        }
        let by = measured_by(&a.kind);
        let (qty, unit) = match by {
            "length" => (r2(a.length_ft), "lf"),
            "area" => (r2(a.area_sf), "sf"),
            _ => (a.count as f64, "ea"),
        };
        let item = a.group.iter().filter(|v| *v != "(none)").cloned().collect::<Vec<_>>().join(" / ");
        let key = format!("{}|{}", a.kind, a.group.join("|"));
        let mut l = json!({
            "key": key, "kind": a.kind, "trade": trade_of(&a.kind), "group": group, "item": if item.is_empty() { a.kind.clone() } else { item },
            "count": a.count, "qty": qty, "unit": unit, "element_count": a.count, "elements": a.elements, "levels": a.levels,
        });
        for (k, v) in keys.iter().zip(&a.group) {
            if !matches!(k.as_str(), "key" | "kind" | "group" | "item" | "count" | "qty" | "unit" | "elements" | "element_count" | "levels" | "trade")
            {
                l[k] = json!(v);
            }
        }
        if by == "length" {
            l["length_ft"] = json!(r2(a.length_ft));
        }
        if by == "area" {
            l["area_sf"] = json!(r2(a.area_sf));
        }
        if a.kind == "duct" {
            l["sheet_metal_sf"] = json!(r2(a.extra.sheet_metal_sf));
            l["weight_lb"] = json!(r2(a.extra.weight_lb));
            l["gauges"] = json!(a.extra.gauges);
            if a.extra.assumed_gauge > 0 {
                l["gauge_rule"] =
                    json!(format!("{} duct(s) carry no gauge: the low-pressure gauge for their largest side (assumed)", a.extra.assumed_gauge));
            }
        }
        if matches!(a.kind.as_str(), "duct" | "pipe") {
            l["insulation_lf"] = json!(r2(a.extra.insulation_lf));
            l["fittings"] = json!(a.extra.fittings);
        }
        if a.kind == "pipe" {
            l["hangers"] = json!(a.extra.hangers);
        }
        out_lines.push(l);
    }
    // One key named as text that no element carries is a mistyped grouping, not a property.
    if let Some(Value::String(_)) = p.get("group_by")
        && let Some(k) = keys.iter().find(|k| *k != "kind" && !carried.contains(&k.to_ascii_lowercase()))
    {
        return Err(bad(
            cmd,
            format!(
                "no element carries `{k}`: group by a property the elements carry (bim.props.summary lists them), or as the command's params say"
            ),
        ));
    }
    // Totals.
    let sum = |k: &str| r2(out_lines.iter().filter_map(|l| l[k].as_f64()).sum::<f64>());
    let totals = json!({"lines": out_lines.len(), "elements": seen, "length_ft": sum("length_ft"), "area_sf": sum("area_sf"), "sheet_metal_sf": sum("sheet_metal_sf"), "weight_lb": sum("weight_lb"), "hangers": sum("hangers")});
    // The table.
    let mut head: Vec<String> = keys.clone();
    head.extend(["kind", "count", "qty", "unit"].map(String::from));
    let rows: Vec<Vec<String>> = std::iter::once(head)
        .chain(out_lines.iter().map(|l| {
            let mut r: Vec<String> = keys.iter().map(|k| l["group"][k].as_str().unwrap_or_default().to_string()).collect();
            r.push(l["kind"].as_str().unwrap_or_default().to_string());
            r.push(l["count"].to_string());
            r.push(l["qty"].to_string());
            r.push(l["unit"].as_str().unwrap_or_default().to_string());
            r
        }))
        .collect();
    let mut out = json!({"lines": out_lines, "totals": totals, "group_by": keys, "by": "properties", "csv": csv_text(&rows)});
    if seen == 0 {
        out["next_step"] = json!(if m.els.is_empty() {
            "the model has no elements yet: build it with model.build or bim.build"
        } else {
            "no element matches the filter: see bim.props.summary for the kinds and keys the model has"
        });
    }
    if let Some(path) = str_param(p, "file").map(str::trim).filter(|x| !x.is_empty()) {
        out["path"] = write_table(cmd, path, &rows, "Takeoff")?;
    }
    Ok(out)
}

// ─── Tables ────────────────────────────────────────────────────────────────

fn csv_cell(v: &str) -> String {
    if v.contains([',', '"', '\n']) { format!("\"{}\"", v.replace('"', "\"\"")) } else { v.to_string() }
}

pub(crate) fn csv_text(rows: &[Vec<String>]) -> String {
    rows.iter().map(|r| r.iter().map(|c| csv_cell(c)).collect::<Vec<_>>().join(",") + "\n").collect()
}

pub(crate) fn write_table(cmd: &str, path: &str, rows: &[Vec<String>], sheet: &str) -> Result<Value> {
    let bytes = if path.to_ascii_lowercase().ends_with(".xlsx") {
        super::table::xlsx::write(rows, sheet).map_err(|e| bad(cmd, e))?
    } else {
        csv_text(rows).into_bytes()
    };
    #[cfg(not(target_arch = "wasm32"))]
    crate::writefile::write(path, &bytes).map_err(|e| crate::EngineError::Other(format!("{path}: {e}")))?;
    #[cfg(target_arch = "wasm32")]
    let _ = bytes;
    Ok(json!(path))
}

// ─── Schedules ─────────────────────────────────────────────────────────────

/// A schedule being made: its columns and rows.
struct Sched {
    columns: Vec<&'static str>,
    rows: Vec<Vec<String>>,
    notes: Vec<String>,
}

impl Sched {
    fn new(columns: &[&'static str]) -> Sched {
        Sched { columns: columns.to_vec(), rows: Vec::new(), notes: Vec::new() }
    }

    fn push(&mut self, row: Vec<String>) {
        if self.rows.len() < MAX_ROWS {
            self.rows.push(row);
        }
    }
}

fn n2(v: f64) -> String {
    let r = (v * 100.0).round() / 100.0;
    if r.fract() == 0.0 { format!("{r:.0}") } else { format!("{r}") }
}

/// The value with its unit, as the schedule shows it.
fn shown(el: &El, key: &str) -> String {
    el.get(key)
        .map(|p| {
            let t = p.text().trim().to_string();
            let marked = if p.source.kind.is_assumed() { " (assumed)" } else { "" };
            match p.unit.as_deref().filter(|u| !u.is_empty() && !t.ends_with(*u)) {
                Some(u) => format!("{t} {u}{marked}"),
                None => format!("{t}{marked}"),
            }
        })
        .unwrap_or_default()
}

/// A number-aware sort key: "10" after "9", "A101" after "A20".
fn natural(t: &str) -> (String, u64, String) {
    let head: String = t.chars().take_while(|c| !c.is_ascii_digit()).collect();
    let digits: String = t.chars().skip(head.chars().count()).take_while(char::is_ascii_digit).collect();
    let rest: String = t.chars().skip(head.chars().count() + digits.chars().count()).collect();
    (head.to_ascii_uppercase(), digits.parse().unwrap_or(0), rest)
}

fn joined(set: &BTreeSet<String>) -> String {
    set.iter().filter(|s| !s.is_empty()).cloned().collect::<Vec<_>>().join(", ")
}

fn sched_device(m: &Model, by_circuit: bool, level: Option<&str>) -> Sched {
    let mut s = Sched::new(if by_circuit {
        &["circuit", "panel", "type", "type_mark", "count", "rooms", "mount_height", "load_va"]
    } else {
        &["room", "type", "type_mark", "count", "circuits", "mount_height", "level"]
    });
    let mut g: BTreeMap<(String, String, String), (usize, BTreeSet<String>, BTreeSet<String>, BTreeSet<String>, f64, String)> = BTreeMap::new();
    for el in m.els.iter().filter(|e| matches!(e.kind.as_str(), "device" | "light_fixture") && on_level(e, level)) {
        let head = if by_circuit {
            circuit_of(el).map(|c| c.key).unwrap_or_else(|| "(no circuit)".into())
        } else {
            el.text("room").unwrap_or_else(|| "(no room)".into())
        };
        let ty = el.text("type").unwrap_or_else(|| el.kind.clone());
        let mark = el.text("type_mark").unwrap_or_default();
        let e = g.entry((head, ty, mark)).or_default();
        e.0 += 1;
        e.1.insert(if by_circuit { el.text("room").unwrap_or_default() } else { circuit_of(el).map(|c| c.key).unwrap_or_default() });
        e.2.insert(shown(el, "mount_height"));
        e.3.insert(el.level());
        e.4 += load_va(el).map_or(0.0, |(v, _)| v);
        if e.5.is_empty() {
            e.5 = circuit_of(el).map(|c| c.panel).unwrap_or_default();
        }
    }
    let mut keys: Vec<_> = g.into_iter().collect();
    keys.sort_by(|a, b| natural(&a.0.0).cmp(&natural(&b.0.0)).then(a.0.1.cmp(&b.0.1)));
    for ((head, ty, mark), (n, other, heights, levels, va, panel)) in keys {
        s.push(if by_circuit {
            vec![head, panel, ty, mark, n.to_string(), joined(&other), joined(&heights), n2(va)]
        } else {
            vec![head, ty, mark, n.to_string(), joined(&other), joined(&heights), joined(&levels)]
        });
    }
    s
}

fn on_level(el: &El, level: Option<&str>) -> bool {
    level.is_none_or(|l| el.level().eq_ignore_ascii_case(l))
}

/// One row per element of `kind`, the columns its properties.
fn sched_each(m: &Model, kinds: &[&str], columns: &[&'static str], sort_by: &str, level: Option<&str>, keep: impl Fn(&El) -> bool) -> Sched {
    let mut s = Sched::new(columns);
    let mut els: Vec<&El> = m.els.iter().filter(|e| kinds.contains(&e.kind.as_str()) && on_level(e, level) && keep(e)).collect();
    els.sort_by_key(|a| natural(&a.text(sort_by).unwrap_or_default()));
    for el in els {
        s.push(
            columns
                .iter()
                .map(|c| match *c {
                    "area" => n2(m.area_sf(el)),
                    "handle" => el.handle().hex(),
                    "tag" => el.tag().unwrap_or_default(),
                    c => shown(el, c),
                })
                .collect(),
        );
    }
    s
}

fn sched_lights(m: &Model, level: Option<&str>) -> Sched {
    let mut s = Sched::new(&["type_mark", "type", "model", "count", "mount_height", "circuits", "load_va"]);
    let mut g: BTreeMap<String, (String, String, usize, BTreeSet<String>, BTreeSet<String>, f64)> = BTreeMap::new();
    for el in m.els.iter().filter(|e| e.kind == "light_fixture" && on_level(e, level)) {
        let e = g.entry(el.text("type_mark").unwrap_or_else(|| "(no mark)".into())).or_default();
        if e.0.is_empty() {
            e.0 = el.text("type").unwrap_or_default();
            e.1 = el.text("model").unwrap_or_default();
        }
        e.2 += 1;
        e.3.insert(shown(el, "mount_height"));
        e.4.insert(circuit_of(el).map(|c| c.key).unwrap_or_default());
        e.5 += load_va(el).map_or(0.0, |(v, _)| v);
    }
    for (mark, (ty, model, n, h, c, va)) in g {
        s.push(vec![mark, ty, model, n.to_string(), joined(&h), joined(&c), n2(va)]);
    }
    s
}

fn sched_plumbing(m: &Model, level: Option<&str>) -> Sched {
    let mut s = Sched::new(&["room", "type_mark", "type", "count", "dfu_each", "dfu_total", "wsfu_total", "waste", "vent", "level"]);
    let mut g: BTreeMap<(String, String, String), (usize, f64, f64, f64, usize, usize, BTreeSet<String>)> = BTreeMap::new();
    let mut assumed = 0usize;
    for el in m.els.iter().filter(|e| is_plumbing_fixture(e) && on_level(e, level)) {
        let room = el.text("room").unwrap_or_else(|| "(no room)".into());
        let mark = el.text("type_mark").unwrap_or_default();
        let e = g.entry((room, mark, el.text("type").unwrap_or_default())).or_default();
        e.0 += 1;
        let dfu = el.num(&["dfu", "fixture_units", "drainage_fixture_units"]).or_else(|| {
            let v = codes::dfu_of(&el.words());
            assumed += usize::from(v.is_some());
            v
        });
        e.1 = dfu.unwrap_or(0.0);
        e.2 += dfu.unwrap_or(0.0);
        e.3 += el.num(&["wsfu", "supply_fixture_units"]).unwrap_or(0.0);
        e.4 += usize::from(el.first(&["waste", "drains_to", "drain"]).is_some());
        e.5 += usize::from(el.first(&["vent", "vented_by"]).is_some());
        e.6.insert(el.level());
    }
    let mut rows: Vec<_> = g.into_iter().collect();
    rows.sort_by(|a, b| natural(&a.0.0).cmp(&natural(&b.0.0)).then(a.0.1.cmp(&b.0.1)));
    for ((room, mark, ty), (n, each, dfu, wsfu, waste, vent, levels)) in rows {
        s.push(vec![room, mark, ty, n.to_string(), n2(each), n2(dfu), n2(wsfu), format!("{waste}/{n}"), format!("{vent}/{n}"), joined(&levels)]);
    }
    if assumed > 0 {
        s.notes.push(format!("{assumed} fixture(s) carry no fixture units: IPC Table 709.1's value for their type (assumed)"));
    }
    s
}

/// The air terminals each unit serves, with their airflow.
pub(crate) fn air_systems<'m>(m: &'m Model, level: Option<&str>) -> (BTreeMap<usize, Vec<&'m El<'m>>>, Vec<&'m El<'m>>) {
    let mut by_unit: BTreeMap<usize, Vec<&El>> = BTreeMap::new();
    let mut unserved = Vec::new();
    for el in m.els.iter().filter(|e| is_terminal(e) && on_level(e, level)) {
        match m.link(el, &["served_by", "fed_from", "unit"]) {
            Some(u) => by_unit.entry(u).or_default().push(el),
            None => unserved.push(el),
        }
    }
    (by_unit, unserved)
}

fn sched_air(m: &Model, level: Option<&str>) -> Sched {
    let mut s =
        Sched::new(&["unit", "unit_cfm", "terminals", "terminal_cfm", "difference_pct", "terminal", "type_mark", "size", "cfm", "room", "level"]);
    let (by_unit, unserved) = air_systems(m, level);
    for (u, terms) in &by_unit {
        let Some(unit) = m.get(*u) else { continue };
        let ucfm = unit.num(&["cfm", "airflow", "capacity"]);
        let sum: f64 = terms.iter().filter_map(|t| t.num(&["cfm", "airflow"])).sum();
        let diff = ucfm.filter(|c| *c > 0.0).map(|c| n2((sum - c) / c * 100.0)).unwrap_or_default();
        let name = unit.tag().unwrap_or_else(|| unit.handle().hex());
        for t in terms {
            s.push(vec![
                name.clone(),
                ucfm.map(n2).unwrap_or_default(),
                terms.len().to_string(),
                n2(sum),
                diff.clone(),
                t.handle().hex(),
                t.text("type_mark").unwrap_or_default(),
                t.text("size").unwrap_or_default(),
                t.num(&["cfm", "airflow"]).map(n2).unwrap_or_default(),
                t.text("room").unwrap_or_default(),
                t.level(),
            ]);
        }
    }
    for t in &unserved {
        s.push(vec![
            "(not served)".into(),
            String::new(),
            String::new(),
            String::new(),
            String::new(),
            t.handle().hex(),
            t.text("type_mark").unwrap_or_default(),
            t.text("size").unwrap_or_default(),
            t.num(&["cfm", "airflow"]).map(n2).unwrap_or_default(),
            t.text("room").unwrap_or_default(),
            t.level(),
        ]);
    }
    s
}

/// The phase a panel circuit lands on: circuits 1-2 on A, 3-4 on B, 5-6 on C (three phase), or
/// A/B alternating (single phase).
fn phase_of(number: &str, phases: u32) -> Option<char> {
    let n: u32 = number.trim().chars().take_while(char::is_ascii_digit).collect::<String>().parse().ok().filter(|n| *n > 0)?;
    let row = (n - 1) / 2;
    let order: &[char] = if phases == 1 { &['A', 'B'] } else { &['A', 'B', 'C'] };
    order.get(row as usize % order.len()).copied()
}

/// A branch circuit as the model shows it: its devices, load, breaker and wire.
pub(crate) struct Circuit<'m> {
    pub id: CircuitId,
    pub devices: Vec<&'m El<'m>>,
    pub load_va: f64,
    pub assumed_load: usize,
    pub unknown_load: usize,
    pub breaker: Option<f64>,
    pub poles: Option<u32>,
    pub wire: Option<String>,
    pub voltage: Option<f64>,
    pub description: String,
    pub conduit: Vec<&'m El<'m>>,
}

/// The model's branch circuits, by key.
pub(crate) fn circuits<'m>(m: &'m Model, level: Option<&str>) -> BTreeMap<String, Circuit<'m>> {
    let mut out: BTreeMap<String, Circuit> = BTreeMap::new();
    for el in m.els.iter().filter(|e| on_level(e, level)) {
        let Some(id) = circuit_of(el) else { continue };
        let key = id.key.clone();
        let c = out.entry(key).or_insert_with(|| Circuit {
            id,
            devices: Vec::new(),
            load_va: 0.0,
            assumed_load: 0,
            unknown_load: 0,
            breaker: None,
            poles: None,
            wire: None,
            voltage: None,
            description: String::new(),
            conduit: Vec::new(),
        });
        if el.kind == "conduit" {
            c.conduit.push(el);
        } else {
            c.devices.push(el);
            match load_va(el) {
                Some((v, assumed)) => {
                    c.load_va += v;
                    c.assumed_load += usize::from(assumed);
                }
                None => c.unknown_load += 1,
            }
        }
        c.breaker = c.breaker.or_else(|| el.num(&["breaker", "ocpd"]));
        c.poles = c.poles.or_else(|| el.num(&["poles"]).map(|p| p.round().clamp(1.0, 3.0) as u32));
        c.wire = c.wire.clone().or_else(|| el.first(&["wire", "wire_size", "conductors"]));
        c.voltage = c.voltage.or_else(|| el.num(&["voltage", "volts"]));
        if c.description.is_empty() {
            c.description = el.first(&["circuit_description"]).unwrap_or_default();
        }
    }
    out
}

fn sched_panel(m: &Model, level: Option<&str>) -> (Sched, Value) {
    let mut s = Sched::new(&["panel", "circuit", "description", "breaker", "poles", "wire", "devices", "load_va", "phase"]);
    let cs = circuits(m, level);
    // Panels: the ones the circuits name, and the panel elements.
    let mut panels: BTreeMap<String, Vec<&Circuit>> = BTreeMap::new();
    for el in m.els.iter().filter(|e| is_panel(e)) {
        if let Some(t) = el.tag() {
            panels.entry(t.to_ascii_uppercase()).or_default();
        }
    }
    for c in cs.values() {
        panels.entry(if c.id.panel.is_empty() { "(no panel)".into() } else { c.id.panel.clone() }).or_default().push(c);
    }
    let mut summary = Vec::new();
    let mut assumed = 0usize;
    let mut unknown = 0usize;
    for (panel, mut list) in panels {
        let pel = m.by_tag.get(&panel).and_then(|i| m.get(*i));
        let phases = pel.and_then(|p| p.num(&["phases", "phase"])).map_or(3, |p| if p.round() as u32 == 1 { 1 } else { 3 });
        list.sort_by_key(|a| natural(&a.id.number));
        let mut per_phase: BTreeMap<char, f64> = BTreeMap::new();
        let mut total = 0.0;
        for c in &list {
            let poles = c.poles.unwrap_or(1).max(1);
            let first = phase_of(&c.id.number, phases);
            // A multi-pole load spreads over the phases its poles land on.
            let mut landed = String::new();
            if let Some(p0) = first {
                let order: &[char] = if phases == 1 { &['A', 'B'] } else { &['A', 'B', 'C'] };
                let at = order.iter().position(|x| *x == p0).unwrap_or(0);
                for k in 0..poles as usize {
                    let ph = order.get((at + k) % order.len()).copied().unwrap_or('A');
                    *per_phase.entry(ph).or_insert(0.0) += c.load_va / f64::from(poles);
                    landed.push(ph);
                }
            }
            total += c.load_va;
            assumed += c.assumed_load;
            unknown += c.unknown_load;
            let desc = if c.description.is_empty() {
                let types: BTreeSet<String> = c.devices.iter().map(|d| d.text("type").unwrap_or_else(|| d.kind.clone())).collect();
                joined(&types)
            } else {
                c.description.clone()
            };
            s.push(vec![
                panel.clone(),
                c.id.number.clone(),
                desc,
                c.breaker.map(|b| format!("{}A", n2(b))).unwrap_or_default(),
                c.poles.map(|p| p.to_string()).unwrap_or_default(),
                c.wire.clone().unwrap_or_default(),
                c.devices.len().to_string(),
                n2(c.load_va),
                landed,
            ]);
        }
        summary.push(json!({
            "panel": panel, "phases": phases, "circuits": list.len(), "connected_va": n2(total).parse::<f64>().unwrap_or(0.0),
            "per_phase_va": per_phase.iter().map(|(k, v)| (k.to_string(), json!((v * 100.0).round() / 100.0))).collect::<serde_json::Map<_, _>>(),
            "element": pel.map(|p| p.handle().hex()),
        }));
    }
    if assumed > 0 {
        s.notes.push(format!("{assumed} receptacle(s) carry no load: 180 VA per yoke (NEC 220.14(I), assumed)"));
    }
    if unknown > 0 {
        s.notes.push(format!("{unknown} device(s) carry no load and are counted at 0 VA: set load_va on them"));
    }
    s.notes.push("phases: circuits 1-2 on A, 3-4 on B, 5-6 on C (single-phase panels A/B), as panelboards number them".into());
    (s, json!(summary))
}

/// A circuit's route length, feet: its conduit, else a chain from the panel through its devices
/// (nearest next, along the axes; the first [`MAX_ROUTED`]), marked estimated.
pub(crate) fn circuit_length(m: &Model, c: &Circuit) -> (f64, bool) {
    let conduit: f64 = c.conduit.iter().map(|e| m.length_ft(e)).sum();
    if conduit > 0.0 {
        return (conduit, false);
    }
    let start = c.devices.iter().find_map(|d| m.link(d, &["fed_from_handle", "fed_from"])).and_then(|i| m.get(i)).map(El::center);
    let mut left: Vec<[f64; 3]> = c.devices.iter().take(MAX_ROUTED).map(|d| d.center()).collect();
    let mut at = match start {
        Some(p) => p,
        None if !left.is_empty() => left.remove(0),
        None => return (0.0, true),
    };
    let mut len = 0.0;
    while !left.is_empty() {
        let (i, d) = left.iter().enumerate().map(|(i, p)| (i, m.run_ft(at, *p))).min_by(|a, b| a.1.total_cmp(&b.1)).unwrap_or((0, 0.0));
        len += d;
        at = left.remove(i);
    }
    (len, true)
}

fn sched_wire(m: &Model, level: Option<&str>, limits: &Value) -> Sched {
    let drop_ft = limits["drop_ft"].as_f64().filter(|v| v.is_finite() && *v >= 0.0).unwrap_or(10.0);
    let makeup = limits["makeup_pct"].as_f64().filter(|v| v.is_finite() && *v >= 0.0).unwrap_or(10.0);
    let mut s = Sched::new(&["circuit", "devices", "conduit_size", "conduit_lf", "route", "wire", "conductors", "wire_size", "wire_lf"]);
    let mut by_size: BTreeMap<String, f64> = BTreeMap::new();
    let cs = circuits(m, level);
    let mut list: Vec<&Circuit> = cs.values().collect();
    list.sort_by_key(|a| natural(&a.id.key));
    for c in list {
        let (route, estimated) = circuit_length(m, c);
        let sizes: BTreeSet<String> = c.conduit.iter().filter_map(|e| e.text("size")).collect();
        let wire = c.wire.clone().or_else(|| c.conduit.iter().find_map(|e| e.first(&["wire", "wire_size"])));
        let parsed = wire.as_deref().and_then(codes::Wire::parse);
        let count = parsed.as_ref().map_or(0, codes::Wire::count);
        let size = parsed.as_ref().and_then(|w| w.phase_size().map(str::to_string)).unwrap_or_default();
        let drops = drop_ft * c.devices.len() as f64;
        let wire_lf = f64::from(count) * (route + drops) * (1.0 + makeup / 100.0);
        if count > 0 {
            *by_size.entry(size.clone()).or_insert(0.0) += wire_lf;
        }
        s.push(vec![
            c.id.key.clone(),
            c.devices.len().to_string(),
            joined(&sizes),
            n2(route),
            if estimated { "estimated route".into() } else { "conduit".into() },
            wire.unwrap_or_default(),
            count.to_string(),
            size,
            n2(wire_lf),
        ]);
    }
    for (size, lf) in by_size {
        s.notes.push(format!("#{size}: {} lf of wire", n2(lf)));
    }
    s.notes.push(format!(
        "wire lf = conductors × (route + {} ft drop per device) × (1 + {}% makeup) (assumed allowances; set limits.drop_ft / limits.makeup_pct)",
        n2(drop_ft),
        n2(makeup)
    ));
    s
}

fn run_schedule(s: &mut Session, p: &Value) -> Result<Value> {
    let kind = str_param(p, "kind").map(|k| k.trim().to_ascii_lowercase()).unwrap_or_default();
    let level = str_param(p, "level").map(str::trim).filter(|l| !l.is_empty()).map(str::to_string);
    let lv = level.as_deref();
    let limits = p.get("limits").cloned().unwrap_or(Value::Null);
    let d = s.doc()?;
    let m = Model::new(d);
    let mut extra = Value::Null;
    let sched = match kind.as_str() {
        "device" | "devices" => {
            let by = str_param(p, "by").unwrap_or("room").to_ascii_lowercase();
            if !matches!(by.as_str(), "room" | "circuit") {
                return Err(bad(SCHEDULE, "`by` is room or circuit"));
            }
            sched_device(&m, by == "circuit", lv)
        }
        "door" | "doors" => sched_each(
            &m,
            &["door"],
            &["number", "width", "height", "frame", "hardware_set", "fire_rating", "from_room", "to_room", "level", "handle"],
            "number",
            lv,
            |_| true,
        ),
        "window" | "windows" => sched_each(
            &m,
            &["window"],
            &["number", "type_mark", "width", "height", "sill_height", "fire_rating", "room", "level", "handle"],
            "number",
            lv,
            |_| true,
        ),
        "room_finish" | "room" | "rooms" | "finish" => sched_each(
            &m,
            &["room"],
            &["number", "name", "level", "area", "floor_finish", "wall_finish", "ceiling_finish", "ceiling_height", "handle"],
            "number",
            lv,
            |_| true,
        ),
        "light_fixture" | "lighting" | "lighting_fixture" => sched_lights(&m, lv),
        "equipment" => sched_each(
            &m,
            &["equipment"],
            &[
                "tag",
                "type",
                "model",
                "capacity",
                "cfm",
                "voltage",
                "mca",
                "mocp",
                "breaker",
                "fed_from",
                "circuit",
                "served_by",
                "room",
                "level",
                "handle",
            ],
            "tag",
            lv,
            |_| true,
        ),
        "plumbing_fixture" | "plumbing" | "fixture" => sched_plumbing(&m, lv),
        "air_distribution" | "air" => sched_air(&m, lv),
        "panel" | "panels" => {
            let (sc, sum) = sched_panel(&m, lv);
            extra = sum;
            sc
        }
        "wire" | "wire_conduit" | "conduit" => sched_wire(&m, lv, &limits),
        "duct" | "pipe" => {
            let keys = if kind == "duct" { json!(["system", "size", "gauge", "insulation"]) } else { json!(["system", "size", "material"]) };
            let mut q = json!({"group_by": keys, "filter": {"kind": kind}});
            if let Some(l) = lv {
                q["filter"]["level"] = json!(l);
            }
            let mut out = grouped(s, SCHEDULE, &q)?;
            out["schedule"] = json!(kind);
            if let Some(path) = str_param(p, "file").map(str::trim).filter(|x| !x.is_empty()) {
                let rows: Vec<Vec<String>> = out["csv"].as_str().unwrap_or("").lines().map(|l| l.split(',').map(str::to_string).collect()).collect();
                out["path"] = write_table(SCHEDULE, path, &rows, "Schedule")?;
            }
            return Ok(out);
        }
        "" => {
            return Err(bad(
                SCHEDULE,
                "`kind` is required: device, door, window, room_finish, light_fixture, equipment, plumbing_fixture, air_distribution, panel, wire, duct or pipe",
            ));
        }
        other => {
            return Err(bad(
                SCHEDULE,
                format!(
                    "no `{other}` schedule: device, door, window, room_finish, light_fixture, equipment, plumbing_fixture, air_distribution, panel, wire, duct or pipe"
                ),
            ));
        }
    };
    let table: Vec<Vec<String>> =
        std::iter::once(sched.columns.iter().map(|c| (*c).to_string()).collect()).chain(sched.rows.iter().cloned()).collect();
    let rows: Vec<Value> = sched
        .rows
        .iter()
        .map(|r| sched.columns.iter().zip(r).map(|(c, v)| ((*c).to_string(), json!(v))).collect::<serde_json::Map<_, _>>().into())
        .collect();
    let mut out =
        json!({"schedule": kind, "columns": sched.columns, "rows": rows, "count": sched.rows.len(), "notes": sched.notes, "csv": csv_text(&table)});
    if !extra.is_null() {
        out["panels"] = extra;
    }
    if m.els.is_empty() {
        out["next_step"] = json!("the model has no elements yet: build it with model.build or bim.build");
    }
    if let Some(path) = str_param(p, "file").map(str::trim).filter(|x| !x.is_empty()) {
        out["path"] = write_table(SCHEDULE, path, &table, "Schedule")?;
    }
    Ok(out)
}

/// Property coverage per kind, for a job's report: elements, and how many of the kind's
/// standard keys have values (filled / possible, percent).
pub fn coverage(d: &Drawing) -> Value {
    let sum = super::elemprops::summary(d, None);
    let mut out = serde_json::Map::new();
    if let Some(kinds) = sum["kinds"].as_object() {
        for (k, v) in kinds {
            let count = v["count"].as_u64().unwrap_or(0);
            let standard: Vec<&str> = v["standard"].as_array().map(|a| a.iter().filter_map(Value::as_str).collect()).unwrap_or_default();
            let filled: u64 = standard.iter().map(|s| v["filled"][*s].as_u64().unwrap_or(0).min(count)).sum();
            let possible = count * standard.len() as u64;
            let missing: Vec<&str> = standard.iter().copied().filter(|s| v["filled"][*s].as_u64().unwrap_or(0) == 0).collect();
            let assumed: u64 = v["sources"]
                .as_object()
                .map(|o| {
                    o.values()
                        .filter_map(Value::as_object)
                        .flat_map(|m| m.iter())
                        .filter(|(src, _)| SourceKind::parse(src).is_some_and(SourceKind::is_assumed))
                        .filter_map(|(_, n)| n.as_u64())
                        .sum()
                })
                .unwrap_or(0);
            out.insert(
                k.clone(),
                json!({
                    "elements": count, "filled": filled, "possible": possible,
                    "percent": if possible > 0 { ((filled as f64 / possible as f64) * 1000.0).round() / 10.0 } else { 0.0 },
                    "assumed_values": assumed, "keys_with_no_value": missing,
                }),
            );
        }
    }
    json!(out)
}
