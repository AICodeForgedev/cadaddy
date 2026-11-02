//! The print validator (Q321): keep the model true to the documents. A deterministic check an
//! agent or a person runs like a compiler or a test suite after every change; the checks
//! themselves are [`cadaddy_walk3d::validate`].
//!
//! - `bim.prints {prints?, merge?, fromPlan?, gate?, clear?}`: what the documents say (symbols,
//!   schedule rows, dimension strings, rooms, the rules file, submittals, lugs), kept in the
//!   building spec, so it is saved with the drawing;
//! - `bim.validate {checks?, level?, handles?, area?, severity?, limit?}`: the pass/fail list,
//!   each failure with its element, rule, expected against found, source, severity and fix;
//!   with `handles` or `area` only what is within reach of them is checked (fast re-checks);
//! - `bim.validate.overlay {id, path?}`: a picture of one failure: the print's symbols (grey
//!   crosses), the model's elements (blue), the failure (red);
//! - `bim.source {handles, status, source?, homerun?, tag?, kind?}`: an element's provenance.
//!
//! The model read back from the drawing ([`model_of`]): devices are the plan's device and
//! fixture blocks as Walk derives them (kind = their type attribute, else the block name, in
//! lower case; mark = their BIM type mark; height and mounting from the device rules), walls
//! the walls the last build made (else the plan's wall faces), openings its doors and
//! windows, runs the lines on run layers and lines or polylines whose BIM type is a raceway,
//! duct or pipe. Coordinates are building feet on each level's plan, as the prints'.
//!
//! The gate ([`gate_before`], [`gate_after`]): an agent's edit (MCP, the control channel) of a
//! drawing whose prints have `settings.gate` warn or refuse is re-checked round what it
//! changed; failures it added come back with the result (`validation.new`), and with refuse
//! the edit is undone and refused.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::Arc;

use cadaddy_doc::{Common, Drawing, Entity, EntityKind, Handle, RawTag, Space};
use cadaddy_geom::{Bounds2, Vec2, Vec3};
use cadaddy_walk3d::validate::{
    self as v, Check, Element, GateMode, Model, ModelLevel, ModelOpening, ModelWall, Mounting, PrintSheet, Prints, Report, Run, Scope, Severity,
    Status, Symbol,
};
use cadaddy_walk3d::{BuildingSpec, Role, level_at};
use serde_json::{Value, json};

use super::bspec::current;
use super::*;
use crate::{Result, Session};

/// Provenance in XDATA: `status=…`, `source=…`, `homerun=…`, `tag=…`, `kind=…` strings.
pub const PROV_APP: &str = "CADADDY_PROVENANCE";
/// Failures listed by default.
const DEFAULT_LIMIT: usize = 500;
/// Entities read for the model, at most: every one a drawing holds. A print set imported as
/// linework runs to millions of objects and the build is added after it, so a lower cap read
/// none of the built walls.
pub(crate) const MAX_ENTITIES: usize = 50_000_000;
/// Longest provenance value kept.
const MAX_VALUE: usize = 500;
/// Largest picture side.
const MAX_PX: u32 = 4096;

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new("bim.prints", "Print Validator Documents", run_prints).params(
            "{prints?: {sheets, symbols: [{sheet, level, kind, mark?, tag?, at: [x,y] feet, mountingIn?}], schedule: [{sheet, schedule, mark, kind?, qty?, mounting_in?, fields}], dimensions: [{sheet, level, a, b, value_ft, text, to?: center|face}], rooms: [{level, number, name, outline, unit_type?, exterior?, ceiling_ft?, sheet}], submittals: [{mark, section, fields}], terminations: [{tag, sheet, lugs: {per_phase, min, max, material}}], rules: {heights: [{kind?, mark?, mounting_in, source}], spec: [{section, kind?, category?, outdoors?, field, allowed?, forbidden?, text}], source_order?, tolerances?}, settings: {gate: off|warn|refuse}} (what the documents say; points in building feet on each level's plan), merge?: bool (add to the stored prints instead of replacing them), fromPlan?: bool (add the plan's device and fixture blocks as symbols of sheet `sheet`, default \"PLAN\"), sheet?, gate?: off|warn|refuse, clear?: true} → {symbols, schedule, dimensions, rooms, submittals, terminations, rules, gate}",
        ),
        CommandSpec::new("bim.validate", "Validate Model Against Prints", run_validate)
            .params(
                "{checks?: [overlay|counts|dimensions|tags|connectivity|physical|provenance|spec|termination], level?: name, handles?: [hex] (only round these: a fast re-check), area?: [x0,y0,x1,y1] drawing units, severity?: high|medium|low, limit?: n (500)} → {pass, score (0–100), total, failures: [{id, check, rule, element?, level, at?, sheetAt?, expected, found, source: {sheet?, at?, row?, section?}, severity, fix}], byCheck, bySeverity, checked, scoped, ms}",
            )
            .noundo(),
        CommandSpec::new("bim.validate.overlay", "Validation Failure Overlay", run_overlay)
            .params("{force?: bool (replace an existing file; otherwise it is refused), id: failure id, path?: PNG file (else base64), width?, height? (900×700), around?: feet (default 12)} → {id, path?|png, width, height}")
            .noundo(),
        CommandSpec::new("bim.source", "Element Provenance", run_source).params(
            "{handles: [hex], status: fixed|rule|zone|field_verified|override|agent, source?: text (sheet and place, note, rule), homerun?: panel tag (a run), tag?: text, kind?: text, boxDepth?: inches (a wall device's box)} → {changed}: where an element's place and type come from (XDATA CADADDY_PROVENANCE)",
        ),
    ]
}

// ---------------------------------------------------------------- the prints

/// The stored prints, if any.
pub(crate) fn prints_of(sp: &BuildingSpec) -> Option<&Prints> {
    sp.prints.as_deref()
}

fn run_prints(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "bim.prints";
    // MUST 1: an agent may not move the goalposts — gate, clear, and rules/tolerances are
    // human-only; the agent adds symbols, schedule rows and rooms (the evidence) but never
    // rewrites the rules it is checked against.
    if s.agent.from_agent {
        if p.get("gate").is_some_and(|v| !v.is_null()) {
            return Err(bad(CMD, "an agent may not change the validation gate (a person must set it)"));
        }
        if bool_or(p, "clear", false) {
            return Err(bad(CMD, "an agent may not clear the prints (a person must do it)"));
        }
        if let Some(pp) = p.get("prints").filter(|v| !v.is_null()) {
            if pp.get("rules").is_some_and(|v| !v.is_null()) {
                return Err(bad(CMD, "an agent may not replace validation rules or tolerances (a person must set them)"));
            }
            if pp.get("settings").is_some_and(|v| !v.is_null()) {
                return Err(bad(CMD, "an agent may not change validation settings (a person must set them)"));
            }
        }
    }
    let (mut sp, _) = current(s, CMD)?;
    let mut prints = if bool_or(p, "clear", false) { Prints::default() } else { sp.prints.as_deref().cloned().unwrap_or_default() };
    if let Some(given) = p.get("prints").filter(|v| !v.is_null()) {
        let new = Prints::from_json(&given.to_string()).map_err(|e| bad(CMD, e))?;
        prints = if bool_or(p, "merge", false) { merged(prints, new) } else { new };
    }
    if bool_or(p, "fromPlan", false) {
        let sheet = str_param(p, "sheet").unwrap_or("PLAN").chars().take(100).collect::<String>();
        let (m, _) = model_of(s.doc()?, &sp);
        prints.symbols.retain(|x| x.sheet != sheet);
        for e in m.elements.iter().filter(|e| e.wall.is_none() && !e.id.starts_with("run:")) {
            prints.symbols.push(v::Symbol {
                sheet: sheet.clone(),
                level: e.level.clone(),
                kind: e.kind.clone(),
                mark: e.mark.clone(),
                tag: e.tag.clone(),
                at: e.at,
                mounting_in: if e.mounting == Mounting::Wall { e.z_in } else { None },
            });
        }
    }
    if let Some(g) = str_param(p, "gate") {
        prints.settings.gate = match g.trim().to_ascii_lowercase().as_str() {
            "off" => GateMode::Off,
            "warn" => GateMode::Warn,
            "refuse" => GateMode::Refuse,
            _ => return Err(bad(CMD, "`gate` is off, warn or refuse")),
        };
    }
    let prints = prints.capped();
    let out = json!({
        "symbols": prints.symbols.len(), "schedule": prints.schedule.len(), "dimensions": prints.dimensions.len(),
        "rooms": prints.rooms.len(), "submittals": prints.submittals.len(), "terminations": prints.terminations.len(),
        "rules": prints.rules.heights.len() + prints.rules.spec.len(), "gate": serde_json::to_value(prints.settings.gate).unwrap_or(Value::Null),
    });
    sp.prints = (!prints.is_empty() || prints.settings.gate != GateMode::default()).then(|| Box::new(prints));
    s.doc_mut()?.building_spec = Some(sp.to_json());
    Ok(out)
}

fn merged(mut a: Prints, b: Prints) -> Prints {
    a.sheets.extend(b.sheets);
    a.symbols.extend(b.symbols);
    a.schedule.extend(b.schedule);
    a.dimensions.extend(b.dimensions);
    a.rooms.extend(b.rooms);
    a.submittals.extend(b.submittals);
    a.terminations.extend(b.terminations);
    a.rules.heights.extend(b.rules.heights);
    a.rules.spec.extend(b.rules.spec);
    if !b.rules.source_order.is_empty() {
        a.rules.source_order = b.rules.source_order;
    }
    if b.rules.tolerances != v::Tolerances::default() {
        a.rules.tolerances = b.rules.tolerances;
    }
    if b.settings.gate != GateMode::default() {
        a.settings.gate = b.settings.gate;
    }
    a
}

// ---------------------------------------------------------------- the model

/// Sheet to building feet, per level: one frame for every level. A point of a level's plan is
/// its offset from the level's base, so the levels stand over each other as the 3D model and
/// `model.render` stack them (a building point is where the stacked model has it, less the
/// lowest level's base). A built object `model.build` moved to stack its level (its `stacked`
/// tag) is taken back onto its level's plan first, so it is never moved twice.
pub(crate) struct Frame {
    upf: f64,
    /// Level name, its plan's origin on the sheet.
    levels: Vec<(String, Vec2)>,
    spec_levels: Vec<cadaddy_walk3d::Level>,
    /// Level name, how far `model.build` moved its built objects to stack the levels.
    moved: Vec<(String, Vec2)>,
}

/// How far `model.build` moved each level's built objects to stack the levels (levels it did
/// not move are not listed).
fn build_moves(d: &Drawing, levels: usize) -> Vec<(String, Vec2)> {
    let mut out: Vec<(String, Vec2)> = Vec::new();
    for e in d.model.iter().take(MAX_ENTITIES) {
        if out.len() >= levels {
            break;
        }
        if super::wallgen::made_as(e).is_none() {
            continue;
        }
        let off = super::bspec::stacked_by(e);
        if off.len() <= 1e-9 {
            continue;
        }
        if let Some(l) = super::bspec::made_level(e).filter(|l| !out.iter().any(|(n, _)| n.eq_ignore_ascii_case(l))) {
            out.push((l, off));
        }
    }
    out
}

impl Frame {
    /// The frame of the spec's levels without deriving the scene (the levels' base points
    /// only): what one element's place needs.
    pub(crate) fn of_spec(d: &Drawing, sp: &BuildingSpec) -> Frame {
        let upf = if sp.units_per_foot.is_finite() && sp.units_per_foot > 0.0 { sp.units_per_foot } else { 12.0 };
        Frame { upf, levels: cadaddy_walk3d::level_bases(d, &sp.to_rules()), spec_levels: sp.levels.clone(), moved: build_moves(d, sp.levels.len()) }
    }
    /// The spec's name of a level (any case), else the name as given.
    fn level_name(&self, level: &str) -> String {
        self.spec_levels.iter().find(|l| l.name.eq_ignore_ascii_case(level.trim())).map_or_else(|| level.trim().to_string(), |l| l.name.clone())
    }
    /// How far `model.build` moved `level`'s built objects to stack the levels (zero when it
    /// didn't).
    fn moved(&self, level: &str) -> Vec2 {
        self.moved.iter().find(|(n, _)| n.eq_ignore_ascii_case(level)).map_or(Vec2::new(0.0, 0.0), |m| m.1)
    }
    /// A built object's point in building feet on `level`'s plan.
    pub(crate) fn built_on(&self, level: &str, e: &Entity, p: Vec2) -> [f64; 2] {
        self.to_ft_on(level, p - super::bspec::stacked_by(e))
    }
    /// A built object's level (its build tag; else where the point lies on the sheets) and the
    /// point in building feet.
    pub(crate) fn built_ft(&self, e: &Entity, p: Vec2) -> (String, [f64; 2]) {
        match super::bspec::made_level(e).filter(|l| !l.trim().is_empty()) {
            Some(l) => {
                let l = self.level_name(&l);
                let at = self.built_on(&l, e, p);
                (l, at)
            }
            None => self.to_ft(p - super::bspec::stacked_by(e)),
        }
    }
    /// A built object's plan box on its level's plan (drawing units): where its level's sheet
    /// draws it.
    pub(crate) fn built_box(&self, e: &Entity, b: Bounds2) -> Bounds2 {
        let off = super::bspec::stacked_by(e);
        Bounds2::new(b.min - off, b.max - off)
    }
    /// A building point of `level` where the built model has it: the drawing coordinates of the
    /// level's built objects, which `model.render` draws a level, a box or a camera in.
    pub(crate) fn to_built(&self, level: &str, p: [f64; 2]) -> Vec2 {
        self.to_sheet(level, p) + self.moved(level)
    }
    /// Drawing units per foot.
    pub(crate) fn upf(&self) -> f64 {
        self.upf
    }
    /// The level a sheet point is on and the point in building feet.
    pub(crate) fn to_ft(&self, p: Vec2) -> (String, [f64; 2]) {
        let name =
            level_at(&self.spec_levels, p.x, p.y).map(|l| l.name.clone()).or_else(|| self.levels.first().map(|l| l.0.clone())).unwrap_or_default();
        let base = self.levels.iter().find(|l| l.0.eq_ignore_ascii_case(&name)).map_or(Vec2::new(0.0, 0.0), |l| l.1);
        (name, [(p.x - base.x) / self.upf, (p.y - base.y) / self.upf])
    }
    /// A sheet point in building feet on the plan of `level`.
    pub(crate) fn to_ft_on(&self, level: &str, p: Vec2) -> [f64; 2] {
        let base = self.levels.iter().find(|l| l.0.eq_ignore_ascii_case(level)).map_or(Vec2::new(0.0, 0.0), |l| l.1);
        [(p.x - base.x) / self.upf, (p.y - base.y) / self.upf]
    }
    /// A building point on a level, on the sheet.
    pub(crate) fn to_sheet(&self, level: &str, p: [f64; 2]) -> Vec2 {
        let base = self.levels.iter().find(|l| l.0.eq_ignore_ascii_case(level)).map_or(Vec2::new(0.0, 0.0), |l| l.1);
        Vec2::new(base.x + p[0] * self.upf, base.y + p[1] * self.upf)
    }
}

fn xdata_after<'a>(x: &'a [RawTag], app: &str) -> impl Iterator<Item = &'a str> + 'a {
    let start = x.iter().position(|t| t.code == 1001 && t.value.trim().eq_ignore_ascii_case(app));
    let app_end = start.map(|s| s + 1);
    x.iter().skip(app_end.unwrap_or(x.len())).take_while(|t| t.code != 1001).filter(|t| t.code == 1000).map(|t| t.value.as_str())
}

/// An entity's provenance strings, `key=value`.
fn provenance(e: &Entity) -> BTreeMap<String, String> {
    xdata_after(&e.common.xdata, PROV_APP)
        .filter_map(|s| s.split_once('='))
        .map(|(k, v)| (k.trim().to_ascii_lowercase(), v.trim().to_string()))
        .collect()
}

fn has_override(e: &Entity) -> bool {
    e.common.xdata.iter().any(|t| t.code == 1001 && t.value.trim().eq_ignore_ascii_case(cadaddy_doc::overrides::OVERRIDE_APP))
}

/// The drawing's BIM types: mark → fields (with `category`).
fn types(d: &Drawing) -> BTreeMap<String, BTreeMap<String, String>> {
    let mut m = BTreeMap::new();
    for t in &d.bim_types.types {
        let mut f: BTreeMap<String, String> = t.fields.iter().map(|(k, v)| (k.clone(), v.value.clone())).collect();
        if !t.category.is_empty() {
            f.insert("category".into(), t.category.clone());
        }
        m.insert(t.mark.clone(), f);
    }
    m
}

fn is_run_category(c: &str) -> bool {
    let c = c.to_ascii_lowercase();
    ["raceway", "conduit", "duct", "pipe", "cable"].iter().any(|k| c.contains(k))
}

/// The model in the drawing, and the frame its points were taken in.
pub(crate) fn model_of(d: &Drawing, sp: &BuildingSpec) -> (Model, Frame) {
    let rules = sp.to_rules();
    let scene = cadaddy_walk3d::derive(d, &rules);
    let upf = if scene.units_per_foot.is_finite() && scene.units_per_foot > 0.0 { scene.units_per_foot } else { sp.units_per_foot.max(1e-9) };
    let frame = Frame {
        upf,
        levels: scene.levels.iter().map(|l| (l.name.clone(), l.base)).collect(),
        spec_levels: sp.levels.clone(),
        moved: build_moves(d, sp.levels.len()),
    };
    let types = types(d);
    let mut m = Model {
        levels: scene.levels.iter().map(|l| ModelLevel { name: l.name.clone(), height_ft: l.height_ft }).collect(),
        types: types.clone(),
        ..Model::default()
    };
    let by_handle: HashMap<Handle, &Arc<Entity>> = d.model.iter().take(MAX_ENTITIES).map(|e| (e.handle, e)).collect();
    // Devices, as Walk places them.
    for dev in scene.devices.iter().take(v::MAX_ITEMS) {
        let Some(l) = scene.levels.get(dev.level) else { continue };
        let Some(e) = by_handle.get(&dev.handle) else { continue };
        let prov = provenance(e);
        let ty_attr = dev.attrs.iter().find(|(k, _)| k.eq_ignore_ascii_case(&rules.devices.type_attr)).map(|(_, v)| v.clone());
        let mark =
            cadaddy_doc::bimspec::entity_type_mark(&e.common.xdata).map(str::to_string).or_else(|| ty_attr.clone()).filter(|m| !m.trim().is_empty());
        let kind = prov.get("kind").cloned().or_else(|| ty_attr.clone()).unwrap_or_else(|| dev.block.clone()).to_ascii_lowercase();
        let tag = prov.get("tag").cloned().or_else(|| dev.attrs.iter().find(|(k, _)| k.eq_ignore_ascii_case("TAG")).map(|(_, v)| v.clone()));
        let mut fields =
            mark.as_ref().and_then(|mk| types.iter().find(|(k, _)| k.eq_ignore_ascii_case(mk))).map(|(_, f)| f.clone()).unwrap_or_default();
        for (k, val) in prov.iter().filter(|(k, _)| k.starts_with("field.")) {
            fields.insert(k.trim_start_matches("field.").to_string(), val.clone());
        }
        // Walk hangs a ceiling device's box from the ceiling: its face is the box's top.
        let half = rules
            .mount(ty_attr.as_deref().unwrap_or(""))
            .size_ft
            .get(2)
            .copied()
            .filter(|h| h.is_finite())
            .map_or(0.0, |h| h.clamp(0.01, 20.0) / 2.0);
        let z_ft = dev.pos.z - l.elevation_ft + if dev.ceiling { half } else { 0.0 };
        // A device the build made and moved to stack its level: back over its own level's plan.
        let (level, at) = match (&e.kind, super::wallgen::made_as(e)) {
            (EntityKind::Insert(i), Some(_)) => {
                let raw = i.insert.xy();
                let (lv, ft) = frame.built_ft(e, raw);
                let was = frame.to_ft_on(&l.name, raw);
                (lv, [dev.pos.x + ft[0] - was[0], dev.pos.y + ft[1] - was[1]])
            }
            _ => (l.name.clone(), [dev.pos.x, dev.pos.y]),
        };
        m.elements.push(Element {
            id: dev.handle.hex(),
            kind,
            mark,
            tag,
            level,
            at,
            z_in: Some(if dev.ceiling { z_ft * 12.0 } else { dev.aff_ft * 12.0 }),
            mounting: if dev.ceiling { Mounting::Ceiling } else { Mounting::Wall },
            box_depth_in: prov.get("boxdepth").and_then(|v| v::parse_inches(v)),
            wall: None,
            status: if has_override(e) { Some(Status::Override) } else { prov.get("status").and_then(|s| Status::parse(s)) },
            source: prov.get("source").cloned(),
            fields,
        });
    }
    let devices: BTreeSet<Handle> = scene.devices.iter().map(|d| d.handle).collect();
    // Walls, doors and windows the build made.
    let wall_types = stud_depths(sp);
    let mut built_walls = false;
    for e in d.model.iter().take(MAX_ENTITIES) {
        let Some(part) = super::wallgen::made_as(e) else { continue };
        let EntityKind::Mesh(mesh) = &e.kind else { continue };
        if part == "wall" {
            built_walls = true;
            if m.walls.len() < v::MAX_ITEMS {
                m.walls.extend(mesh_walls(e, mesh, &frame, None, &wall_types));
            }
            continue;
        }
        if !matches!(part.as_str(), "door" | "window") {
            continue;
        }
        for quad in plan_quads(mesh) {
            let c = quad.iter().fold(Vec2::new(0.0, 0.0), |a, p| a + *p) * (1.0 / quad.len().max(1) as f64);
            let (level, _) = frame.built_ft(e, c);
            let ft: Vec<[f64; 2]> = quad.iter().map(|p| frame.built_on(&level, e, *p)).collect();
            let (lo, hi) = ft
                .iter()
                .fold(([f64::MAX; 2], [f64::MIN; 2]), |(lo, hi), p| ([lo[0].min(p[0]), lo[1].min(p[1])], [hi[0].max(p[0]), hi[1].max(p[1])]));
            if lo[0] <= hi[0] && lo[1] <= hi[1] {
                m.openings.push(ModelOpening { id: e.handle.hex(), level, kind: part.clone(), min: lo, max: hi, swing: None });
            }
        }
    }
    // No build yet: the plan's wall faces, as walls with no thickness. Once walls are built
    // they are the walls, never the faces of the plan under them.
    if !built_walls {
        for (i, sg) in scene.walls.iter().enumerate().take(v::MAX_ITEMS) {
            let Some(l) = scene.levels.get(sg.level) else { continue };
            m.walls.push(ModelWall {
                id: format!("face-{i}"),
                level: l.name.clone(),
                a: [sg.a.x, sg.a.y],
                b: [sg.b.x, sg.b.y],
                thickness_in: 0.0,
                stud_in: None,
                wall_type: None,
            });
        }
    }
    // Runs: lines on run layers, and lines and polylines typed as raceways, ducts or pipes.
    for e in d.model.iter().take(MAX_ENTITIES) {
        if devices.contains(&e.handle) || super::wallgen::made_as(e).is_some() {
            continue;
        }
        let mark = cadaddy_doc::bimspec::entity_type_mark(&e.common.xdata).map(str::to_string);
        let fields = mark.as_ref().and_then(|mk| types.iter().find(|(k, _)| k.eq_ignore_ascii_case(mk))).map(|(_, f)| f.clone()).unwrap_or_default();
        let on_run_layer = sp.role(&e.common.layer).is_some_and(|r| r.role == Role::Run);
        let typed_run = fields.get("category").is_some_and(|c| is_run_category(c));
        let pts: Vec<Vec2> = match &e.kind {
            EntityKind::Line(l) if on_run_layer || typed_run => vec![l.a.xy(), l.b.xy()],
            EntityKind::LwPolyline(pl) if on_run_layer || typed_run => pl.vertices.iter().take(10_000).map(|v| v.p).collect(),
            _ => {
                // Any other typed element (equipment drawn as a block or outline).
                if let Some(mk) = mark.as_ref().filter(|_| !matches!(e.kind, EntityKind::Line(_) | EntityKind::LwPolyline(_)) || !typed_run) {
                    let b = cadaddy_doc::entity_bounds(d, e, 0);
                    if b.is_empty() {
                        continue;
                    }
                    let prov = provenance(e);
                    let (level, at) = frame.to_ft(b.center());
                    let kind = prov
                        .get("kind")
                        .cloned()
                        .or_else(|| fields.get("category").cloned())
                        .unwrap_or_else(|| "element".into())
                        .to_ascii_lowercase();
                    m.elements.push(Element {
                        id: e.handle.hex(),
                        mounting: v::mounting_of(&kind),
                        kind,
                        mark: Some(mk.clone()),
                        tag: prov.get("tag").cloned(),
                        level,
                        at,
                        z_in: None,
                        box_depth_in: None,
                        wall: None,
                        status: if has_override(e) { Some(Status::Override) } else { prov.get("status").and_then(|s| Status::parse(s)) },
                        source: prov.get("source").cloned(),
                        fields,
                    });
                }
                continue;
            }
        };
        if pts.len() < 2 {
            continue;
        }
        let prov = provenance(e);
        // A run's own values (`field.size=…`) win over its type's, as a device's do.
        let mut fields = fields;
        for (k, val) in prov.iter().filter(|(k, _)| k.starts_with("field.")) {
            fields.insert(k.trim_start_matches("field.").to_string(), val.clone());
        }
        let (level, _) = frame.to_ft(pts.first().copied().unwrap_or(Vec2::new(0.0, 0.0)));
        let conductors = match (fields.get("conductorSize"), prov.get("conductorsize")) {
            (_, Some(sz)) | (Some(sz), None) => Some(v::Conductors {
                per_phase: prov.get("perphase").or_else(|| fields.get("conductorsPerPhase")).and_then(|x| x.trim().parse().ok()).unwrap_or(1),
                size: sz.clone(),
                material: prov.get("conductormaterial").or_else(|| fields.get("conductorMaterial")).cloned().unwrap_or_default(),
            }),
            _ => None,
        };
        m.runs.push(Run {
            id: e.handle.hex(),
            kind: fields.get("category").cloned().unwrap_or_else(|| "run".into()).to_ascii_lowercase(),
            level,
            path: pts.iter().map(|p| frame.to_ft(*p).1).collect(),
            mark,
            homerun: prov.get("homerun").cloned(),
            conductors,
            status: if has_override(e) { Some(Status::Override) } else { prov.get("status").and_then(|s| Status::parse(s)) },
            source: prov.get("source").cloned(),
            fields,
        });
    }
    m.walls = distinct_walls(std::mem::take(&mut m.walls));
    (m.capped(), frame)
}

/// The framing depth of each wall type (lower-case name → inches).
fn stud_depths(sp: &BuildingSpec) -> HashMap<String, f64> {
    sp.wall_types
        .iter()
        .filter_map(|w| {
            let stud =
                w.assembly.as_ref()?.layers.iter().filter(|l| l.category.eq_ignore_ascii_case("framing")).map(|l| l.thickness_in).fold(0.0, f64::max);
            (stud > 0.0).then(|| (w.name.to_ascii_lowercase(), stud))
        })
        .collect()
}

/// One plan rectangle (feet) of a built wall as a model wall.
fn wall_piece(e: &Entity, level: String, ft: &[[f64; 2]], studs: &HashMap<String, f64>) -> Option<ModelWall> {
    let (a, b, thick) = centreline(ft)?;
    let ty = xdata_after(&e.common.xdata, super::wallgen::APP).find_map(|s| s.strip_prefix("type:")).map(str::to_string);
    Some(ModelWall {
        id: e.handle.hex(),
        level,
        a,
        b,
        thickness_in: thick * 12.0,
        stud_in: ty.as_ref().and_then(|t| studs.get(&t.to_ascii_lowercase()).copied()),
        wall_type: ty,
    })
}

/// The built walls of `level` whose plan box comes within `reach` feet of `rect` (building
/// feet): what one element's host is looked for among, without reading the whole model.
pub(crate) fn walls_near(d: &Drawing, sp: &BuildingSpec, frame: &Frame, level: &str, rect: [f64; 4], reach: f64) -> Vec<ModelWall> {
    let studs = stud_depths(sp);
    let lo = frame.to_sheet(level, [rect[0] - reach, rect[1] - reach]);
    let hi = frame.to_sheet(level, [rect[2] + reach, rect[3] + reach]);
    let near = Bounds2::new(Vec2::new(lo.x.min(hi.x), lo.y.min(hi.y)), Vec2::new(lo.x.max(hi.x), lo.y.max(hi.y)));
    let mut out = Vec::new();
    for e in d.model.iter().take(MAX_ENTITIES) {
        if super::wallgen::made_as(e).is_none_or(|p| p != "wall") {
            continue;
        }
        let EntityKind::Mesh(mesh) = &e.kind else { continue };
        if super::bspec::made_level(e).is_some_and(|l| !l.eq_ignore_ascii_case(level)) {
            continue;
        }
        let b = cadaddy_doc::entity_bounds(d, e, 0);
        if b.is_empty() || !frame.built_box(e, b).intersects(&near) {
            continue;
        }
        out.extend(mesh_walls(e, mesh, frame, Some(level), &studs));
        if out.len() >= v::MAX_ITEMS {
            break;
        }
    }
    distinct_walls(out)
}

/// A built wall mesh's walls (building feet on its level's plan, on `only` when given): one per
/// plan rectangle of its pieces ([`wall_rects`]), with the wall's type and thickness.
fn mesh_walls(e: &Entity, mesh: &cadaddy_doc::Mesh, frame: &Frame, only: Option<&str>, studs: &HashMap<String, f64>) -> Vec<ModelWall> {
    let mut out = Vec::new();
    for rect in wall_rects(mesh) {
        let c = rect.iter().fold(Vec2::new(0.0, 0.0), |a, p| a + *p) * (1.0 / rect.len().max(1) as f64);
        let (on, _) = frame.built_ft(e, c);
        if only.is_some_and(|l| !on.eq_ignore_ascii_case(l)) {
            continue;
        }
        let ft: Vec<[f64; 2]> = rect.iter().map(|p| frame.built_on(&on, e, *p)).collect();
        out.extend(wall_piece(e, on, &ft, studs));
    }
    out
}

/// A wall mesh's plan rectangles (sheet units). A wall built from a face pair is boxes: their
/// bottoms on the floor are its pieces (the boxes over an opening start higher and are left
/// out, so an opening is a gap). A wall Walk mode built has no bottoms: its faces stand on the
/// floor and the cap over each pair of faces is the wall; the caps are its pieces then.
pub(crate) fn wall_rects(mesh: &cadaddy_doc::Mesh) -> Vec<Vec<Vec2>> {
    let z0 = mesh.vertices.iter().fold(f64::INFINITY, |lo, v| lo.min(v.z));
    let z1 = mesh.vertices.iter().fold(f64::NEG_INFINITY, |hi, v| hi.max(v.z));
    let tol = ((z1 - z0).abs() * 1e-6).max(1e-9);
    // Its flat faces, and the height each lies at.
    let flat: Vec<(f64, Vec<Vec2>)> = mesh
        .faces
        .iter()
        .take(100_000)
        .filter_map(|f| {
            let vs: Vec<Vec3> = f.v.iter().take(64).filter_map(|i| mesh.vertices.get(*i as usize).copied()).collect();
            let z = vs.first()?.z;
            (vs.len() >= 3 && vs.iter().all(|v| (v.z - z).abs() <= tol)).then(|| (z, vs.iter().map(|v| v.xy()).collect()))
        })
        .collect();
    let on_floor = flat.iter().any(|(z, _)| (z - z0).abs() <= tol);
    let mut seen: BTreeSet<Vec<(i64, i64)>> = BTreeSet::new();
    let mut out: Vec<Vec<Vec2>> = Vec::new();
    for (z, f) in flat {
        if on_floor && (z - z0).abs() > tol {
            continue;
        }
        // A box's top over its bottom, or a cap drawn twice, is one piece.
        let mut key: Vec<(i64, i64)> = f.iter().map(|p| ((p.x * 1e4).round() as i64, (p.y * 1e4).round() as i64)).collect();
        key.sort_unstable();
        key.dedup();
        if seen.insert(key) {
            out.push(f);
        }
    }
    out
}

/// `walls` without the ones listed twice: a piece on the same centreline (either way round), as
/// thick, on the same level as one before it (within an inch) is dropped.
pub(crate) fn distinct_walls(walls: Vec<ModelWall>) -> Vec<ModelWall> {
    const SAME_FT: f64 = 1.0 / 12.0;
    let cell = |p: [f64; 2]| ((p[0] / SAME_FT).floor() as i64, (p[1] / SAME_FT).floor() as i64);
    let mid = |w: &ModelWall| [(w.a[0] + w.b[0]) / 2.0, (w.a[1] + w.b[1]) / 2.0];
    let near = |p: [f64; 2], q: [f64; 2]| (p[0] - q[0]).hypot(p[1] - q[1]) <= SAME_FT;
    let mut seen: HashMap<(String, i64, i64), Vec<usize>> = HashMap::new();
    let mut out: Vec<ModelWall> = Vec::with_capacity(walls.len());
    for w in walls {
        let level = w.level.to_ascii_lowercase();
        let (cx, cy) = cell(mid(&w));
        let twice = (-1..=1).any(|dx| {
            (-1..=1).any(|dy| {
                seen.get(&(level.clone(), cx + dx, cy + dy)).is_some_and(|ix| {
                    ix.iter().filter_map(|i| out.get(*i)).any(|o| {
                        (o.thickness_in - w.thickness_in).abs() <= 1.0 && ((near(o.a, w.a) && near(o.b, w.b)) || (near(o.a, w.b) && near(o.b, w.a)))
                    })
                })
            })
        });
        if twice {
            continue;
        }
        seen.entry((level, cx, cy)).or_default().push(out.len());
        out.push(w);
    }
    out
}

/// A mesh's bottom faces as plan polygons (sheet units).
pub(crate) fn plan_quads(mesh: &cadaddy_doc::Mesh) -> Vec<Vec<Vec2>> {
    let z0 = mesh.vertices.iter().fold(f64::INFINITY, |lo, v| lo.min(v.z));
    let z1 = mesh.vertices.iter().fold(f64::NEG_INFINITY, |hi, v| hi.max(v.z));
    let tol = ((z1 - z0).abs() * 1e-6).max(1e-9);
    mesh.faces
        .iter()
        .take(100_000)
        .filter_map(|f| {
            let vs: Vec<Vec3> = f.v.iter().filter_map(|i| mesh.vertices.get(*i as usize).copied()).collect();
            (vs.len() >= 3 && vs.iter().all(|v| (v.z - z0).abs() <= tol)).then(|| vs.iter().map(|v| v.xy()).collect())
        })
        .collect()
}

/// A wall piece's centreline and thickness from its plan rectangle (feet). Any other outline
/// (a cap drawn as triangles, a piece with a corner split) is read as the smallest rectangle
/// round it that lies along one of its sides.
pub(crate) fn centreline(q: &[[f64; 2]]) -> Option<([f64; 2], [f64; 2], f64)> {
    if q.len() != 4 {
        return fitted_centreline(q);
    }
    let p = |i: usize| q.get(i % 4).copied().unwrap_or([0.0, 0.0]);
    let len = |a: [f64; 2], b: [f64; 2]| (a[0] - b[0]).hypot(a[1] - b[1]);
    let mid = |a: [f64; 2], b: [f64; 2]| [(a[0] + b[0]) / 2.0, (a[1] + b[1]) / 2.0];
    // The short sides are the ends.
    let (e0, e1) = if len(p(0), p(1)) <= len(p(1), p(2)) { ((0, 1), (2, 3)) } else { ((1, 2), (3, 0)) };
    let thick = len(p(e0.0), p(e0.1));
    let (a, b) = (mid(p(e0.0), p(e0.1)), mid(p(e1.0), p(e1.1)));
    (thick.is_finite() && len(a, b) > 1e-9).then_some((a, b, thick))
}

/// [`centreline`] of an outline that isn't four corners: the least rectangle round it along
/// one of its sides, its long sides the wall's faces.
fn fitted_centreline(q: &[[f64; 2]]) -> Option<([f64; 2], [f64; 2], f64)> {
    if q.len() < 3 || q.len() > 64 {
        return None;
    }
    let mut best: Option<(f64, [f64; 2], [f64; 2], f64, f64)> = None;
    for (i, a) in q.iter().enumerate() {
        let b = q.get((i + 1) % q.len()).copied().unwrap_or(*a);
        let l = (b[0] - a[0]).hypot(b[1] - a[1]);
        if l.is_nan() || l <= 1e-9 {
            continue;
        }
        let u = [(b[0] - a[0]) / l, (b[1] - a[1]) / l];
        let n = [-u[1], u[0]];
        let (mut s0, mut s1, mut t0, mut t1) = (f64::INFINITY, f64::NEG_INFINITY, f64::INFINITY, f64::NEG_INFINITY);
        for p in q {
            let (s, t) = (p[0] * u[0] + p[1] * u[1], p[0] * n[0] + p[1] * n[1]);
            (s0, s1, t0, t1) = (s0.min(s), s1.max(s), t0.min(t), t1.max(t));
        }
        let area = (s1 - s0) * (t1 - t0);
        // Along its long side.
        let (u, n, s0, s1, t0, t1) = if s1 - s0 >= t1 - t0 { (u, n, s0, s1, t0, t1) } else { (n, [-u[0], -u[1]], t0, t1, -s1, -s0) };
        if area.is_finite() && best.as_ref().is_none_or(|b| area < b.0 - 1e-12) {
            let t = (t0 + t1) / 2.0;
            let at = |s: f64| [u[0] * s + n[0] * t, u[1] * s + n[1] * t];
            best = Some((area, at(s0), at(s1), t1 - t0, s1 - s0));
        }
    }
    let (_, a, b, thick, len) = best?;
    (thick.is_finite() && len > 1e-9).then_some((a, b, thick))
}

// ---------------------------------------------------------------- validate

fn checks_param(cmd: &str, p: &Value) -> Result<Vec<Check>> {
    match p.get("checks") {
        None | Some(Value::Null) => Ok(Vec::new()),
        Some(Value::Array(a)) => a
            .iter()
            .take(32)
            .map(|c| {
                c.as_str().and_then(Check::parse).ok_or_else(|| {
                    bad(cmd, format!("unknown check {c} (overlay, counts, dimensions, tags, connectivity, physical, provenance, spec, termination)"))
                })
            })
            .collect(),
        Some(Value::String(c)) => Ok(vec![Check::parse(c).ok_or_else(|| bad(cmd, format!("unknown check {c}")))?]),
        Some(_) => Err(bad(cmd, "`checks` is a list of check names")),
    }
}

/// The scope a call asks for.
fn scope_param(cmd: &str, p: &Value, frame: &Frame) -> Result<Scope> {
    let mut sc = Scope { checks: checks_param(cmd, p)?, ..Scope::default() };
    if let Some(l) = str_param(p, "level") {
        sc.levels.push(l.to_string());
    }
    if p.get("handles").is_some_and(|h| !h.is_null()) {
        let hs = p.get("handles").and_then(Value::as_array).ok_or_else(|| bad(cmd, "`handles` is a list of handles"))?;
        for h in hs.iter().take(100_000) {
            let id = h.as_str().and_then(Handle::parse_hex).map(|h| h.hex()).ok_or_else(|| bad(cmd, format!("{h} is not a handle")))?;
            sc.elements.push(id);
        }
    }
    if let Some(a) = p.get("area").filter(|v| !v.is_null()) {
        let n: Vec<f64> = a.as_array().map(|v| v.iter().filter_map(Value::as_f64).collect()).unwrap_or_default();
        let [x0, y0, x1, y1] = n[..] else { return Err(bad(cmd, "`area` is [x0, y0, x1, y1] in drawing units")) };
        if ![x0, y0, x1, y1].iter().all(|v| v.is_finite()) {
            return Err(bad(cmd, "`area` must be finite"));
        }
        let (level, a) = frame.to_ft(Vec2::new(x0.min(x1), y0.min(y1)));
        let (_, b) = frame.to_ft(Vec2::new(x0.max(x1), y0.max(y1)));
        sc.areas.push((level, [a, b]));
    }
    Ok(sc)
}

fn failure_json(f: &v::Failure, frame: &Frame) -> Value {
    let mut j = serde_json::to_value(f).unwrap_or(Value::Null);
    if let Some(at) = f.at {
        let p = frame.to_sheet(&f.level, at);
        j["sheetAt"] = json!([p.x, p.y]);
    }
    j
}

/// Validate the active drawing.
pub(crate) fn report(s: &Session, cmd: &str, p: &Value) -> Result<(Report, Frame)> {
    let (sp, _) = current(s, cmd)?;
    let (model, frame) = model_of(s.doc()?, &sp);
    let prints: Prints = if let Some(p) = prints_of(&sp) {
        p.clone()
    } else if let Some(p) = prints_from_job(s, &frame) {
        p
    } else {
        return Err(bad(
            cmd,
            "the drawing has no prints to check against yet: \
             give them with bim.prints, or start a guided job with job.start \
             {pdfs: \"<folder>\", goal: \"bim\"} which imports them from the sheets",
        ));
    };
    let scope = scope_param(cmd, p, &frame)?;
    Ok((v::validate(&prints, &model, &scope), frame))
}

/// Try to build [`Prints`] from a guided job's data when `sp.prints` is empty.
///
/// The guided job's Devices stage reads device symbols from the plan sheets and stores them in
/// `job.devices`. Its Rooms stage stores rooms in the stage result. Neither writes `sp.prints`,
/// so `bim.validate` needs to find them here.
pub(crate) fn prints_from_job(s: &Session, frame: &Frame) -> Option<Prints> {
    let job = super::guided::load_job(s).ok()??;
    let mut prints = Prints::default();
    // Sheets from the job's classified sheet regions.
    for r in &job.sheet_regions {
        if !prints.sheets.iter().any(|s| s.number.eq_ignore_ascii_case(&r.number)) {
            prints.sheets.push(PrintSheet {
                number: r.number.clone(),
                title: String::new(),
                level: if r.level.is_empty() { None } else { Some(r.level.clone()) },
                kinds: Vec::new(),
            });
        }
    }
    // Symbols from the job's confirmed devices.
    for dev in &job.devices {
        let level = if !dev.level.is_empty() {
            dev.level.clone()
        } else {
            job.sheet_regions.iter().find(|r| r.number.eq_ignore_ascii_case(&dev.sheet)).map(|r| r.level.clone()).unwrap_or_default()
        };
        let (_, at_ft) = frame.to_ft(Vec2::new(dev.at[0], dev.at[1]));
        prints.symbols.push(Symbol {
            sheet: if dev.sheet.is_empty() { "PLAN".into() } else { dev.sheet.clone() },
            level,
            kind: dev.tag.to_ascii_lowercase(),
            mark: None,
            tag: if dev.tag.is_empty() { None } else { Some(dev.tag.clone()) },
            at: at_ft,
            mounting_in: None,
        });
    }
    // Rooms from the Rooms stage result, when the stage ran.
    if let Some(st) = job.stages.iter().find(|s| s.name == super::guided::StageName::Rooms)
        && let Some(rooms_arr) = st.result.as_ref().and_then(|r| r.get("rooms")).and_then(Value::as_array)
    {
        for room in rooms_arr.iter().take(v::MAX_ITEMS) {
            let number = room.get("number").or_else(|| room.get("name")).and_then(Value::as_str).unwrap_or("").to_string();
            let name = room.get("name").and_then(Value::as_str).unwrap_or("").to_string();
            let level = room.get("level").and_then(Value::as_str).unwrap_or("").to_string();
            let sheet = room.get("sheet").and_then(Value::as_str).unwrap_or("").to_string();
            let unit_type = room.get("unit_type").and_then(Value::as_str).map(String::from);
            let exterior = room.get("exterior").and_then(Value::as_bool).unwrap_or(false);
            let ceiling_ft = room.get("ceiling_ft").and_then(Value::as_f64).filter(|v| v.is_finite());
            let outline: Vec<[f64; 2]> = room
                .get("outline")
                .and_then(Value::as_array)
                .map(|a| {
                    a.iter()
                        .filter_map(|p| {
                            let arr = p.as_array()?;
                            let x = arr.first()?.as_f64()?;
                            let y = arr.get(1)?.as_f64()?;
                            Some(frame.to_ft(Vec2::new(x, y)).1)
                        })
                        .collect()
                })
                .unwrap_or_default();
            if !number.is_empty() || !name.is_empty() {
                prints.rooms.push(v::PrintRoom { level, number, name, outline, unit_type, exterior, ceiling_ft, sheet });
            }
        }
    }
    if prints.sheets.is_empty() && prints.symbols.is_empty() && prints.rooms.is_empty() {
        return None;
    }
    Some(prints.capped())
}

fn run_validate(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "bim.validate";
    let start = web_time::Instant::now();
    let severity = match str_param(p, "severity") {
        Some(x) => Some(Severity::parse(x).ok_or_else(|| bad(CMD, "`severity` is high, medium or low"))?),
        None => None,
    };
    let limit = p.get("limit").and_then(Value::as_u64).map_or(DEFAULT_LIMIT, |n| n.min(v::MAX_FAILURES as u64) as usize);
    let (r, frame) = report(s, CMD, p)?;
    let [high, medium, low] = r.by_severity();
    let listed: Vec<Value> =
        r.failures.iter().filter(|f| severity.is_none_or(|x| f.severity == x)).take(limit).map(|f| failure_json(f, &frame)).collect();
    Ok(json!({
        "pass": r.pass,
        "score": r.score,
        "total": r.failures.len(),
        "failures": listed,
        "byCheck": r.by_check(),
        "bySeverity": {"high": high, "medium": medium, "low": low},
        "checked": r.checked,
        "scoped": r.scoped,
        "ms": start.elapsed().as_millis() as u64,
    }))
}

// ---------------------------------------------------------------- overlay

fn run_overlay(s: &mut Session, p: &Value) -> Result<Value> {
    super::exchange::guard_params(s, CMD, p, &["path", "output"])?;
    use cadaddy_doc::color::Color;
    const CMD: &str = "bim.validate.overlay";
    let id = str_param(p, "id").ok_or_else(|| bad(CMD, "`id` (a failure id from bim.validate) is required"))?;
    let w = p.get("width").and_then(Value::as_u64).map_or(900, |v| v.clamp(16, u64::from(MAX_PX)) as u32);
    let h = p.get("height").and_then(Value::as_u64).map_or(700, |v| v.clamp(16, u64::from(MAX_PX)) as u32);
    let around = p.get("around").and_then(Value::as_f64).filter(|v| v.is_finite() && *v > 0.0).unwrap_or(12.0).min(1_000.0);
    let (sp, _) = current(s, CMD)?;
    let (model, frame) = model_of(s.doc()?, &sp);
    let prints: Prints = if let Some(p) = prints_of(&sp) {
        p.clone()
    } else if let Some(p) = prints_from_job(s, &frame) {
        p
    } else {
        return Err(bad(CMD, "the drawing has no prints to check against yet"));
    };
    let r = v::validate(&prints, &model, &Scope::all());
    let f = r.failures.iter().find(|f| f.id.eq_ignore_ascii_case(id)).ok_or_else(|| bad(CMD, format!("there is no failure {id} now")))?;
    let at = f.at.or(f.source.at).ok_or_else(|| bad(CMD, format!("failure {id} has no place to picture")))?;
    let level = f.level.clone();
    let (lo, hi) = ([at[0] - around, at[1] - around], [at[0] + around, at[1] + around]);
    let near = |q: [f64; 2]| q[0] >= lo[0] && q[0] <= hi[0] && q[1] >= lo[1] && q[1] <= hi[1];
    let mut od = Drawing::new_imperial();
    let put = |od: &mut Drawing, kind: EntityKind, color: u8| {
        let _ = od.add(&Space::Model, Common { color: Color::Index(color), ..Common::default() }, kind);
    };
    let sheet = |q: [f64; 2]| {
        let p = frame.to_sheet(&level, q);
        Vec3::new(p.x, p.y, 0.0)
    };
    let upf = frame.upf;
    let line = |a: Vec3, b: Vec3| EntityKind::Line(cadaddy_doc::Line { a, b });
    // The plan round it, in grey.
    let d = s.doc()?;
    let win = Bounds2::new(frame.to_sheet(&level, lo), frame.to_sheet(&level, hi));
    for e in d.model.iter().take(MAX_ENTITIES) {
        if super::wallgen::made_as(e).is_some() {
            continue;
        }
        let b = cadaddy_doc::entity_bounds(d, e, 0);
        if b.is_empty() || !win.intersects(&b) {
            continue;
        }
        let mut c = (**e).clone();
        c.common.color = Color::Index(8);
        c.common.layer = "0".into();
        let _ = od.add(&Space::Model, c.common, c.kind);
    }
    od.blocks = d.blocks.clone();
    // Walls (blue outlines), elements (blue squares), the print's symbols (grey crosses).
    for wl in model.walls.iter().filter(|x| x.level.eq_ignore_ascii_case(&level) && (near(x.a) || near(x.b))) {
        put(&mut od, line(sheet(wl.a), sheet(wl.b)), 5);
    }
    let r6 = upf * 0.25;
    for e in model.elements.iter().filter(|e| e.level.eq_ignore_ascii_case(&level) && near(e.at)) {
        let c = sheet(e.at);
        let vertices = [(-r6, -r6), (r6, -r6), (r6, r6), (-r6, r6)]
            .iter()
            .map(|(dx, dy)| cadaddy_geom::PolyVertex { p: Vec2::new(c.x + dx, c.y + dy), bulge: 0.0, start_width: 0.0, end_width: 0.0 })
            .collect();
        put(
            &mut od,
            EntityKind::LwPolyline(cadaddy_doc::LwPolyline { vertices, closed: true, const_width: 0.0, elevation: 0.0, plinegen: false }),
            5,
        );
    }
    for r in model.runs.iter().filter(|r| r.level.eq_ignore_ascii_case(&level) && r.path.iter().any(|q| near(*q))) {
        for pair in r.path.windows(2) {
            if let [a, b] = pair {
                put(&mut od, line(sheet(*a), sheet(*b)), 4);
            }
        }
    }
    for sy in prints.symbols.iter().filter(|x| x.level.eq_ignore_ascii_case(&level) && near(x.at)) {
        let c = sheet(sy.at);
        put(&mut od, line(Vec3::new(c.x - r6, c.y - r6, 0.0), Vec3::new(c.x + r6, c.y + r6, 0.0)), 8);
        put(&mut od, line(Vec3::new(c.x - r6, c.y + r6, 0.0), Vec3::new(c.x + r6, c.y - r6, 0.0)), 8);
    }
    // The failure, in red: a ring, and a line to where the print has it.
    let c = sheet(at);
    put(&mut od, EntityKind::Circle(cadaddy_doc::Circle { center: c, radius: upf * 0.75 }), 1);
    if let Some(src) = f.source.at.filter(|q| *q != at) {
        put(&mut od, line(c, sheet(src)), 1);
    }
    let ext = Bounds2::new(frame.to_sheet(&level, lo), frame.to_sheet(&level, hi));
    let all: Vec<&Entity> = od.model.iter().map(|e| e.as_ref()).collect();
    let list = cadaddy_render::build_entities(&od, all, &cadaddy_render::Options::default());
    let view = cadaddy_render::raster::View::fit(&ext, w, h, 0.03);
    let ro = cadaddy_render::raster::RasterOptions { background: cadaddy_color::Rgb(255, 255, 255), hairline: 1.5, px_per_mm: 0.0, antialias: true };
    let png = cadaddy_render::raster::render_png(&list, &view, &ro).ok_or_else(|| bad(CMD, "the picture couldn't be drawn"))?;
    let mut out = json!({ "id": f.id, "width": w, "height": h, "rule": f.rule });
    super::verify::deliver(png, p, &mut out)?;
    Ok(out)
}

// ---------------------------------------------------------------- provenance

fn run_source(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "bim.source";
    let from_agent = s.agent.from_agent;
    let hs = targets(s, p)?;
    if hs.is_empty() {
        return Err(bad(CMD, "`handles` is required"));
    }
    let status = str_param(p, "status").ok_or_else(|| bad(CMD, "`status` is fixed, rule, zone, field_verified, override or agent"))?;
    let status = Status::parse(status).ok_or_else(|| bad(CMD, "`status` is fixed, rule, zone, field_verified, override or agent"))?;
    // MUST 1: an agent may not stamp elements as field_verified or override — those statuses
    // exempt an element from misplaced, not_on_print and type checks.
    if from_agent && matches!(status, Status::FieldVerified | Status::Override) {
        return Err(bad(CMD, format!("an agent may not set status {} (only a person can certify field verification or overrides)", status.name())));
    }
    let clip = |s: &str| s.chars().filter(|c| !c.is_control()).take(MAX_VALUE).collect::<String>();
    let mut tags = vec![RawTag { code: 1001, value: PROV_APP.into() }, RawTag { code: 1000, value: format!("status={}", status.name()) }];
    // Record the author so the audit trail shows who set each provenance.
    tags.push(RawTag { code: 1000, value: format!("author={}", if from_agent { "agent" } else { "user" }) });
    for key in ["source", "homerun", "tag", "kind"] {
        if let Some(v) = str_param(p, key).filter(|v| !v.trim().is_empty()) {
            tags.push(RawTag { code: 1000, value: format!("{key}={}", clip(v)) });
        }
    }
    if let Some(d) = p.get("boxDepth").filter(|v| !v.is_null()) {
        let d = d.as_f64().filter(|d| d.is_finite() && *d > 0.0 && *d < 100.0).ok_or_else(|| bad(CMD, "`boxDepth` is a depth in inches"))?;
        tags.push(RawTag { code: 1000, value: format!("boxdepth={d}") });
    }
    let d = s.doc_mut()?;
    let mut changed = 0;
    for h in hs.iter().take(100_000) {
        let t = tags.clone();
        if d.modify_entity(*h, |e| {
            e.common.xdata = strip_app(&e.common.xdata, PROV_APP).into();
            e.common.xdata.extend(t);
        })
        .is_ok()
        {
            changed += 1;
        }
    }
    if changed == 0 {
        return Err(bad(CMD, "none of those handles is in the drawing"));
    }
    Ok(json!({ "changed": changed }))
}

/// XDATA without one application's group.
fn strip_app(x: &[RawTag], app: &str) -> Vec<RawTag> {
    let mut out = Vec::with_capacity(x.len());
    let mut skipping = false;
    for t in x {
        if t.code == 1001 {
            skipping = t.value.trim().eq_ignore_ascii_case(app);
        }
        if !skipping {
            out.push(t.clone());
        }
    }
    out
}

// ---------------------------------------------------------------- the gate

/// An agent's edit under way: the drawing before it, for comparing and putting back.
pub(crate) struct Gate {
    mode: GateMode,
    docs: Vec<crate::DocState>,
    active: usize,
    before: Arc<Drawing>,
}

/// Whether an agent's call of `id` is gated, and the state to compare with after it.
/// Commands that are read-only or handled by their own agent guards (not the gate).
fn gate_exempt(cmd: &str) -> bool {
    matches!(cmd, "bim.validate" | "bim.validate.overlay" | "bim.build" | "bim.prints") || cmd.starts_with("agent.")
}

pub(crate) fn gate_before(s: &Session, id: &str) -> Option<Gate> {
    let cmd = resolve_alias(id);
    if gate_exempt(&cmd) || !find_command(&cmd).is_some_and(|c| c.undoable) {
        return None;
    }
    let st = s.state().ok()?;
    let text = st.doc.building_spec.as_deref()?;
    // Cheap test first: no prints, no gate.
    if !text.contains("\"prints\"") {
        return None;
    }
    let sp = BuildingSpec::from_json(text).ok()?;
    let mode = sp.prints.as_ref().map(|p| p.settings.gate)?;
    (mode != GateMode::Off).then(|| Gate { mode, docs: s.docs.clone(), active: s.active, before: Arc::clone(&st.doc) })
}

/// After an agent's gated edit: the failures it added (round what it changed) come back with
/// the result; with refuse, an edit that added any is undone and refused.
pub(crate) fn gate_after(s: &mut Session, g: Gate, r: Result<Value>) -> Result<Value> {
    let Ok(mut result) = r else { return r };
    let Ok(after) = s.doc().map(|d| d as *const Drawing) else { return Ok(result) };
    // The same drawing still?
    if s.active != g.active {
        return Ok(result);
    }
    let Ok(now) = s.doc() else { return Ok(result) };
    if std::ptr::eq(after, Arc::as_ptr(&g.before)) {
        return Ok(result);
    }
    let changed = changed_handles(&g.before, now);
    if changed.is_empty() && g.before.building_spec == now.building_spec && g.before.bim_types == now.bim_types {
        return Ok(result);
    }
    let parts = |d: &Drawing| -> Option<(Prints, Model)> {
        let sp = BuildingSpec::from_json(d.building_spec.as_deref()?).ok()?;
        let prints = sp.prints.as_deref()?.clone();
        Some((prints, model_of(d, &sp).0))
    };
    let (Some((p0, m0)), Some((p1, m1))) = (parts(&g.before), parts(now)) else { return Ok(result) };
    let scope = gate_scope(&changed, &m0, &m1);
    let before = v::validate(&p0, &m0, &scope);
    let after_r = v::validate(&p1, &m1, &scope);
    let (new, fixed) = v::new_failures(&before.failures, &after_r.failures);
    let new: Vec<v::Failure> = new.into_iter().cloned().collect();
    let summary = json!({
        "new": new.iter().take(100).map(|f| serde_json::to_value(f).unwrap_or(Value::Null)).collect::<Vec<_>>(),
        "newCount": new.len(),
        "fixed": fixed,
        "score": after_r.score,
        "gate": serde_json::to_value(g.mode).unwrap_or(Value::Null),
    });
    if g.mode == GateMode::Refuse && !new.is_empty() {
        s.docs = g.docs;
        s.active = g.active;
        let list: Vec<String> = new
            .iter()
            .take(5)
            .map(|f| format!("{} {} ({}: expected {}, found {})", f.id, f.rule, f.element.as_deref().unwrap_or("-"), f.expected, f.found))
            .collect();
        return Err(crate::EngineError::Other(format!(
            "refused: the edit adds {} validation failure{} (prints gate is refuse): {}",
            new.len(),
            if new.len() == 1 { "" } else { "s" },
            list.join("; ")
        )));
    }
    match &mut result {
        Value::Object(o) => {
            o.insert("validation".into(), summary);
        }
        other => {
            let inner = std::mem::take(other);
            *other = json!({ "result": inner, "validation": summary });
        }
    }
    Ok(result)
}

/// Objects added, removed or changed between two versions of a drawing (model space).
fn changed_handles(before: &Drawing, after: &Drawing) -> Vec<(Handle, Option<Arc<Entity>>)> {
    let b: HashMap<Handle, &Arc<Entity>> = before.model.iter().map(|e| (e.handle, e)).collect();
    let a: HashMap<Handle, &Arc<Entity>> = after.model.iter().map(|e| (e.handle, e)).collect();
    let mut out: Vec<(Handle, Option<Arc<Entity>>)> = Vec::new();
    for (h, e) in &a {
        match b.get(h) {
            Some(old) if Arc::ptr_eq(old, e) || old == e => {}
            Some(old) => out.push((*h, Some(Arc::clone(old)))),
            None => out.push((*h, None)),
        }
    }
    for (h, e) in &b {
        if !a.contains_key(h) {
            out.push((*h, Some(Arc::clone(e))));
        }
    }
    out.sort_by_key(|(h, _)| h.0);
    out.truncate(100_000);
    out
}

/// The scope round an edit: the objects it touched, where they are now and where they were
/// (a deleted device leaves its symbol unmatched there; a moved wall reaches the devices on
/// it).
fn gate_scope(changed: &[(Handle, Option<Arc<Entity>>)], before: &Model, after: &Model) -> Scope {
    let ids: BTreeSet<String> = changed.iter().map(|(h, _)| h.hex()).collect();
    let mut sc = Scope { elements: ids.iter().cloned().collect(), ..Scope::default() };
    for m in [before, after] {
        for e in m.elements.iter().filter(|e| ids.contains(&e.id)) {
            sc.areas.push((e.level.clone(), [e.at, e.at]));
        }
        for w in m.walls.iter().filter(|w| ids.contains(&w.id)) {
            sc.areas.push((w.level.clone(), [w.a, w.b]));
        }
        for o in m.openings.iter().filter(|o| ids.contains(&o.id)) {
            sc.areas.push((o.level.clone(), [o.min, o.max]));
        }
        for r in m.runs.iter().filter(|r| ids.contains(&r.id)) {
            for q in &r.path {
                sc.areas.push((r.level.clone(), [*q, *q]));
            }
        }
    }
    sc.areas.truncate(200_000);
    sc
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn centreline_of_a_wall_rectangle() {
        let q = [[0.0, 0.0], [10.0, 0.0], [10.0, 0.5], [0.0, 0.5]];
        let (a, b, t) = centreline(&q).unwrap_or(([9.0; 2], [9.0; 2], 9.0));
        assert!((t - 0.5).abs() < 1e-9);
        assert!((a[1] - 0.25).abs() < 1e-9 && (b[1] - 0.25).abs() < 1e-9);
        assert!(((a[0] - b[0]).abs() - 10.0).abs() < 1e-9);
        // Half of it (a cap drawn as two triangles): the same wall.
        let (a, b, t) = centreline(&q[..3]).unwrap_or(([9.0; 2], [9.0; 2], 9.0));
        assert!((t - 0.5).abs() < 1e-9 && (a[1] - 0.25).abs() < 1e-9 && (b[1] - 0.25).abs() < 1e-9, "{a:?} {b:?} {t}");
        assert!(((a[0] - b[0]).abs() - 10.0).abs() < 1e-9);
        assert!(centreline(&q[..2]).is_none());
    }

    #[test]
    fn a_fitted_centreline_runs_along_the_long_side() {
        // A wall running north drawn as five corners (one side split).
        let q = [[0.0, 0.0], [0.5, 0.0], [0.5, 4.0], [0.5, 12.0], [0.0, 12.0]];
        let (a, b, t) = centreline(&q).unwrap_or(([9.0; 2], [9.0; 2], 9.0));
        assert!((t - 0.5).abs() < 1e-9, "{t}");
        assert!((a[0] - 0.25).abs() < 1e-9 && (b[0] - 0.25).abs() < 1e-9, "{a:?} {b:?}");
        assert!(((a[1] - b[1]).abs() - 12.0).abs() < 1e-9, "{a:?} {b:?}");
    }

    fn quad_mesh(quads: &[[[f64; 3]; 4]]) -> cadaddy_doc::Mesh {
        let mut m = super::super::wallgen::empty_mesh();
        for q in quads {
            let base = m.vertices.len() as u32;
            m.vertices.extend(q.iter().map(|p| Vec3::new(p[0], p[1], p[2])));
            m.faces.push(cadaddy_doc::MeshFace { v: vec![base, base + 1, base + 2, base + 3], hidden: 0 });
        }
        m
    }

    #[test]
    fn a_built_box_walls_pieces_are_its_bottoms_an_opening_a_gap() {
        // A 10' wall, 6" thick, 9' high, a 3' door from 4' to 7' (the header from 7' up).
        let mut m = super::super::wallgen::empty_mesh();
        for (s0, s1, z0, z1) in [(0.0, 4.0, 0.0, 9.0), (4.0, 7.0, 7.0, 9.0), (7.0, 10.0, 0.0, 9.0)] {
            super::super::wallgen::add_box(&mut m, Vec2::new(s0, 0.0), Vec2::new(s1, 0.0), 0.5, z0, z1);
        }
        let rs = wall_rects(&m);
        assert_eq!(rs.len(), 2, "the two pieces on the floor, not their tops nor the header: {rs:?}");
        for r in &rs {
            let ft: Vec<[f64; 2]> = r.iter().map(|p| [p.x, p.y]).collect();
            let (_, _, t) = centreline(&ft).unwrap_or(([0.0; 2], [0.0; 2], 0.0));
            assert!((t - 0.5).abs() < 1e-9);
        }
    }

    #[test]
    fn a_walk_mode_walls_pieces_are_its_caps() {
        // Walk mode: the two faces of a 10' wall standing on the floor, the cap between them on
        // top (drawn twice), no bottom.
        let m = quad_mesh(&[
            [[0.0, 0.0, 0.0], [10.0, 0.0, 0.0], [10.0, 0.0, 9.0], [0.0, 0.0, 9.0]],
            [[0.0, 0.5, 0.0], [10.0, 0.5, 0.0], [10.0, 0.5, 9.0], [0.0, 0.5, 9.0]],
            [[0.0, 0.0, 9.0], [10.0, 0.0, 9.0], [10.0, 0.5, 9.0], [0.0, 0.5, 9.0]],
            [[10.0, 0.5, 9.0], [0.0, 0.5, 9.0], [0.0, 0.0, 9.0], [10.0, 0.0, 9.0]],
        ]);
        let rs = wall_rects(&m);
        assert_eq!(rs.len(), 1, "one cap, one piece: {rs:?}");
        // The old reading (flat faces at the mesh's lowest point only) found nothing here.
        assert!(plan_quads(&m).is_empty());
        // A face alone (its partner's mesh holds the cap) is no wall.
        let face = quad_mesh(&[[[0.0, 0.0, 0.0], [10.0, 0.0, 0.0], [10.0, 0.0, 9.0], [0.0, 0.0, 9.0]]]);
        assert!(wall_rects(&face).is_empty());
    }

    fn wall(id: &str, a: [f64; 2], b: [f64; 2], t: f64) -> ModelWall {
        ModelWall { id: id.into(), level: "L1".into(), a, b, thickness_in: t, stud_in: None, wall_type: None }
    }

    #[test]
    fn a_wall_listed_twice_is_listed_once() {
        let ws = vec![
            wall("A", [0.0, 0.0], [10.0, 0.0], 6.0),
            wall("B", [10.0, 0.02], [0.0, 0.0], 6.2),
            wall("C", [0.0, 0.0], [10.0, 0.0], 4.0),
            wall("D", [0.0, 0.0], [10.0, 0.5], 6.0),
            ModelWall { level: "L2".into(), ..wall("E", [0.0, 0.0], [10.0, 0.0], 6.0) },
            wall("F", [20.0, 0.0], [30.0, 0.0], 6.0),
        ];
        let ids: Vec<String> = distinct_walls(ws).into_iter().map(|w| w.id).collect();
        assert_eq!(ids, ["A", "C", "D", "E", "F"]);
    }

    #[test]
    fn strip_app_keeps_other_groups() {
        let t = |c: i32, v: &str| RawTag { code: c, value: v.into() };
        let x = vec![t(1001, "A"), t(1000, "a"), t(1001, PROV_APP), t(1000, "status=fixed"), t(1001, "B"), t(1000, "b")];
        let y = strip_app(&x, PROV_APP);
        assert_eq!(y, vec![t(1001, "A"), t(1000, "a"), t(1001, "B"), t(1000, "b")]);
    }
}
