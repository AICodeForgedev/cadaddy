//! Trade atoms (Q6162): every trade's physical pieces as model elements, as mepatoms made the
//! electrical runs: each piece one element with its properties, its 3D body, what it connects to
//! and where what it is came from, counted in the model's index (`atoms/index.json`), so
//! `bim.count {trade, kind}` answers how many studs, sheets or cubic yards there are with a
//! lookup, never an estimate formula.
//!
//! - **Concrete** (`floors`): each pour (the spec's slabs, footings, beams, pads; else the
//!   lowest level's slab on grade under the built floor) with its volume, thickness, mix and
//!   pour; inside it its reinforcement as pieces (bars cut to stock with their laps, welded wire
//!   sheets with their laps, post-tensioning tendons with their anchors, chairs) and the vapor
//!   barrier under it; the conduit and pipe pieces that cross it are linked to it.
//! - **Framing** (`walls`, `floors`): studs at their spacing per wall type (king and jack studs
//!   at openings, cripples over and under them), plates or tracks top and bottom cut to stock,
//!   headers, fire blocking, backing behind the devices, casework and accessories that need it,
//!   sheathing panels; floor joists, rim and subfloor panels and roof trusses where the
//!   documents give the floor and roof framing.
//! - **Masonry**: a wall type's block courses.
//! - **Drywall**: each sheet of each layer on each face, laid out over the wall's face with the
//!   openings taken out; corner bead at outside corners and free ends, tape by the joints the
//!   layout makes, joint compound and screws by their rules; ceilings the same.
//! - **Insulation**: a batt in each stud bay (its R-value by the cavity), boards by the sheet.
//! - **Firestopping** (`openings`): one system at each place a pipe, duct, conduit or cable
//!   piece passes through a fire-rated wall.
//! - **Finishes** (`rooms`): paint per surface and coat, flooring per room by its finish (pieces
//!   by the product's size when the documents give it), base by length, ceiling tiles and grid.
//! - **Openings** (`openings`): each door's frame and hardware set, each window (doors and
//!   windows are counted as themselves).
//! - **Specialties** (`rooms`): toilet accessories in the rooms their submittal names, casework.
//!
//! What each piece is comes from the documents first: the wall type's layers (the partition
//! legend), the submittals and specification sections `bim.atoms.docs` reads into the building
//! spec (`trades.facts`); else the trade rules (`tradeatoms.json`: stud spacing, sheet sizes,
//! laps, chair spacing… each with its source; waste values are factors that never change a
//! count). A project overrides any rule with project rules (`custom/trade:<section>.<key>`,
//! `stud_layout/<wall type>`, `joist_layout/general`).
//!
//! The pieces live on their own layers (`3D-Atoms-<Trade>`), created off: they sit inside their
//! walls and slabs, and the shell stays the view until a person turns a trade's layer on.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use cadaddy_doc::props::{self, Prop, Props, Source, SourceKind};
use cadaddy_doc::{Common, Drawing, EntityKind, Handle, Mesh, RawTag, Space};
use cadaddy_geom::{Vec2, Vec3};
use cadaddy_walk3d::BuildingSpec;
use serde_json::{Value, json};

use super::*;
use crate::{Result, Session};

mod docs;
mod floors;
mod openings;
mod rooms;
mod walls;

pub(crate) use docs::Fact;

/// The rules data.
const RULES: &str = include_str!("tradeatoms.json");
/// The trades, in the order a report lists them.
pub(crate) const TRADES: &[&str] = &["concrete", "framing", "masonry", "drywall", "insulation", "firestopping", "finishes", "openings", "specialties"];
/// Most pieces one build makes, elements looked at (hostile models).
const MAX_PIECES: usize = 1_500_000;
const MAX_ELEMENTS: usize = 2_000_000;
/// Most boxes one piece's body has.
const MAX_BLOCKS: usize = 64;
/// The layers the pieces go on.
const LAYER_PREFIX: &str = "3D-Atoms-";

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new("bim.atoms.build", "Lay Out Trade Pieces", run_build)
            .params("{trades?: [concrete|framing|masonry|drywall|insulation|firestopping|finishes|openings|specialties] (default all)} → {pieces, byTrade: {trade: {kind: n}}, ms, notes, indexRows}: the built model's walls, floors, ceilings, rooms, doors and windows laid out again as their trades' pieces (model.build does this itself); the pieces of the trades named are replaced, the count index rebuilt"),
        CommandSpec::new("bim.atoms.report", "Trade Pieces Report", run_report)
            .params("{level?, trade?, group_by?: kind|item (default kind)} → {levels: [{level, trades: [{trade, rows: [{kind, item?, count, pieces?, length_ft, area_sf?, volume_cf?, volume_cy?, weight_lb?}], waste_factor?, with_waste?}]}], totals: {trade: {count, length_ft, area_sf?, volume_cy?, weight_lb?}}, compare: [{what, documents, source, model, model_query, difference_pct?}], answer_ms: {query: ms}, rules_from: {documents, defaults, project}}: per level and trade the pieces and their totals from the count index, beside the quantities the documents give")
            .noundo(),
        CommandSpec::new("bim.atoms.rules", "Trade Piece Rules", run_rules)
            .params("{section?: framing|drywall|insulation|masonry|concrete|finishes|firestopping|openings|specialties} → {rules: {section: {key: {value, source}}}, facts: [{trade, key, value, source}], how_to_override}: the trade rules the layouts use, each value with where it came from (the documents, a project rule, the defaults)")
            .noundo(),
        CommandSpec::new("bim.atoms.docs", "Read Trade Documents", run_docs).params(
            "{paths: [file or folder] (submittals, specification sections; PDF or text), store?: bool (default true: keep the facts in the building spec for the layouts)} → {files: [{file, facts, trades}], facts: [{trade, key, value, number?, source, quote}], by_trade: {trade: n}, unread: [{file, why}]}: what the trades' documents say that the pieces are laid out from: concrete mixes and strengths, slab thickness, post-tensioning strand, tendon and chair spacing, vapor barrier, joist and truss spacing, depth and listed quantities, insulation R-values, paint coats, flooring sizes, finish level, door hardware sets and their doors, toilet accessories and their quantities",
        ),
    ]
}

// ─── Rules ───────────────────────────────────────────────────────────────────

/// The trade rules in force: the data, the documents' facts over it, the project's rules over
/// both, each value's source beside it (`<key>_source`).
#[derive(Clone, Debug)]
pub(crate) struct Rules {
    v: Value,
    /// What set each overridden path: `documents` or `project`.
    from: BTreeMap<String, &'static str>,
    pub facts: Vec<Fact>,
    /// The project's stud spacing per wall type (`stud_layout/<wall type>`), inches and why.
    pub stud_by_type: BTreeMap<String, (f64, String)>,
}

impl Default for Rules {
    fn default() -> Self {
        Rules { v: serde_json::from_str(RULES).unwrap_or(Value::Null), from: BTreeMap::new(), facts: Vec::new(), stud_by_type: BTreeMap::new() }
    }
}

/// A rule value given as text: JSON when it reads as JSON (a number, a list), else the text.
fn rule_value(text: &str) -> Value {
    serde_json::from_str::<Value>(text.trim()).ok().filter(|v| !v.is_object()).unwrap_or_else(|| Value::String(text.trim().to_string()))
}

impl Rules {
    /// The rules for a drawing: the data, its spec's documents and its project rules.
    pub(crate) fn of(d: &Drawing, sp: &BuildingSpec) -> Rules {
        let mut r = Rules { facts: docs::facts_of(sp), ..Rules::default() };
        // The documents' facts that are rule values.
        for f in r.facts.clone() {
            if let Some(path) = docs::rule_path(&f.key)
                && let Some(n) = f.number
            {
                r.set(path, json!(n), &format!("{} ({})", f.source, f.quote.chars().take(80).collect::<String>()), "documents");
            }
        }
        // The spec's own trade rules (a project's, written into the spec).
        if let Some(obj) = sp.trades.get("rules").and_then(Value::as_object) {
            for (path, v) in obj.iter().take(500) {
                r.set(path, v.clone(), "the building spec's trade rules", "project");
            }
        }
        use cadaddy_doc::projrules::RuleDomain;
        for e in d.project_rules.list(Some(RuleDomain::Custom)).into_iter().take(500) {
            if let Some(path) = e.key.strip_prefix("trade:") {
                let v = e.active.value.clone();
                r.set(path, rule_value(&v), &format!("project rule custom/{}: {v}", e.key), "project");
            }
        }
        for e in d.project_rules.list(Some(RuleDomain::StudLayout)).into_iter().take(500) {
            let v = e.active.value.clone();
            if e.key != "general"
                && let Some(n) = lead_num(&v)
            {
                r.stud_by_type.insert(e.key.to_ascii_uppercase(), (n, format!("project rule stud_layout/{}: {v}", e.key)));
            }
        }
        if let Some(e) = d.project_rules.get(RuleDomain::StudLayout, "general") {
            let v = e.active.value.clone();
            if let Some(n) = lead_num(&v) {
                r.set("framing.stud_spacing_in", json!(n), &format!("project rule stud_layout/general: {v}"), "project");
            }
        }
        if let Some(e) = d.project_rules.get(RuleDomain::JoistLayout, "general") {
            let v = e.active.value.clone();
            if let Some(n) = lead_num(&v) {
                r.set("framing.joist_spacing_in", json!(n), &format!("project rule joist_layout/general: {v}"), "project");
            }
        }
        r
    }

    /// Set `path` (`section.key`) to `v`, its source `why`.
    fn set(&mut self, path: &str, v: Value, why: &str, from: &'static str) {
        let Some((section, key)) = path.split_once('.') else { return };
        if key.ends_with("_source") || section.len() > 40 || key.len() > 80 {
            return;
        }
        if !self.v.get(section).is_some_and(Value::is_object) {
            if let Some(o) = self.v.as_object_mut() {
                o.insert(section.to_string(), json!({}));
            } else {
                return;
            }
        }
        if let Some(o) = self.v.get_mut(section).and_then(Value::as_object_mut) {
            o.insert(key.to_string(), v);
            o.insert(format!("{key}_source"), json!(why));
            self.from.insert(path.to_string(), from);
        }
    }

    pub(crate) fn get(&self, path: &str) -> Option<&Value> {
        let (section, key) = path.split_once('.')?;
        self.v.get(section)?.get(key)
    }

    pub(crate) fn num(&self, path: &str, fallback: f64) -> f64 {
        self.get(path).and_then(Value::as_f64).filter(|v| v.is_finite()).unwrap_or(fallback)
    }

    pub(crate) fn text(&self, path: &str) -> String {
        self.get(path).and_then(Value::as_str).unwrap_or_default().to_string()
    }

    pub(crate) fn list(&self, path: &str) -> Vec<Value> {
        self.get(path).and_then(Value::as_array).cloned().unwrap_or_default()
    }

    /// Numbers of a list (`[8, 9, 10, 12]`).
    pub(crate) fn nums(&self, path: &str) -> Vec<f64> {
        self.list(path).iter().filter_map(Value::as_f64).filter(|v| v.is_finite() && *v > 0.0).take(64).collect()
    }

    /// Words of a list, upper case.
    pub(crate) fn words(&self, path: &str) -> Vec<String> {
        self.list(path).iter().filter_map(Value::as_str).map(str::to_ascii_uppercase).take(64).collect()
    }

    /// Where `path`'s value came from: its own `_source`, else the nearest shorter key's
    /// (`stud_spacing_in` → `stud_spacing_source`), else the section's defaults.
    pub(crate) fn source_text(&self, path: &str) -> String {
        let Some((section, key)) = path.split_once('.') else { return "trade rules".into() };
        let sec = self.v.get(section);
        let mut k = key.to_string();
        loop {
            if let Some(t) = sec.and_then(|s| s.get(format!("{k}_source"))).and_then(Value::as_str) {
                return t.to_string();
            }
            match k.rfind('_') {
                Some(i) => k.truncate(i),
                None => {
                    // The section's own source, else the defaults'.
                    return sec.and_then(|s| s.get("source")).and_then(Value::as_str).map_or_else(|| format!("company default (trade rules, {section})"), str::to_string);
                }
            }
        }
    }

    /// `path`'s source as a property source: a document's, a project rule, a code citation or a
    /// company default.
    pub(crate) fn source(&self, path: &str) -> Source {
        let text = self.source_text(path);
        match self.from.get(path).copied() {
            Some("documents") => Source::from_doc(doc_kind(&text), &text),
            Some(_) => Source::from_doc(SourceKind::Rule, &text),
            None => super::mepatoms::rule_source(&text),
        }
    }

    /// Whether `path`'s value is the data's default (not the documents' or the project's).
    pub(crate) fn is_default(&self, path: &str) -> bool {
        !self.from.contains_key(path)
    }

    /// The first fact of `key`, if the documents give it.
    pub(crate) fn fact(&self, key: &str) -> Option<&Fact> {
        self.facts.iter().find(|f| f.key == key)
    }

    pub(crate) fn facts_of(&self, key: &str) -> impl Iterator<Item = &Fact> {
        self.facts.iter().filter(move |f| f.key == key)
    }
}

/// The kind of document a source text names: a submittal, a specification section, else a print.
pub(crate) fn doc_kind(text: &str) -> SourceKind {
    let up = text.to_ascii_uppercase();
    if up.contains("SUBMITTAL") {
        SourceKind::Submittal
    } else if up.contains("SPEC") || up.contains("SECTION") {
        SourceKind::Spec
    } else {
        SourceKind::Print
    }
}

/// Whether a trade asked for (`framing`, `Drywall`, `drywall pieces`) is `have`.
pub(crate) fn trade_eq(want: &str, have: &str) -> bool {
    let w = want.trim().to_ascii_lowercase();
    let w = match w.as_str() {
        "structure" | "structural" | "reinforcement" | "rebar" | "pt" => "concrete",
        "gypsum" | "gyp" | "board" | "sheetrock" => "drywall",
        "studs" | "carpentry" | "rough carpentry" | "wood" => "framing",
        "cmu" | "block" => "masonry",
        "firestop" => "firestopping",
        "paint" | "flooring" | "finish" => "finishes",
        "doors" | "windows" | "hardware" => "openings",
        "accessories" | "toilet accessories" | "casework" => "specialties",
        x => x,
    };
    w == have.trim().to_ascii_lowercase()
}

// ─── Pieces ──────────────────────────────────────────────────────────────────

/// A box body: centreline `a`–`b` in plan, `t` across it, from `z0` up to `z1` (sheet units).
#[derive(Clone, Copy, Debug)]
pub(crate) struct Block {
    pub a: Vec2,
    pub b: Vec2,
    pub t: f64,
    pub z0: f64,
    pub z1: f64,
}

impl Block {
    pub(crate) fn new(a: Vec2, b: Vec2, t: f64, z0: f64, z1: f64) -> Block {
        Block { a, b, t, z0, z1 }
    }
    fn ok(&self) -> bool {
        self.a.is_finite() && self.b.is_finite() && self.t.is_finite() && self.z0.is_finite() && self.z1.is_finite() && self.t > 0.0 && self.z1 > self.z0
    }
}

/// One piece of a trade as a layout makes it.
#[derive(Clone, Debug, Default)]
pub(crate) struct Piece {
    pub trade: &'static str,
    pub kind: &'static str,
    /// What it is bought as (`5/8" Type X gypsum board 4x10`).
    pub item: String,
    /// Its type or material (`Type X`, `steel 20 ga`, `4000 psi`).
    pub ty: String,
    /// Its size as text (`5/8"`, `3 5/8"`, `#4`).
    pub size: String,
    pub level: String,
    pub room: String,
    /// The element it is part of (a wall, a slab, a room, a door).
    pub host: Option<Handle>,
    pub length_ft: f64,
    pub area_sf: f64,
    pub volume_cf: f64,
    pub weight_lb: f64,
    /// Pieces it stands for (0: one).
    pub qty: f64,
    pub blocks: Vec<Block>,
    /// More properties with their sources.
    pub extra: Vec<(&'static str, String, Source)>,
    /// Where what it is came from (the wall type, the submittal, a rule).
    pub source: Source,
    /// Other pieces of the same layout it connects to (their indices).
    pub links: Vec<usize>,
    /// Elements it connects to (a host's neighbours, a penetrating pipe).
    pub touches: Vec<Handle>,
    /// A body that is not boxes (a slab's outline, a room's floor).
    pub custom: Option<Mesh>,
}

impl Piece {
    pub(crate) fn new(trade: &'static str, kind: &'static str, item: impl Into<String>, level: &str, source: Source) -> Piece {
        Piece { trade, kind, item: item.into(), level: level.to_string(), source, ..Piece::default() }
    }
    pub(crate) fn with(mut self, key: &'static str, value: impl Into<String>, source: Source) -> Piece {
        self.extra.push((key, value.into(), source));
        self
    }
    pub(crate) fn computed(self, key: &'static str, value: impl Into<String>) -> Piece {
        self.with(key, value, Source::computed())
    }
}

/// The pieces of one build, in order.
#[derive(Default)]
pub(crate) struct Batch {
    pub pieces: Vec<Piece>,
    pub notes: Vec<String>,
    /// Trades left out (with why), for the report.
    pub missing: BTreeMap<String, String>,
}

impl Batch {
    /// Add a piece; its index (pieces past the cap are dropped and say so once).
    pub(crate) fn push(&mut self, p: Piece) -> usize {
        if self.pieces.len() >= MAX_PIECES {
            if self.pieces.len() == MAX_PIECES {
                self.notes.push(format!("stopped at {MAX_PIECES} pieces (the cap)"));
            }
            return usize::MAX;
        }
        self.pieces.push(p);
        self.pieces.len() - 1
    }
    pub(crate) fn link(&mut self, a: usize, b: usize) {
        if a == b || a == usize::MAX || b == usize::MAX {
            return;
        }
        if let Some(p) = self.pieces.get_mut(a)
            && p.links.len() < 64
        {
            p.links.push(b);
        }
        if let Some(p) = self.pieces.get_mut(b)
            && p.links.len() < 64
        {
            p.links.push(a);
        }
    }
}

/// A number as stored: up to 4 decimals, no trailing zeros.
pub(crate) fn num_text(v: f64) -> String {
    if !v.is_finite() {
        return "0".into();
    }
    let t = format!("{v:.4}");
    let t = t.trim_end_matches('0').trim_end_matches('.');
    if t.is_empty() || t == "-0" { "0".into() } else { t.to_string() }
}

/// Inches as a builder writes them: `5/8"`, `3 1/2"`, `12"`.
pub(crate) fn inches_text(v: f64) -> String {
    if !(v.is_finite() && v > 0.0) {
        return String::new();
    }
    let e = (v * 16.0).round() as i64;
    let (w, r) = (e / 16, e % 16);
    let g = gcd(r, 16);
    let frac = if r == 0 { String::new() } else { format!("{}/{}", r / g, 16 / g) };
    match (w, frac.is_empty()) {
        (0, false) => format!("{frac}\""),
        (w, true) => format!("{w}\""),
        (w, false) => format!("{w} {frac}\""),
    }
}

fn gcd(a: i64, b: i64) -> i64 {
    if b == 0 { a.max(1) } else { gcd(b, a % b) }
}

/// The trade's layer.
fn layer_of(trade: &str) -> String {
    let mut c = trade.chars();
    let title: String = c.next().map(|f| f.to_ascii_uppercase().to_string() + c.as_str()).unwrap_or_default();
    format!("{LAYER_PREFIX}{title}")
}

fn trade_color(trade: &str) -> u8 {
    match trade {
        "concrete" => 8,
        "framing" => 42,
        "masonry" => 22,
        "drywall" => 254,
        "insulation" => 50,
        "firestopping" => 1,
        "finishes" => 151,
        "openings" => 130,
        _ => 200,
    }
}

/// A piece's body (sheet units).
fn body(p: &Piece) -> Mesh {
    if let Some(m) = p.custom.as_ref().filter(|m| !m.faces.is_empty() && m.vertices.iter().all(|v| v.is_finite())) {
        return m.clone();
    }
    let mut m = super::wallgen::empty_mesh();
    for b in p.blocks.iter().filter(|b| b.ok()).take(MAX_BLOCKS) {
        if b.a.dist(b.b) > 1e-9 {
            super::wallgen::add_box(&mut m, b.a, b.b, b.t, b.z0, b.z1);
        }
    }
    m
}

/// What `emit` made: elements by trade and kind.
pub(crate) struct Emitted {
    pub by_trade: BTreeMap<&'static str, BTreeMap<&'static str, usize>>,
}

/// The batch's pieces into the model, each an element on its trade's layer (created off) with
/// its properties, then what each connects to by handle.
pub(crate) fn emit(d: &mut Drawing, batch: &Batch) -> Emitted {
    let mut by_trade: BTreeMap<&'static str, BTreeMap<&'static str, usize>> = BTreeMap::new();
    let mut handles: Vec<Option<Handle>> = Vec::with_capacity(batch.pieces.len());
    let t = |code: i32, v: String| RawTag { code, value: v.chars().take(250).collect() };
    for p in &batch.pieces {
        let mesh = body(p);
        if mesh.faces.is_empty() {
            handles.push(None);
            continue;
        }
        let layer = layer_of(p.trade);
        if d.layer(&layer).is_none() {
            d.layers.push(cadaddy_doc::Layer {
                name: layer.as_str().into(),
                color: cadaddy_doc::color::Color::Index(trade_color(p.trade)),
                on: false,
                ..cadaddy_doc::Layer::default()
            });
        }
        let tags = vec![
            t(1001, super::wallgen::APP.into()),
            t(1000, "atom".into()),
            t(1000, p.level.clone()),
            t(1000, p.host.unwrap_or(Handle(0)).hex()),
            t(1000, format!("trade:{}", p.trade)),
            t(1000, format!("part:atom:{}", p.kind)),
            t(1000, format!("type:{}", p.item)),
            t(1000, format!("size:{}", p.size)),
        ];
        let ps = piece_props(p);
        let xdata = props::write(&tags, &ps);
        match d.add(&Space::Model, Common { layer: layer.into(), xdata: xdata.into(), ..Common::default() }, EntityKind::Mesh(mesh)) {
            Ok(h) => {
                *by_trade.entry(p.trade).or_default().entry(p.kind).or_default() += 1;
                handles.push(Some(h));
            }
            Err(_) => handles.push(None),
        }
    }
    // What each piece connects to, by handle (both ways for pieces of the batch).
    for (i, p) in batch.pieces.iter().enumerate() {
        if p.links.is_empty() && p.touches.is_empty() {
            continue;
        }
        let Some(h) = handles.get(i).copied().flatten() else { continue };
        let mut cn: Vec<Handle> = p.links.iter().filter_map(|j| handles.get(*j).copied().flatten()).collect();
        cn.extend(p.touches.iter().copied());
        cn.sort_unstable();
        cn.dedup();
        cn.truncate(64);
        if cn.is_empty() {
            continue;
        }
        let text = cn.iter().map(|h| h.hex()).collect::<Vec<_>>().join(",");
        let _ = d.modify_entity(h, |e| {
            let mut ps = props::read(&e.common.xdata);
            ps.set("connects", Prop::new(text, Source::computed()));
            e.common.xdata = props::write(&e.common.xdata, &ps).into();
        });
    }
    Emitted { by_trade }
}

/// A piece's properties.
fn piece_props(p: &Piece) -> Props {
    let mut ps = Props::default();
    let c = Source::computed;
    ps.set(super::mepatoms::ATOM_KEY, Prop::new(p.kind, c()));
    ps.set("trade", Prop::new(p.trade, c()));
    ps.set("item", Prop::new(p.item.clone(), p.source.clone()));
    if !p.ty.is_empty() {
        ps.set("type", Prop::new(p.ty.clone(), p.source.clone()));
    }
    if !p.size.is_empty() {
        ps.set("size", Prop::new(p.size.clone(), p.source.clone()));
    }
    for (k, v, unit) in [("length_ft", p.length_ft, "ft"), ("area_sf", p.area_sf, "sf"), ("volume_cf", p.volume_cf, "cf"), ("weight_lb", p.weight_lb, "lb")] {
        if v > 0.0 && v.is_finite() {
            ps.set(k, Prop::new(num_text(v), c()).with_unit(unit));
        }
    }
    if p.volume_cf > 0.0 && p.volume_cf.is_finite() {
        ps.set("volume_cy", Prop::new(num_text(p.volume_cf / 27.0), c()).with_unit("cy"));
    }
    if p.qty > 0.0 && p.qty.is_finite() && (p.qty - 1.0).abs() > 1e-9 {
        ps.set("qty", Prop::new(num_text(p.qty), c()));
    }
    if !p.room.is_empty() {
        ps.set("room", Prop::new(p.room.clone(), c()));
    }
    if let Some(h) = p.host {
        let mut host = Prop::new(h.hex(), c());
        host.note = Some("the element it is part of".into());
        ps.set("host", host);
    }
    if let Some(doc) = p.source.doc.as_deref() {
        ps.set("source_rule", Prop::new(doc, p.source.clone()));
    }
    for (k, v, s) in p.extra.iter().take(48) {
        ps.set(k, Prop::new(v.clone(), s.clone()));
    }
    ps
}

/// Whether an element is a trade piece (a building trade's atom, not an electrical run's).
pub(crate) fn is_trade_piece(e: &cadaddy_doc::Entity) -> bool {
    super::wallgen::made_as(e).as_deref() == Some("atom")
}

/// The trade a trade piece belongs to.
pub(crate) fn trade_of(e: &cadaddy_doc::Entity) -> Option<String> {
    super::bimlook::made_tag(e, "trade").map(str::to_string)
}

/// Remove the trade pieces (of the trades given, or all); how many.
pub(crate) fn remove(d: &mut Drawing, trades: Option<&BTreeSet<String>>) -> usize {
    let hs: Vec<Handle> = d
        .model
        .iter()
        .filter(|e| is_trade_piece(e))
        .filter(|e| trades.is_none_or(|t| trade_of(e).is_some_and(|x| t.contains(&x))))
        .map(|e| e.handle)
        .take(MAX_ELEMENTS)
        .collect();
    for h in &hs {
        d.model.remove(*h);
    }
    hs.len()
}

/// The host handle a piece's tag names (the WALLGEN tag's source).
fn host_tag(e: &cadaddy_doc::Entity) -> Option<&str> {
    let x = &e.common.xdata;
    let at = x.iter().position(|t| t.code == 1001 && t.value.trim().eq_ignore_ascii_case(super::wallgen::APP))?;
    x.get(at + 3).filter(|t| t.code == 1000).map(|t| t.value.as_str())
}

/// Trim the list `key` of an answer until the answer fits an agent's answer size.
fn fit(out: &mut Value, key: &str) {
    let max = super::agentready::ANSWER_MAX.saturating_sub(500);
    let mut n = out.get(key).and_then(Value::as_array).map_or(0, Vec::len);
    let total = n;
    while n > 0 && out.to_string().len() > max {
        n = n * 3 / 4;
        if let Some(a) = out.get_mut(key).and_then(Value::as_array_mut) {
            a.truncate(n);
        }
    }
    if n < total {
        out[format!("{key}_shown")] = json!(n);
        out[format!("{key}_total")] = json!(total);
    }
}

/// Remove the pieces whose host is one of `gone` (merged copies the identity pass deleted): a
/// deleted wall's studs and sheets go with it. How many.
pub(crate) fn drop_hosted(d: &mut Drawing, gone: &[Handle]) -> usize {
    if gone.is_empty() {
        return 0;
    }
    let gone: BTreeSet<String> = gone.iter().map(|h| h.hex()).collect();
    let hs: Vec<Handle> = d
        .model
        .iter()
        .filter(|e| is_trade_piece(e))
        .filter(|e| host_tag(e).is_some_and(|h| gone.contains(h)))
        .map(|e| e.handle)
        .take(MAX_ELEMENTS)
        .collect();
    for h in &hs {
        d.model.remove(*h);
    }
    if !hs.is_empty() {
        super::mepatoms::reindex(d);
    }
    hs.len()
}

// ─── The build ───────────────────────────────────────────────────────────────

/// Drawing units per foot of a spec.
pub(crate) fn upf_of(sp: &BuildingSpec) -> f64 {
    if sp.units_per_foot.is_finite() && sp.units_per_foot > 0.0 { sp.units_per_foot } else { 12.0 }
}

/// Lay out the trades' pieces of the built model (`model.build` calls it after the MEP atoms);
/// the counts. `only`: the trades to lay out (none: all).
pub(crate) fn build(d: &mut Drawing, sp: &BuildingSpec, only: Option<&BTreeSet<String>>) -> Value {
    let started = std::time::Instant::now();
    let rules = Rules::of(d, sp);
    let want = |t: &str| only.is_none_or(|o| o.contains(t));
    let mut batch = Batch::default();
    let ctx = Ctx::new(d, sp);
    if want("framing") || want("drywall") || want("insulation") || want("masonry") || want("finishes") {
        walls::lay_out(d, sp, &rules, &ctx, &mut batch, &want);
    }
    if want("concrete") || want("framing") {
        floors::lay_out(d, sp, &rules, &ctx, &mut batch, &want);
    }
    if want("finishes") || want("drywall") || want("specialties") || want("framing") {
        rooms::lay_out(d, sp, &rules, &ctx, &mut batch, &want);
    }
    if want("openings") || want("firestopping") {
        openings::lay_out(d, sp, &rules, &ctx, &mut batch, &want);
    }
    let emitted = emit(d, &batch);
    let rows = super::mepatoms::reindex(d);
    let total: usize = emitted.by_trade.values().flat_map(|m| m.values()).sum();
    json!({
        "tradePieces": total,
        "tradePiecesByTrade": emitted.by_trade,
        "tradeNotes": batch.notes.iter().take(40).collect::<Vec<_>>(),
        "tradesMissing": batch.missing,
        "tradeIndexRows": rows,
        "tradeMs": (started.elapsed().as_secs_f64() * 1000.0).round(),
    })
}

/// What every layout looks up: the built walls, rooms, doors and windows by level, the levels'
/// floors and the electrical and MEP pieces (for crossings).
pub(crate) struct Ctx {
    pub upf: f64,
    /// Rooms: level, name, outline (sheet units), handle.
    pub rooms: Vec<RoomEl>,
    /// Devices and casework: level, kind, centre, handle.
    pub devices: Vec<(String, String, Vec3, Handle)>,
}

/// A built room element.
#[derive(Clone, Debug)]
pub(crate) struct RoomEl {
    pub level: String,
    pub name: String,
    pub number: String,
    pub finish: String,
    pub outline: Vec<Vec2>,
    pub z: f64,
    pub handle: Handle,
}

impl Ctx {
    fn new(d: &Drawing, sp: &BuildingSpec) -> Ctx {
        let upf = upf_of(sp);
        let mut rooms = Vec::new();
        let mut devices = Vec::new();
        for e in d.model.iter().filter(|e| super::wallgen::made_as(e).is_some()).take(MAX_ELEMENTS) {
            let part = super::wallgen::made_as(e).unwrap_or_default();
            let level = super::bspec::made_level(e).unwrap_or_default();
            match part.as_str() {
                "room" => {
                    let EntityKind::Mesh(m) = &e.kind else { continue };
                    let outline: Vec<Vec2> = m.vertices.iter().take(10_000).map(|v| Vec2::new(v.x, v.y)).collect();
                    if outline.len() < 3 {
                        continue;
                    }
                    let z = m.vertices.first().map_or(0.0, |v| v.z);
                    let tag = |k: &str| super::bimlook::made_tag(e, k).unwrap_or_default().to_string();
                    let ps = props::read(&e.common.xdata);
                    let finish = ps.get("finish").map(Prop::text).filter(|f| !f.is_empty()).unwrap_or_else(|| tag("finish"));
                    rooms.push(RoomEl { level, name: tag("name"), number: tag("number"), finish, outline, z, handle: e.handle });
                }
                "device" | "casework" => {
                    let Some((lo, hi)) = super::fieldview::mesh_box(e) else { continue };
                    let ps = props::read(&e.common.xdata);
                    let kind = if part == "casework" {
                        "casework".to_string()
                    } else {
                        ps.get("kind").map(Prop::text).unwrap_or_default().replace(' ', "_").to_ascii_lowercase()
                    };
                    let c = Vec3::new((lo[0] + hi[0]) / 2.0, (lo[1] + hi[1]) / 2.0, (lo[2] + hi[2]) / 2.0);
                    devices.push((level, kind, c, e.handle));
                }
                _ => {}
            }
        }
        Ctx { upf, rooms, devices }
    }

    /// The room of a level a plan point is in.
    pub(crate) fn room_at(&self, level: &str, p: Vec2) -> Option<&RoomEl> {
        self.rooms.iter().find(|r| r.level.eq_ignore_ascii_case(level) && cadaddy_geom::point_in_polygon(&r.outline, p))
    }

    /// A room's label: its name and number.
    pub(crate) fn room_label(r: &RoomEl) -> String {
        match (r.name.trim(), r.number.trim()) {
            ("", "") => String::new(),
            (n, "") => n.to_string(),
            ("", k) => k.to_string(),
            (n, k) if n.contains(k) => n.to_string(),
            (n, k) => format!("{n} {k}"),
        }
    }
}

// ─── Commands ────────────────────────────────────────────────────────────────

fn trades_param(cmd: &str, p: &Value) -> Result<Option<BTreeSet<String>>> {
    let Some(v) = p.get("trades").filter(|v| !v.is_null()) else { return Ok(None) };
    let list: Vec<String> = match v {
        Value::String(s) => s.split(',').map(|x| x.trim().to_ascii_lowercase()).filter(|x| !x.is_empty()).collect(),
        Value::Array(a) => a.iter().filter_map(Value::as_str).map(|x| x.trim().to_ascii_lowercase()).take(32).collect(),
        _ => return Err(bad(cmd, "`trades` must be a list of trade names")),
    };
    let mut out = BTreeSet::new();
    for t in list {
        match TRADES.iter().find(|x| trade_eq(&t, x)) {
            Some(x) => {
                out.insert((*x).to_string());
            }
            None => return Err(bad(cmd, format!("`{t}` is not a trade: one of {}", TRADES.join(", ")))),
        }
    }
    Ok(Some(out))
}

fn run_build(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "bim.atoms.build";
    let only = trades_param(CMD, p)?;
    let (sp, _) = super::bspec::current(s, CMD)?;
    let d = s.doc_mut()?;
    if !d.model.iter().any(|e| super::wallgen::made_as(e).as_deref() == Some("wall")) {
        return Err(bad(CMD, "the model has no built walls: build it first (model.build)"));
    }
    let removed = remove(d, only.as_ref());
    let r = build(d, &sp, only.as_ref());
    s.touch();
    Ok(json!({
        "pieces": r["tradePieces"],
        "byTrade": r["tradePiecesByTrade"],
        "removed": removed,
        "ms": r["tradeMs"],
        "notes": r["tradeNotes"],
        "missing": r["tradesMissing"],
        "indexRows": r["tradeIndexRows"],
    }))
}

/// The waste factor of a trade's pieces, from its rules.
fn waste_of(r: &Rules, trade: &str) -> Option<(f64, String)> {
    let path = match trade {
        "drywall" => "drywall.waste_factor",
        "insulation" => "insulation.waste_factor",
        "masonry" => "masonry.waste_factor",
        "concrete" => "concrete.waste_factor",
        "finishes" => "finishes.flooring_waste_factor",
        _ => return None,
    };
    let f = r.num(path, 0.0);
    (f > 0.0 && f < 1.0).then(|| (f, r.source_text(path)))
}

fn run_report(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "bim.atoms.report";
    let level = str_param(p, "level").map(str::to_string);
    let trade = str_param(p, "trade").map(str::to_string);
    let by_item = match str_param(p, "group_by").unwrap_or("kind") {
        "kind" => false,
        "item" => true,
        other => return Err(bad(CMD, format!("`group_by` must be kind or item, not `{other}`"))),
    };
    let (sp, _) = super::bspec::current(s, CMD)?;
    let d = s.doc()?;
    let rules = Rules::of(d, &sp);
    let ix = super::mepatoms::stored(d).unwrap_or_else(|| super::mepatoms::Index::of(d));
    // Per level, trade and kind (or item).
    let mut tree: BTreeMap<String, BTreeMap<String, BTreeMap<(String, String), super::mepatoms::Tally>>> = BTreeMap::new();
    let mut totals: BTreeMap<String, super::mepatoms::Tally> = BTreeMap::new();
    for (k, t) in &ix.rows {
        if k.trade.is_empty() {
            continue;
        }
        if level.as_deref().is_some_and(|l| !l.eq_ignore_ascii_case(&k.level)) || trade.as_deref().is_some_and(|w| !trade_eq(w, &k.trade)) {
            continue;
        }
        let key = (k.kind.clone(), if by_item { k.item.clone() } else { String::new() });
        tree.entry(k.level.clone()).or_default().entry(k.trade.clone()).or_default().entry(key).or_default().merge(t);
        totals.entry(k.trade.clone()).or_default().merge(t);
    }
    let order = |t: &str| TRADES.iter().position(|x| *x == t).unwrap_or(TRADES.len());
    let levels: Vec<Value> = tree
        .iter()
        .map(|(lv, trades)| {
            let mut ts: Vec<(&String, &BTreeMap<(String, String), super::mepatoms::Tally>)> = trades.iter().collect();
            ts.sort_by_key(|(t, _)| order(t));
            json!({
                "level": lv,
                "trades": ts.iter().map(|(t, rows)| {
                    let mut tj = json!({
                        "trade": t,
                        "rows": rows.iter().take(400).map(|((kind, item), tally)| {
                            let mut r = super::mepatoms::tally_json(tally);
                            r["kind"] = json!(kind);
                            if by_item {
                                r["item"] = json!(item);
                            }
                            r
                        }).collect::<Vec<_>>(),
                    });
                    if let Some((f, src)) = waste_of(&rules, t) {
                        tj["waste_factor"] = json!(f);
                        tj["waste_source"] = json!(src);
                    }
                    tj
                }).collect::<Vec<_>>(),
            })
        })
        .collect();
    let mut tot = serde_json::Map::new();
    for (t, tally) in &totals {
        let mut j = super::mepatoms::tally_json(tally);
        if let Some((f, _)) = waste_of(&rules, t) {
            j["with_waste"] = json!({
                "area_sf": (tally.area_sf * (1.0 + f) * 100.0).round() / 100.0,
                "volume_cy": (tally.volume_cf / 27.0 * (1.0 + f) * 100.0).round() / 100.0,
                "pieces": (tally.pieces() * (1.0 + f)).ceil(),
            });
        }
        tot.insert(t.clone(), j);
    }
    // The quantities the documents give, beside the model's.
    let compare = docs::compare(&rules.facts, &ix, level.as_deref());
    // How fast the index answers.
    let mut answer_ms = serde_json::Map::new();
    for (name, q) in [
        ("studs", json!({"trade": "framing", "kind": "stud"})),
        ("drywall sheets by type", json!({"trade": "drywall", "kind": "sheet", "group_by": "type"})),
        ("concrete by level", json!({"trade": "concrete", "kind": "pour", "group_by": "level"})),
        ("everything by trade", json!({"group_by": "trade"})),
    ] {
        let t0 = std::time::Instant::now();
        let f = super::mepatoms::Filter::from_params(&q);
        let _ = ix.query(&f, q.get("group_by").and_then(Value::as_str));
        answer_ms.insert(name.into(), json!((t0.elapsed().as_secs_f64() * 1e6).round() / 1000.0));
    }
    let mut from = BTreeMap::<&str, usize>::new();
    for v in rules.from.values() {
        *from.entry(v).or_default() += 1;
    }
    Ok(json!({
        "levels": levels,
        "totals": tot,
        "compare": compare,
        "answer_ms": answer_ms,
        "rules_from": {"documents": from.get("documents").copied().unwrap_or(0), "project": from.get("project").copied().unwrap_or(0), "facts": rules.facts.len()},
    }))
}

fn run_rules(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "bim.atoms.rules";
    let section = str_param(p, "section").map(str::to_ascii_lowercase);
    let (sp, _) = super::bspec::current(s, CMD)?;
    let d = s.doc()?;
    let r = Rules::of(d, &sp);
    let mut out = serde_json::Map::new();
    if let Some(obj) = r.v.as_object() {
        for (sec, vals) in obj.iter().filter(|(k, _)| !k.starts_with('_')) {
            if section.as_deref().is_some_and(|w| w != sec) {
                continue;
            }
            let Some(vals) = vals.as_object() else { continue };
            let mut m = serde_json::Map::new();
            for (k, v) in vals.iter().filter(|(k, _)| !k.ends_with("_source")) {
                let path = format!("{sec}.{k}");
                m.insert(k.clone(), json!({"value": v, "source": r.source_text(&path), "from": r.from.get(&path).copied().unwrap_or("defaults")}));
            }
            out.insert(sec.clone(), Value::Object(m));
        }
    }
    if out.is_empty()
        && let Some(sec) = section
    {
        return Err(bad(CMD, format!("`{sec}` is not a section of the trade rules")));
    }
    Ok(json!({
        "rules": out,
        "facts": r.facts.iter().take(200).map(Fact::to_json).collect::<Vec<_>>(),
        "how_to_override": "project.rules.set {domain: \"custom\", key: \"trade:<section>.<key>\", value} (or stud_layout/general, joist_layout/general for the spacings); the next model.build or bim.atoms.build lays the pieces out with it",
    }))
}

fn run_docs(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "bim.atoms.docs";
    let paths: Vec<String> = match p.get("paths").or_else(|| p.get("path")) {
        Some(Value::String(x)) => vec![x.clone()],
        Some(Value::Array(a)) => a.iter().filter_map(Value::as_str).map(str::to_string).take(64).collect(),
        _ => return Err(bad(CMD, "`paths` (the submittal and specification files or folders) is required")),
    };
    if paths.is_empty() {
        return Err(bad(CMD, "`paths` is empty: give the submittal and specification files or folders"));
    }
    let store = bool_or(p, "store", true);
    let read = docs::read(s, &paths);
    if read.files.is_empty() && read.unread.is_empty() {
        return Err(bad(CMD, "no PDF or text files at the paths given"));
    }
    if store {
        let (mut sp, _) = super::bspec::current(s, CMD)?;
        docs::store(&mut sp, &read);
        s.doc_mut()?.building_spec = Some(sp.to_json());
        s.touch();
    }
    let mut by_trade = BTreeMap::<&str, usize>::new();
    for f in &read.facts {
        *by_trade.entry(f.trade.as_str()).or_default() += 1;
    }
    let mut out = json!({
        "files": read.files,
        "facts": read.facts.iter().take(300).map(Fact::to_json).collect::<Vec<_>>(),
        "by_trade": by_trade,
        "unread": read.unread,
        "stored": store,
    });
    fit(&mut out, "facts");
    Ok(out)
}

/// Shared geometry helpers for the layouts.
pub(crate) mod geo {
    use super::*;

    /// The intervals `[lo, hi]` of `items` merged where they overlap or touch.
    pub(crate) fn merge(mut items: Vec<(f64, f64)>) -> Vec<(f64, f64)> {
        items.retain(|(a, b)| a.is_finite() && b.is_finite() && b > a);
        items.sort_by(|x, y| x.0.total_cmp(&y.0));
        let mut out: Vec<(f64, f64)> = Vec::new();
        for (a, b) in items {
            match out.last_mut() {
                Some(l) if a <= l.1 + 1e-9 => l.1 = l.1.max(b),
                _ => out.push((a, b)),
            }
        }
        out
    }

    /// Where the line through `p` along `dir` crosses a polygon: the parameter intervals inside.
    pub(crate) fn inside_spans(poly: &[Vec2], p: Vec2, dir: Vec2) -> Vec<(f64, f64)> {
        let n = poly.len();
        if n < 3 {
            return Vec::new();
        }
        let mut ts: Vec<f64> = Vec::new();
        for i in 0..n {
            let (Some(a), Some(b)) = (poly.get(i), poly.get((i + 1) % n)) else { continue };
            let e = *b - *a;
            let den = dir.x * e.y - dir.y * e.x;
            if den.abs() < 1e-12 {
                continue;
            }
            let w = *a - p;
            let t = (w.x * e.y - w.y * e.x) / den;
            let u = (w.x * dir.y - w.y * dir.x) / den;
            if (0.0..1.0).contains(&u) && t.is_finite() {
                ts.push(t);
            }
        }
        ts.sort_by(f64::total_cmp);
        ts.chunks(2).filter_map(|c| if let [a, b] = c { Some((*a, *b)) } else { None }).filter(|(a, b)| b > a).collect()
    }

    /// Cut `total` feet into stock lengths: full sticks then the cut rest; (lengths, cut).
    pub(crate) fn stock_cuts(total: f64, stock: f64) -> Vec<(f64, bool)> {
        if !(total.is_finite() && total > 1e-6 && stock.is_finite() && stock > 0.1) {
            return Vec::new();
        }
        let full = (total / stock).floor().min(100_000.0) as usize;
        let mut out: Vec<(f64, bool)> = (0..full).map(|_| (stock, false)).collect();
        let rest = total - stock * full as f64;
        if rest > 1e-3 {
            out.push((rest, true));
        }
        out
    }

    /// The smallest stock length at least `need`; else the largest (none when the list is empty).
    pub(crate) fn stock_for(need: f64, stock: &[f64]) -> Option<f64> {
        let mut s: Vec<f64> = stock.iter().copied().filter(|v| v.is_finite() && *v > 0.0).collect();
        s.sort_by(f64::total_cmp);
        s.iter().copied().find(|v| *v + 1e-6 >= need).or_else(|| s.last().copied())
    }

    /// A mesh's flat (horizontal) faces as plan polygons, with their height.
    pub(crate) fn flat_faces(m: &Mesh) -> Vec<(Vec<Vec2>, f64)> {
        let mut out = Vec::new();
        for f in m.faces.iter().take(100_000) {
            let pts: Vec<Vec3> = f.v.iter().filter_map(|i| m.vertices.get(*i as usize).copied()).collect();
            if pts.len() < 3 {
                continue;
            }
            let z = pts.iter().map(|p| p.z).sum::<f64>() / pts.len() as f64;
            if !pts.iter().all(|p| (p.z - z).abs() < 1e-6) {
                continue;
            }
            let poly: Vec<Vec2> = pts.iter().map(|p| Vec2::new(p.x, p.y)).collect();
            if cadaddy_geom::shoelace(&poly).abs() > 1e-9 {
                out.push((poly, z));
            }
        }
        out
    }

    /// The up-facing faces of a mesh as polygons (plan) and their top height.
    #[allow(dead_code)]
    pub(crate) fn top_faces(m: &Mesh) -> Vec<(Vec<Vec2>, f64)> {
        let mut out = Vec::new();
        for f in m.faces.iter().take(100_000) {
            let pts: Vec<Vec3> = f.v.iter().filter_map(|i| m.vertices.get(*i as usize).copied()).collect();
            if pts.len() < 3 {
                continue;
            }
            let poly: Vec<Vec2> = pts.iter().map(|p| Vec2::new(p.x, p.y)).collect();
            let a = cadaddy_geom::shoelace(&poly);
            let z = pts.iter().map(|p| p.z).sum::<f64>() / pts.len() as f64;
            let flat = pts.iter().all(|p| (p.z - z).abs() < 1e-6);
            if flat && a > 1e-9 {
                out.push((poly, z));
            }
        }
        out
    }
}

/// A number at the start of a rule's text (`16`, `16" o.c.`, `24 in`).
pub(crate) fn lead_num(v: &str) -> Option<f64> {
    let t: String = v.trim().chars().take_while(|c| c.is_ascii_digit() || *c == '.').collect();
    t.parse::<f64>().ok().filter(|x| x.is_finite() && *x > 0.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rules_read_and_cite_their_sources() {
        let r = Rules::default();
        assert_eq!(r.num("framing.stud_spacing_in", 0.0), 16.0);
        assert!(r.source_text("framing.stud_spacing_in").contains("16\" o.c."));
        // A key with no source of its own takes the nearest shorter key's.
        assert!(r.source_text("drywall.sheet_lengths_ft").contains("trade stock"));
        assert_eq!(r.nums("drywall.sheet_lengths_ft"), vec![8.0, 9.0, 10.0, 12.0]);
        assert_eq!(r.source("drywall.screws_per_sf").kind, SourceKind::Default);
        assert_eq!(r.source("firestopping.item").kind, SourceKind::Rule);
    }

    #[test]
    fn a_project_rule_overrides_the_data_and_says_so() {
        let mut r = Rules::default();
        r.set("framing.stud_spacing_in", json!(24), "project rule custom/trade:framing.stud_spacing_in: 24", "project");
        assert_eq!(r.num("framing.stud_spacing_in", 0.0), 24.0);
        assert!(!r.is_default("framing.stud_spacing_in"));
        assert_eq!(r.source("framing.stud_spacing_in").kind, SourceKind::Rule);
        // A source key can't be set as a value.
        r.set("framing.stud_spacing_in_source", json!("x"), "y", "project");
        assert!(r.source_text("framing.stud_spacing_in").starts_with("project rule"));
    }

    #[test]
    fn helpers() {
        assert_eq!(inches_text(0.625), "5/8\"");
        assert_eq!(inches_text(3.625), "3 5/8\"");
        assert_eq!(inches_text(12.0), "12\"");
        assert_eq!(inches_text(f64::NAN), "");
        assert_eq!(num_text(1.23456), "1.2346");
        assert_eq!(num_text(2.0), "2");
        assert_eq!(geo::stock_cuts(25.0, 10.0), vec![(10.0, false), (10.0, false), (5.0, true)]);
        assert!(geo::stock_cuts(f64::INFINITY, 10.0).is_empty());
        assert_eq!(geo::stock_for(8.5, &[8.0, 9.0, 10.0]), Some(9.0));
        assert_eq!(geo::stock_for(30.0, &[8.0, 12.0]), Some(12.0));
        assert_eq!(geo::merge(vec![(0.0, 1.0), (0.5, 2.0), (3.0, 4.0)]), vec![(0.0, 2.0), (3.0, 4.0)]);
        let sq = vec![Vec2::new(0.0, 0.0), Vec2::new(10.0, 0.0), Vec2::new(10.0, 10.0), Vec2::new(0.0, 10.0)];
        let spans = geo::inside_spans(&sq, Vec2::new(-5.0, 5.0), Vec2::new(1.0, 0.0));
        assert_eq!(spans.len(), 1);
        assert!((spans[0].0 - 5.0).abs() < 1e-9 && (spans[0].1 - 15.0).abs() < 1e-9);
        assert!(trade_eq("Studs", "framing") && trade_eq("gyp", "drywall") && !trade_eq("paint", "drywall"));
        assert_eq!(lead_num("16\" o.c."), Some(16.0));
        assert_eq!(rule_value("24"), json!(24));
        assert_eq!(rule_value("[8, 10]"), json!([8, 10]));
        assert_eq!(rule_value("vertical"), json!("vertical"));
    }
}
