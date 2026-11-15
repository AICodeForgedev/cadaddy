//! Each piece of equipment that carries its circuit gets its connection (Q6624,
//! `E-EQUIP-DISCONNECT`, trade practice rules data).
//!
//! A heat pump, a fan or an outdoor air unit whose plan tag (or record) names its circuit is a
//! load to route, whether or not the plans draw a device at it. Where no device of that circuit
//! stands at it:
//!
//! 1. a disconnect the plans draw beside it with no circuit of its own takes the circuit (drawn);
//! 2. else its connection is built beside its housing on the side toward its panel: a disconnect
//!    for a motor or air-conditioning load (NEC 440.14, 430.102(B)), a receptacle for a
//!    cord-and-plug unit (NEC 440.13), else an equipment connection, inferred, the rule's basis
//!    on it, its equipment named (`serves`, `serves_handle`).
//!
//! The router then routes the circuit to it, and on from it to the equipment's terminal box (its
//! whip). Worked out again before every routing: what an earlier pass built goes first, so a
//! connection follows its equipment and its circuit as they stand.

use std::collections::BTreeMap;

use cadaddy_doc::props::{self, Prop, Props, Source, SourceKind};
use cadaddy_doc::{Common, Drawing, Entity, EntityKind, Handle, RawTag, Space};
use cadaddy_geom::{Vec2, Vec3};
use cadaddy_walk3d::{BuildingSpec, Made, MepEquipment};
use serde_json::{Value, json};

use super::propfill::WithNote;

/// The trade practice rule the connection comes from.
pub(crate) const RULE: &str = "E-EQUIP-DISCONNECT";
/// The property a connection this step built carries (its value the rule's id).
pub(crate) const PLACED_BY: &str = "placed_by";
/// The equipment a connection serves, by handle (hex): the router's whip ends there.
pub(crate) const SERVES_HANDLE: &str = "serves_handle";
/// Elements looked at, at most.
const MAX_ELEMENTS: usize = 2_000_000;
/// Pieces of equipment given a connection in one pass, at most.
const MAX_LOADS: usize = 20_000;
/// Rows listed in the report, at most.
const MAX_LISTED: usize = 200;

/// The rule's numbers and words, from its data.
#[derive(Clone, Debug)]
pub(crate) struct Rule {
    reach_ft: f64,
    at_ft: f64,
    drawn_reach_ft: f64,
    gap_in: f64,
    mount_in: f64,
    high_bottom_ft: f64,
    connection_kinds: Vec<String>,
    disconnect_words: Vec<String>,
    receptacle_words: Vec<String>,
    box_words: Vec<String>,
    sizes_in: BTreeMap<String, [f64; 3]>,
    basis: String,
}

/// Words as whole words: upper case, anything but letters and digits a space, a space each side.
fn spaced(s: &str) -> String {
    let w: Vec<String> = s.split(|c: char| !c.is_ascii_alphanumeric()).filter(|w| !w.is_empty()).map(str::to_ascii_uppercase).collect();
    format!(" {} ", w.join(" "))
}

/// Whether `text` holds any of `words` (each the start of a whole word: `REFRIGERAT` finds
/// `REFRIGERATION`).
fn has_any(text: &str, words: &[String]) -> bool {
    let t = spaced(text);
    words.iter().any(|w| {
        let w = spaced(w);
        let w = w.trim_end();
        !w.trim().is_empty() && t.contains(w)
    })
}

impl Rule {
    pub(crate) fn read() -> Self {
        let d = super::provenance::rule_data(RULE);
        let num = |k: &str, or: f64, most: f64| d.get(k).and_then(Value::as_f64).filter(|v| v.is_finite() && *v >= 0.0).unwrap_or(or).min(most);
        let words = |k: &str| -> Vec<String> {
            d.get(k)
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .take(200)
                .filter_map(Value::as_str)
                .map(|w| w.trim().to_ascii_uppercase())
                .filter(|w| !w.is_empty())
                .collect()
        };
        let mut sizes_in: BTreeMap<String, [f64; 3]> = BTreeMap::new();
        if let Some(m) = d.get("sizes_in").and_then(Value::as_object) {
            for (k, v) in m.iter().take(20) {
                let n: Vec<f64> = v.as_array().into_iter().flatten().filter_map(Value::as_f64).filter(|x| x.is_finite() && *x > 0.0).collect();
                if let [a, b, c] = n.as_slice() {
                    sizes_in.insert(k.to_ascii_lowercase(), [a.min(60.0), b.min(60.0), c.min(60.0)]);
                }
            }
        }
        Rule {
            reach_ft: num("reach_ft", 50.0, 500.0),
            at_ft: num("at_ft", 10.0, 100.0),
            drawn_reach_ft: num("drawn_reach_ft", 15.0, 100.0),
            gap_in: num("gap_in", 12.0, 120.0),
            mount_in: num("mount_in", 48.0, 240.0),
            high_bottom_ft: num("high_bottom_ft", 7.0, 100.0),
            connection_kinds: words("connection_kinds"),
            disconnect_words: words("disconnect_words"),
            receptacle_words: words("receptacle_words"),
            box_words: words("box_words"),
            sizes_in,
            basis: super::provenance::rule(RULE).basis_for(""),
        }
    }

    /// The box of a connection of `kind` (inches: along the housing, out from it, high).
    fn size_of(&self, kind: &str) -> [f64; 3] {
        self.sizes_in.get(kind).copied().unwrap_or(match kind {
            "disconnect" => [8.0, 5.0, 14.0],
            "receptacle" => [2.25, 2.0, 4.0],
            _ => [4.0, 4.0, 2.125],
        })
    }

    /// What connection equipment described by `words` takes: a disconnect, a receptacle or an
    /// equipment connection.
    pub(crate) fn connection_for(&self, words: &str) -> &'static str {
        if has_any(words, &self.receptacle_words) {
            "receptacle"
        } else if has_any(words, &self.disconnect_words) {
            "disconnect"
        } else {
            "equipment connection"
        }
    }

    /// Whether a device of `kind` is a connection (an equipment connection, a disconnect, a
    /// junction box at a motor).
    pub(crate) fn is_connection(&self, kind: &str) -> bool {
        has_any(kind, &self.connection_kinds)
    }

    /// Whether a device of `kind` is a disconnect.
    fn is_disconnect(kind: &str) -> bool {
        has_any(kind, &["DISCONNECT".to_string(), "SAFETY SWITCH".to_string()])
    }
}

/// A circuit label as one key (`LP1-2,4`): the panel and its numbers, else as written.
pub(crate) fn circuit_key(label: &str) -> String {
    match super::printcond::parse_circuit(label) {
        Some((p, n)) => format!("{p}-{n}"),
        None => label.split_whitespace().collect::<String>().to_ascii_uppercase(),
    }
}

/// The circuit (`LP1-2,4`) and panel a piece of equipment carries: its element's properties
/// (filled in or set), else its build record's (`prop:CIRCUIT`, `prop:PANEL`; a bare number with
/// its panel beside it). None: no circuit a router can name.
pub(crate) fn circuit_of(e: &Entity) -> Option<(String, String)> {
    let ps = props::read(&e.common.xdata);
    let text = |k: &str| ps.get(k).filter(|p| p.has_value()).map(|p| p.text().trim().to_string()).filter(|v| !v.is_empty());
    let made = |k: &str| super::bimlook::made_prop(e, k).map(str::trim).filter(|v| !v.is_empty()).map(str::to_string);
    let (circuit, panel) = match text("circuit") {
        Some(c) => (c, text("panel")),
        None => (made("CIRCUIT")?, made("PANEL")),
    };
    let label = match super::printcond::parse_circuit(&circuit) {
        Some((p, n)) => format!("{p}-{n}"),
        None => {
            let p = panel.clone()?;
            let (p2, n) = super::printcond::parse_circuit(&format!("{}-{}", p.trim(), circuit.trim()))?;
            format!("{p2}-{n}")
        }
    };
    let panel = super::printcond::parse_circuit(&label).map(|(p, _)| p).or(panel).unwrap_or_default();
    Some((label, panel))
}

/// A device of the model as this step sees it.
struct Device {
    handle: Handle,
    level: String,
    circuit: String,
    kind: String,
    at: Vec2,
    placed: bool,
}

/// A piece of equipment that carries its circuit.
struct Load {
    handle: Handle,
    tag: String,
    level: String,
    words: String,
    circuit: String,
    panel: String,
    lo: Vec3,
    hi: Vec3,
    off: Vec2,
}

impl Load {
    fn lo2(&self) -> Vec2 {
        Vec2::new(self.lo.x, self.lo.y)
    }
    fn hi2(&self) -> Vec2 {
        Vec2::new(self.hi.x, self.hi.y)
    }
    /// How far a plan point is from its housing (0 inside it).
    fn gap(&self, p: Vec2) -> f64 {
        let dx = (self.lo.x - p.x).max(p.x - self.hi.x).max(0.0);
        let dy = (self.lo.y - p.y).max(p.y - self.hi.y).max(0.0);
        (dx * dx + dy * dy).sqrt()
    }
}

/// Whether two plan boxes overlap.
fn overlap(a: (Vec2, Vec2), b: (Vec2, Vec2)) -> bool {
    a.0.x < b.1.x && b.0.x < a.1.x && a.0.y < b.1.y && b.0.y < a.1.y
}

/// The record of a built piece of equipment (its tag and level).
fn record<'a>(sp: &'a BuildingSpec, tag: &str, level: &str) -> Option<&'a MepEquipment> {
    sp.mep.equipment.iter().find(|q| q.tag.eq_ignore_ascii_case(tag) && q.level.trim().eq_ignore_ascii_case(level.trim()))
}

/// Whether a built element is a connection this step placed.
pub(crate) fn placed_here(e: &Entity) -> bool {
    props::read(&e.common.xdata).get(PLACED_BY).is_some_and(|p| p.text() == RULE)
}

/// Give each piece of equipment that carries its circuit its connection where no device of its
/// circuit stands at it (see the module docs). What an earlier pass placed goes first. The
/// report: how many loads, connected already, drawn disconnects that took their circuit, built,
/// and why any was left.
pub(crate) fn place(d: &mut Drawing, sp: &BuildingSpec) -> Value {
    let rule = Rule::read();
    let upf = if sp.units_per_foot.is_finite() && sp.units_per_foot > 0.0 { sp.units_per_foot } else { 12.0 };
    // An earlier pass's connections go; the drawn disconnects it gave a circuit keep theirs (set
    // again below, the same).
    let old: Vec<Handle> = d.model.iter().take(MAX_ELEMENTS).filter(|e| placed_here(e)).map(|e| e.handle).collect();
    for h in &old {
        d.model.remove(*h);
    }
    let gear_words: Vec<String> = super::conduitroute::rules().gear.kinds.iter().map(|w| w.to_ascii_uppercase()).collect();
    let mut devices: Vec<Device> = Vec::new();
    let mut loads: Vec<Load> = Vec::new();
    let mut boxes = 0usize;
    for e in d.model.iter().take(MAX_ELEMENTS) {
        if super::wallgen::made_as(e).as_deref() == Some("device") {
            let Some((lo, hi)) = super::conduitroute::mesh_box(e) else { continue };
            let off = super::bspec::stacked_by(e);
            let ps = props::read(&e.common.xdata);
            devices.push(Device {
                handle: e.handle,
                level: super::bspec::made_level(e).unwrap_or_default(),
                circuit: ps.get("circuit").map(Prop::text).map(|c| circuit_key(&super::propfill::split_switch_leg(c.trim()).0)).unwrap_or_default(),
                kind: ps.get("kind").map(Prop::text).unwrap_or_default().replace('_', " "),
                at: Vec2::new((lo.x + hi.x) / 2.0 - off.x, (lo.y + hi.y) / 2.0 - off.y),
                placed: false,
            });
            continue;
        }
        if loads.len() >= MAX_LOADS || !super::bimlook::made_tag(e, "part").is_some_and(|p| p.to_ascii_lowercase().starts_with("equipment")) {
            continue;
        }
        let Some((circuit, panel)) = circuit_of(e) else { continue };
        let Some((lo, hi)) = super::conduitroute::mesh_box(e) else { continue };
        let tag = super::bimlook::made_tag(e, "tag").unwrap_or_default().to_string();
        let level = super::bspec::made_level(e).unwrap_or_default();
        let rec = record(sp, &tag, &level);
        // Site work is fed by its buried runs, never through the building.
        if rec.is_some_and(super::utilityxfmr::is_site) || props::read(&e.common.xdata).get(super::sitework::MARK).is_some() {
            continue;
        }
        let kind = super::bimlook::made_tag(e, "part").and_then(|p| p.split_once(':')).map(|(_, k)| k.replace('_', " ")).unwrap_or_default();
        let words = format!("{kind} {} {tag}", rec.map(|q| q.title.as_str()).unwrap_or_default());
        // Gear's circuit is a feeder (the router's); a connection box is a connection itself.
        if has_any(&kind, &gear_words) {
            continue;
        }
        if has_any(&words, &rule.box_words) {
            boxes += 1;
            continue;
        }
        let off = super::bspec::stacked_by(e);
        let off3 = Vec3::new(off.x, off.y, 0.0);
        loads.push(Load { handle: e.handle, tag, level, words, circuit, panel, lo: lo - off3, hi: hi - off3, off });
    }
    if loads.is_empty() {
        return json!({"loads": 0, "removed": old.len(), "connection_boxes": boxes});
    }
    // Where each panel is (its level's plan, carried to another level's by the stacking).
    let moves = super::conduitroute::level_moves(d, sp);
    let move_of = |l: &str| moves.get(&l.to_ascii_lowercase()).copied().unwrap_or(Vec2::new(0.0, 0.0));
    let mut panels: Vec<(String, String, Vec2)> = Vec::new();
    for e in d.model.iter().take(MAX_ELEMENTS) {
        if !super::bimlook::made_tag(e, "part").is_some_and(|p| p.to_ascii_lowercase().starts_with("equipment")) {
            continue;
        }
        let kind = super::bimlook::made_tag(e, "part").and_then(|p| p.split_once(':')).map(|(_, k)| k.replace('_', " ")).unwrap_or_default();
        if !has_any(&kind, &gear_words) {
            continue;
        }
        let Some((lo, hi)) = super::conduitroute::mesh_box(e) else { continue };
        let off = super::bspec::stacked_by(e);
        let tag = super::bimlook::made_tag(e, "tag").unwrap_or_default().trim().to_ascii_uppercase();
        panels.push((tag, super::bspec::made_level(e).unwrap_or_default(), Vec2::new((lo.x + hi.x) / 2.0 - off.x, (lo.y + hi.y) / 2.0 - off.y)));
    }
    let elev = |l: &str| sp.level(l).map_or(0.0, |x| x.elevation_ft);
    let panel_at = |ld: &Load| -> Option<Vec2> {
        let p = ld.panel.trim().to_ascii_uppercase();
        if p.is_empty() {
            return None;
        }
        panels
            .iter()
            .filter(|(t, _, _)| *t == p)
            .min_by(|a, b| (elev(&a.1) - elev(&ld.level)).abs().total_cmp(&(elev(&b.1) - elev(&ld.level)).abs()))
            .map(|(_, l, at)| *at + move_of(l) - move_of(&ld.level))
    };
    // Every equipment box of each level (a built connection stands clear of them).
    let mut housings: Vec<(String, Vec2, Vec2)> = Vec::new();
    for e in d.model.iter().take(MAX_ELEMENTS) {
        if !super::bimlook::made_tag(e, "part").is_some_and(|p| p.to_ascii_lowercase().starts_with("equipment")) {
            continue;
        }
        if let Some((lo, hi)) = super::conduitroute::mesh_box(e) {
            let off = super::bspec::stacked_by(e);
            housings.push((
                super::bspec::made_level(e).unwrap_or_default().to_ascii_lowercase(),
                Vec2::new(lo.x - off.x, lo.y - off.y),
                Vec2::new(hi.x - off.x, hi.y - off.y),
            ));
        }
    }
    let layer = super::wallgen::layer_of(cadaddy_walk3d::Part::Device);
    if d.layer(layer).is_none() {
        d.layers.push(cadaddy_doc::Layer { name: layer.into(), color: cadaddy_doc::color::Color::Index(30), ..cadaddy_doc::Layer::default() });
    }
    let (reach, at_reach, drawn_reach) = (rule.reach_ft * upf, rule.at_ft * upf, rule.drawn_reach_ft * upf);
    let mut connected = 0usize;
    let mut took: Vec<Value> = Vec::new();
    let mut built: Vec<Value> = Vec::new();
    let mut by_kind: BTreeMap<&'static str, usize> = BTreeMap::new();
    let mut adopted: Vec<(Handle, &Load)> = Vec::new();
    for ld in &loads {
        let same = |dv: &Device| dv.level.trim().eq_ignore_ascii_case(ld.level.trim());
        // A device of its circuit at it, or within sight and a connection's kind: connected.
        let have = devices
            .iter()
            .filter(|dv| same(dv) && dv.circuit == ld.circuit)
            .any(|dv| ld.gap(dv.at) <= at_reach || (ld.gap(dv.at) <= reach && rule.is_connection(&dv.kind)));
        if have {
            connected += 1;
            continue;
        }
        // A disconnect drawn beside it with no circuit: its disconnect.
        let drawn = devices
            .iter_mut()
            .filter(|dv| !dv.placed && same(dv) && dv.circuit.is_empty() && Rule::is_disconnect(&dv.kind) && ld.gap(dv.at) <= drawn_reach)
            .min_by(|a, b| ld.gap(a.at).total_cmp(&ld.gap(b.at)));
        if let Some(dv) = drawn {
            dv.placed = true;
            dv.circuit = ld.circuit.clone();
            adopted.push((dv.handle, ld));
            if took.len() < MAX_LISTED {
                took.push(json!({"equipment": ld.tag, "level": ld.level, "circuit": ld.circuit, "disconnect": dv.handle.hex()}));
            }
            continue;
        }
        // Else built beside its housing, toward its panel, clear of the other equipment.
        let kind = rule.connection_for(&ld.words);
        let size = rule.size_of(kind);
        let inch = upf / 12.0;
        let (lo, hi) = (ld.lo2(), ld.hi2());
        let c = (lo + hi) * 0.5;
        let gap = rule.gap_in * inch;
        let depth = size[1] * inch;
        let half_w = size[0] * inch / 2.0;
        // Each side: the box's middle, its outward direction.
        let out = gap + depth / 2.0;
        let sides = [
            (Vec2::new(hi.x + out, c.y), Vec2::new(1.0, 0.0)),
            (Vec2::new(lo.x - out, c.y), Vec2::new(-1.0, 0.0)),
            (Vec2::new(c.x, hi.y + out), Vec2::new(0.0, 1.0)),
            (Vec2::new(c.x, lo.y - out), Vec2::new(0.0, -1.0)),
        ];
        let toward = panel_at(ld);
        let mut order: Vec<usize> = (0..sides.len()).collect();
        if let Some(p) = toward {
            order.sort_by(|a, b| {
                let dd = |i: usize| sides.get(i).map_or(f64::INFINITY, |(q, _)| (q.x - p.x).abs() + (q.y - p.y).abs());
                dd(*a).total_cmp(&dd(*b))
            });
        }
        let level_l = ld.level.to_ascii_lowercase();
        let footprint = |at: Vec2, dir: Vec2| {
            let along = Vec2::new(-dir.y, dir.x);
            let a = at - along * half_w - dir * (depth / 2.0);
            let b = at + along * half_w + dir * (depth / 2.0);
            (Vec2::new(a.x.min(b.x), a.y.min(b.y)), Vec2::new(a.x.max(b.x), a.y.max(b.y)))
        };
        let clear = |at: Vec2, dir: Vec2| {
            let fp = footprint(at, dir);
            !housings.iter().any(|(l, a, b)| *l == level_l && overlap(fp, (*a, *b)) && !overlap((*a, *b), (lo, hi)))
        };
        let Some((at, dir)) = order
            .iter()
            .filter_map(|i| sides.get(*i).copied())
            .find(|(at, dir)| clear(*at, *dir))
            .or_else(|| order.first().and_then(|i| sides.get(*i).copied()))
        else {
            continue;
        };
        // Its middle: the rule's height over the floor; a unit hung high, at the unit's middle.
        let floor = elev(&ld.level) * upf;
        let bottom_ft = (ld.lo.z - floor) / upf;
        let mid = if bottom_ft >= rule.high_bottom_ft { (ld.lo.z + ld.hi.z) / 2.0 } else { floor + rule.mount_in * inch };
        let (z0, z1) = (mid - size[2] * inch / 2.0, mid + size[2] * inch / 2.0);
        let along = Vec2::new(-dir.y, dir.x);
        let mut m = super::wallgen::empty_mesh();
        let world = at + ld.off;
        super::wallgen::add_box(&mut m, world - along * half_w, world + along * half_w, depth, z0, z1);
        if m.faces.is_empty() {
            continue;
        }
        let why = format!(
            "{} carries circuit {} and no device of that circuit stands at it: its {kind} beside its housing{}",
            ld.tag,
            ld.circuit,
            if toward.is_some() { format!(", on the side toward panel {}", ld.panel) } else { String::new() }
        );
        let basis = super::provenance::rule(RULE).basis_for(&why);
        let mut xdata = super::wallgen::tag(cadaddy_walk3d::Part::Device, &ld.level, Handle(0));
        xdata.push(RawTag { code: 1000, value: format!("{}:{}", super::elemprops::ELEMENT_TAG, "device") });
        xdata.extend(super::provenance::tags(Made::Inferred, &basis));
        if ld.off.len() > 1e-9 {
            xdata = super::bspec::with_stacked(&xdata, ld.off);
        }
        let src = Source::from_doc(SourceKind::Rule, RULE);
        let mut store = Props::default();
        store.set("kind", Prop::new(kind.replace(' ', "_"), src.clone()).with_note(&basis));
        store.set("circuit", Prop::new(ld.circuit.clone(), Source::computed()).with_note(&format!("the circuit {} carries", ld.tag)));
        if !ld.panel.is_empty() {
            store.set("panel", Prop::new(ld.panel.clone(), Source::computed()));
            store.set("fed_from", Prop::new(format!("panel {}, circuit {}", ld.panel, ld.circuit), Source::computed()));
        }
        store.set("serves", Prop::new(ld.tag.clone(), src.clone()));
        store.set(SERVES_HANDLE, Prop::new(ld.handle.hex(), src.clone()));
        store.set("mount_height", Prop::new(((mid - floor) / inch * 100.0).round() / 100.0, src.clone()).with_unit("in"));
        store.set("mounting", Prop::new("beside its equipment", src.clone()));
        store.set(PLACED_BY, Prop::new(RULE, src.clone()));
        let xdata = props::write(&xdata, &store);
        let common = Common { layer: layer.into(), xdata: xdata.into(), ..Common::default() };
        let Ok(h) = d.add(&Space::Model, common, EntityKind::Mesh(m)) else { continue };
        *by_kind.entry(kind).or_default() += 1;
        if built.len() < MAX_LISTED {
            built.push(json!({"equipment": ld.tag, "level": ld.level, "circuit": ld.circuit, "kind": kind, "handle": h.hex(), "made": "inferred", "basis": basis}));
        }
    }
    // The drawn disconnects that took their equipment's circuit.
    for (h, ld) in &adopted {
        let why = format!("the disconnect drawn beside {} is its disconnect: the circuit {} it carries", ld.tag, ld.circuit);
        let src = Source::from_doc(SourceKind::Rule, RULE);
        let _ = d.modify_entity(*h, |e| {
            let mut ps = props::read(&e.common.xdata);
            ps.set("circuit", Prop::new(ld.circuit.clone(), src.clone()).with_note(&why));
            if !ld.panel.is_empty() {
                ps.set("panel", Prop::new(ld.panel.clone(), src.clone()).with_note(&why));
                ps.set("fed_from", Prop::new(format!("panel {}, circuit {}", ld.panel, ld.circuit), src.clone()));
            }
            ps.set("serves", Prop::new(ld.tag.clone(), src.clone()).with_note(&why));
            ps.set(SERVES_HANDLE, Prop::new(ld.handle.hex(), src.clone()));
            e.common.xdata = props::write(&e.common.xdata, &ps).into();
        });
    }
    json!({
        "loads": loads.len(),
        "connected": connected,
        "drawn_disconnects": adopted.len(),
        "built": by_kind.values().sum::<usize>(),
        "built_by_kind": by_kind,
        "removed": old.len(),
        "connection_boxes": boxes,
        "took_circuit": took,
        "built_list": built,
        "rule": rule.basis,
    })
}
