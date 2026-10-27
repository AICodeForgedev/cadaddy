//! `bim.props.check`: findings on what the elements' properties say.
//!
//! Each check reads the properties (and the links between elements: `fed_from`, `served_by`,
//! `waste`, `vent`, `host_wall`) and lists what to look at, never changing the model:
//!
//! - general: `rated_door` (a door in a rated wall with no rating), `room_number` (a room with no
//!   number), `default_height` (a device whose mounting height is assumed).
//! - electrical: `no_circuit`, `no_fed_from` (a circuit with no panel), `gfci_wet` (a receptacle
//!   in a wet room that is not GFCI), `circuit_load` (connected load over 80 % of the breaker),
//!   `wire_breaker` (a breaker larger than the wire allows), `conduit_fill`, `voltage_drop`
//!   (over 3 % on the circuit's route), `equipment_mocp` (a breaker over the unit's MOCP or under
//!   its MCA).
//! - mechanical: `terminal_cfm` (terminals' airflow against their unit's, ±10 %), `duct_velocity`,
//!   `fire_damper` (a duct through a rated wall with no damper), `terminal_unserved`,
//!   `equipment_power` (equipment with no circuit).
//! - plumbing: `drain_dfu` (drainage fixture units through each drain against its size),
//!   `pipe_slope`, `fixture_waste_vent`, `water_heater` (recovery against the fixtures' demand),
//!   `gas_size` (each gas pipe's load against its size at the run's length).

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{Value, json};

use super::codes::{self, DrainRole, Section};
use super::{El, Model, circuit_length, circuits, is_panel, is_plumbing_fixture, is_receptacle, is_water_heater};
use crate::cmd::{bad, str_param};
use crate::{Result, Session};

pub(crate) const CHECK: &str = "bim.props.check";

/// Every check, in the order they run.
pub const CHECKS: [&str; 21] = [
    "no_circuit",
    "no_fed_from",
    "default_height",
    "rated_door",
    "room_number",
    "gfci_wet",
    "circuit_load",
    "wire_breaker",
    "conduit_fill",
    "voltage_drop",
    "equipment_mocp",
    "terminal_cfm",
    "duct_velocity",
    "fire_damper",
    "terminal_unserved",
    "equipment_power",
    "drain_dfu",
    "pipe_slope",
    "fixture_waste_vent",
    "water_heater",
    "gas_size",
];

/// Most findings one check lists (the count says how many there were).
const MAX_FINDINGS: usize = 2_000;
/// Most water heaters checked.
const MAX_HEATERS: usize = 200;
/// Most links followed from one fixture or appliance up its chain.
const MAX_CHAIN: usize = 200;

/// The checks' limits.
struct Limits {
    cfm_tolerance_pct: f64,
    duct_velocity_fpm: f64,
    voltage_drop_pct: f64,
    load_pct: f64,
    hw_demand_factor: f64,
    gas_btu_per_cf: f64,
}

impl Limits {
    fn from(v: &Value) -> Limits {
        let f = |k: &str, d: f64| v[k].as_f64().filter(|x| x.is_finite() && *x > 0.0).unwrap_or(d);
        Limits {
            cfm_tolerance_pct: f("cfm_tolerance_pct", 10.0),
            duct_velocity_fpm: f("duct_velocity_fpm", 2000.0),
            voltage_drop_pct: f("voltage_drop_pct", 3.0),
            load_pct: f("load_pct", 80.0),
            hw_demand_factor: f("hw_demand_factor", 0.25),
            gas_btu_per_cf: f("gas_btu_per_cf", 1000.0),
        }
    }

    fn json(&self) -> Value {
        json!({"cfm_tolerance_pct": self.cfm_tolerance_pct, "duct_velocity_fpm": self.duct_velocity_fpm, "voltage_drop_pct": self.voltage_drop_pct,
            "load_pct": self.load_pct, "hw_demand_factor": self.hw_demand_factor, "gas_btu_per_cf": self.gas_btu_per_cf})
    }
}

/// The findings being collected.
#[derive(Default)]
struct Out {
    findings: Vec<Value>,
    counts: BTreeMap<&'static str, usize>,
    checked: BTreeMap<&'static str, usize>,
}

impl Out {
    fn add(&mut self, check: &'static str, severity: &str, el: Option<&El>, message: String, values: Value) {
        let n = self.counts.entry(check).or_insert(0);
        *n += 1;
        if *n > MAX_FINDINGS {
            return;
        }
        let mut f = json!({"check": check, "severity": severity, "message": message});
        if let Some(el) = el {
            f["handle"] = json!(el.handle().hex());
            f["kind"] = json!(el.kind);
            let lv = el.level();
            if !lv.is_empty() {
                f["level"] = json!(lv);
            }
        }
        if !values.is_null() {
            f["values"] = values;
        }
        self.findings.push(f);
    }

    fn checked(&mut self, check: &'static str, n: usize) {
        *self.checked.entry(check).or_insert(0) += n;
    }
}

fn r2(v: f64) -> f64 {
    (v * 100.0).round() / 100.0
}

/// A fire rating in hours, from `1 HR`, `2-HOUR`, `60 min`, `45`, `none`.
fn rating_hours(t: &str) -> Option<f64> {
    let l = t.trim().to_ascii_lowercase();
    if matches!(l.as_str(), "" | "none" | "nr" | "n/a" | "-" | "0" | "no" | "non-rated" | "unrated") {
        return Some(0.0);
    }
    let v = codes::lead_num(&l)?;
    Some(if l.contains("min") || (v >= 20.0 && !l.contains("h")) { v / 60.0 } else { v })
}

fn rated(el: &El) -> Option<(f64, String)> {
    let t = el.text("fire_rating")?;
    rating_hours(&t).filter(|h| *h > 0.0).map(|h| (h, t))
}

const WET: [&str; 22] = [
    "toilet",
    "restroom",
    "rest room",
    "bath",
    "washroom",
    "lavatory",
    "shower",
    "locker",
    "kitchen",
    "break",
    "pantry",
    "laundry",
    "janitor",
    "jan.",
    "mop",
    "custodial",
    "pool",
    "spa",
    "garage",
    "wet",
    "outdoor",
    "exterior",
];

fn is_gfci(el: &El) -> bool {
    let w = el.words();
    w.contains("gfci")
        || w.contains("gfi")
        || w.split(|c: char| !c.is_ascii_alphanumeric()).any(|x| x == "gf" || x == "wp" && w.contains("gf"))
        || el
            .get("gfci")
            .is_some_and(|p| p.value.as_bool() == Some(true) || matches!(p.text().trim().to_ascii_lowercase().as_str(), "yes" | "true" | "y"))
}

/// A room's name as the model knows it, from the label a device carries (`101`, `101 TOILET`).
fn room_words(m: &Model, label: &str) -> String {
    let up = label.trim().to_ascii_uppercase();
    let first = up.split_whitespace().next().unwrap_or("").to_string();
    let named = m
        .els
        .iter()
        .filter(|e| e.kind == "room")
        .find(|r| {
            r.text("number").is_some_and(|n| n.eq_ignore_ascii_case(&up) || n.eq_ignore_ascii_case(&first))
                || r.text("name").is_some_and(|n| n.eq_ignore_ascii_case(&up))
        })
        .and_then(|r| r.text("name"))
        .unwrap_or_default();
    format!("{label} {named}").to_ascii_lowercase()
}

/// Two plan boxes overlap.
fn overlap(a: &cadaddy_geom::Bounds2, b: &cadaddy_geom::Bounds2, tol: f64) -> bool {
    a.min.x <= b.max.x + tol && b.min.x <= a.max.x + tol && a.min.y <= b.max.y + tol && b.min.y <= a.max.y + tol
}

fn same_level(a: &El, b: &El) -> bool {
    let (x, y) = (a.level(), b.level());
    x.is_empty() || y.is_empty() || x.eq_ignore_ascii_case(&y)
}

/// The wall a door is in: its `host_wall`, else the wall of its level its box overlaps most.
fn host_wall<'m>(m: &'m Model, door: &El) -> Option<&'m El<'m>> {
    if let Some(w) = m.link(door, &["host_wall", "wall"]).and_then(|i| m.get(i)) {
        return Some(w);
    }
    let db = door.plan();
    let tol = 0.25 * m.upf;
    m.els.iter().filter(|w| w.kind == "wall" && same_level(w, door) && overlap(&w.plan(), &db, tol)).max_by(|a, b| {
        let i = |w: &El| {
            let x = w.plan().intersection(&db);
            if x.is_empty() { 0.0 } else { (x.max.x - x.min.x).max(0.0) + (x.max.y - x.min.y).max(0.0) }
        };
        i(a).total_cmp(&i(b))
    })
}

/// A duct crosses a wall: it reaches past both faces of the wall's thin side and overlaps its
/// length.
fn crosses(duct: &El, wall: &El, tol: f64) -> bool {
    let (w, d) = (wall.plan(), duct.plan());
    let (dx, dy) = (w.max.x - w.min.x, w.max.y - w.min.y);
    if dx <= dy {
        d.min.x < w.min.x - tol && d.max.x > w.max.x + tol && d.max.y > w.min.y && d.min.y < w.max.y
    } else {
        d.min.y < w.min.y - tol && d.max.y > w.max.y + tol && d.max.x > w.min.x && d.min.x < w.max.x
    }
}

fn is_damper(el: &El) -> bool {
    let w = el.words();
    let mark = el.text("type_mark").unwrap_or_default().to_ascii_uppercase();
    w.contains("damper") || matches!(mark.as_str(), "FD" | "FSD" | "SD" | "CRD")
}

/// The chain of elements up from `start` through `keys` links (fixture → branch → stack → main;
/// appliance → gas branch → main), at most [`MAX_CHAIN`] long, stopping at a loop.
fn chain(m: &Model, start: usize, keys: &[&str]) -> Vec<usize> {
    let mut out = Vec::new();
    let mut seen = BTreeSet::from([start]);
    let mut at = start;
    for _ in 0..MAX_CHAIN {
        let Some(next) = m.get(at).and_then(|el| m.link(el, keys)) else { break };
        if !seen.insert(next) {
            break;
        }
        out.push(next);
        at = next;
    }
    out
}

fn drain_system(el: &El) -> bool {
    let sys = el.first(&["system", "service"]).unwrap_or_default().to_ascii_uppercase();
    let words: Vec<&str> = sys.split(|c: char| !c.is_ascii_alphanumeric()).collect();
    ["SAN", "SANITARY", "WASTE", "SOIL", "W", "SS", "STORM", "SD", "RD", "DRAIN", "GW", "IW"].iter().any(|k| words.contains(k))
        || sys.contains("WASTE")
        || sys.contains("SANITARY")
}

fn gas_system(el: &El) -> bool {
    let sys = el.first(&["system", "service"]).unwrap_or_default().to_ascii_uppercase();
    sys.contains("GAS") || sys.trim() == "G" || sys.trim() == "NG"
}

fn drain_role(m: &Model, el: &El) -> DrainRole {
    let t = format!("{} {}", el.first(&["role", "part"]).unwrap_or_default(), el.tag().unwrap_or_default()).to_ascii_lowercase();
    if t.contains("stack") || t.contains("riser") {
        return DrainRole::Stack;
    }
    if t.contains("main") || t.contains("building drain") || t.contains("building sewer") {
        return DrainRole::Main;
    }
    // A pipe standing up is a stack.
    let (dx, dy, dz) = (el.hi[0] - el.lo[0], el.hi[1] - el.lo[1], el.hi[2] - el.lo[2]);
    if dz > dx.max(dy) * 2.0 && dz > 2.0 * m.upf { DrainRole::Stack } else { DrainRole::Branch }
}

fn wants(set: &BTreeSet<&str>, c: &str) -> bool {
    set.contains(c)
}

pub(crate) fn run(s: &mut Session, p: &Value) -> Result<Value> {
    let set: BTreeSet<&str> = match p.get("checks") {
        None | Some(Value::Null) => CHECKS.iter().copied().collect(),
        Some(Value::Array(a)) => {
            let mut out = BTreeSet::new();
            for v in a.iter().take(64) {
                let name = v.as_str().map(str::trim).unwrap_or("");
                let Some(c) = CHECKS.iter().find(|c| c.eq_ignore_ascii_case(name)) else {
                    return Err(bad(CHECK, format!("no check `{name}`: {}", CHECKS.join(", "))));
                };
                out.insert(*c);
            }
            out
        }
        Some(Value::String(t)) => {
            let Some(c) = CHECKS.iter().find(|c| c.eq_ignore_ascii_case(t.trim())) else {
                return Err(bad(CHECK, format!("no check `{t}`: {}", CHECKS.join(", "))));
            };
            BTreeSet::from([*c])
        }
        Some(_) => return Err(bad(CHECK, "`checks` is a list of check names")),
    };
    let level = str_param(p, "level").map(str::trim).filter(|l| !l.is_empty()).map(str::to_string);
    let lim = Limits::from(p.get("limits").unwrap_or(&Value::Null));
    let d = s.doc()?;
    let m = Model::new(d);
    let on = |el: &El| level.as_deref().is_none_or(|l| el.level().eq_ignore_ascii_case(l));
    let els: Vec<&El> = m.els.iter().filter(|e| on(e)).collect();
    let mut out = Out::default();
    let mut totals = json!({});

    // ── General ──
    let devices: Vec<&El> = els.iter().copied().filter(|e| matches!(e.kind.as_str(), "device" | "light_fixture")).collect();
    if wants(&set, "no_circuit") {
        out.checked("no_circuit", devices.len());
        for el in devices.iter().filter(|e| e.first(&["circuit", "fed_from"]).is_none()) {
            out.add("no_circuit", "warning", Some(el), format!("{} has no circuit", el.label()), Value::Null);
        }
    }
    if wants(&set, "no_fed_from") {
        let with: Vec<&&El> = devices.iter().filter(|e| e.text("circuit").is_some()).collect();
        out.checked("no_fed_from", with.len());
        for el in with {
            let panel = el.text("panel").is_some()
                || el.text("fed_from").is_some_and(|f| f.to_ascii_lowercase().contains("panel"))
                || el.text("fed_from_handle").is_some()
                || super::circuit_of(el).is_some_and(|c| !c.panel.is_empty());
            if !panel {
                out.add(
                    "no_fed_from",
                    "warning",
                    Some(el),
                    format!("{} is on circuit {} but nothing says which panel feeds it", el.label(), el.text("circuit").unwrap_or_default()),
                    Value::Null,
                );
            }
        }
    }
    if wants(&set, "default_height") {
        out.checked("default_height", devices.len());
        for el in &devices {
            match el.get("mount_height") {
                Some(p) if p.source.kind.is_assumed() => out.add(
                    "default_height",
                    "info",
                    Some(el),
                    format!("{}: mounting height {} is assumed ({}), not read from the prints", el.label(), p.text(), p.source.kind.as_str()),
                    json!({"mount_height": p.text()}),
                ),
                None => out.add("default_height", "info", Some(el), format!("{} has no mounting height", el.label()), Value::Null),
                _ => {}
            }
        }
    }
    if wants(&set, "rated_door") {
        let doors: Vec<&El> = els.iter().copied().filter(|e| e.kind == "door").collect();
        out.checked("rated_door", doors.len());
        for door in doors {
            let Some(wall) = host_wall(&m, door) else { continue };
            let Some((hours, text)) = rated(wall) else { continue };
            let door_rating = door.text("fire_rating").and_then(|t| rating_hours(&t)).unwrap_or(0.0);
            if door_rating <= 0.0 {
                out.add(
                    "rated_door",
                    "error",
                    Some(door),
                    format!("{} is in a {text} rated wall ({}) but has no fire rating", door.label(), wall.label()),
                    json!({"wall": wall.handle().hex(), "wall_rating_hr": hours}),
                );
            }
        }
    }
    if wants(&set, "room_number") {
        let rooms: Vec<&El> = els.iter().copied().filter(|e| e.kind == "room").collect();
        out.checked("room_number", rooms.len());
        for r in rooms.iter().filter(|r| r.text("number").is_none()) {
            out.add("room_number", "warning", Some(r), format!("{} has no number", r.label()), Value::Null);
        }
    }
    if wants(&set, "gfci_wet") {
        let recs: Vec<&El> = els.iter().copied().filter(|e| is_receptacle(e)).collect();
        out.checked("gfci_wet", recs.len());
        for el in recs {
            let Some(room) = el.text("room") else { continue };
            let words = room_words(&m, &room);
            if WET.iter().any(|w| words.contains(w)) && !is_gfci(el) {
                out.add(
                    "gfci_wet",
                    "error",
                    Some(el),
                    format!("{} is in {room} (a wet room) but is not GFCI protected", el.label()),
                    json!({"room": room}),
                );
            }
        }
    }

    // ── Electrical circuits ──
    let cs = circuits(&m, level.as_deref());
    if wants(&set, "circuit_load") {
        out.checked("circuit_load", cs.len());
        for c in cs.values() {
            let Some(brk) = c.breaker.filter(|b| *b > 0.0) else { continue };
            let volts = c.voltage.unwrap_or(120.0);
            let poles = f64::from(c.poles.unwrap_or(1).max(1));
            // Single phase line-to-neutral, or across poles (208/240 V two pole).
            let capacity = brk * volts * if poles >= 2.0 && volts < 200.0 { poles } else { 1.0 };
            let pct = c.load_va / capacity * 100.0;
            if pct > lim.load_pct {
                let first = c.devices.first().copied();
                out.add(
                    "circuit_load",
                    if pct > 100.0 { "error" } else { "warning" },
                    first,
                    format!("circuit {}: {} VA on a {}A breaker is {}% of it (limit {}%)", c.id.key, r2(c.load_va), r2(brk), r2(pct), lim.load_pct),
                    json!({"circuit": c.id.key, "load_va": r2(c.load_va), "breaker_a": brk, "percent": r2(pct), "assumed_loads": c.assumed_load}),
                );
            }
        }
    }
    if wants(&set, "wire_breaker") {
        let mut n = 0;
        for c in cs.values() {
            let (Some(brk), Some(w)) = (c.breaker, c.wire.as_deref().and_then(codes::Wire::parse)) else { continue };
            let Some(size) = w.phase_size() else { continue };
            n += 1;
            if let Some(max) = codes::max_breaker(size)
                && brk > max
            {
                out.add(
                    "wire_breaker",
                    "error",
                    c.devices.first().copied(),
                    format!("circuit {}: #{size} wire may be protected at {max}A at most, the breaker is {}A", c.id.key, r2(brk)),
                    json!({"circuit": c.id.key, "wire": size, "breaker_a": brk, "max_a": max}),
                );
            }
        }
        // Equipment's own breaker and feeder.
        for el in els.iter().filter(|e| e.kind == "equipment") {
            let (Some(brk), Some(w)) =
                (el.num(&["breaker", "ocpd"]), el.first(&["wire", "wire_size", "feeder"]).and_then(|t| codes::Wire::parse(&t)))
            else {
                continue;
            };
            let Some(size) = w.phase_size() else { continue };
            n += 1;
            if let Some(max) = codes::max_breaker(size).filter(|max| brk > *max) {
                out.add(
                    "wire_breaker",
                    "error",
                    Some(el),
                    format!("{}: #{size} wire may be protected at {max}A at most, the breaker is {}A", el.label(), r2(brk)),
                    json!({"wire": size, "breaker_a": brk, "max_a": max}),
                );
            }
        }
        out.checked("wire_breaker", n);
    }
    if wants(&set, "conduit_fill") {
        let mut n = 0;
        for el in els.iter().filter(|e| e.kind == "conduit") {
            let wire = el
                .first(&["wire", "wire_size", "conductors"])
                .or_else(|| super::circuit_of(el).and_then(|c| cs.get(&c.key)).and_then(|c| c.wire.clone()));
            let Some(w) = wire.as_deref().and_then(codes::Wire::parse) else { continue };
            let trade = el.text("size").and_then(|t| codes::inches(&t)).or(w.conduit_in);
            let (Some(trade), Some(area)) = (trade, w.area()) else { continue };
            let Some(inside) = codes::emt_area(trade) else { continue };
            n += 1;
            let fill = area / inside;
            let limit = codes::fill_limit(w.count());
            if fill > limit {
                out.add(
                    "conduit_fill",
                    "error",
                    Some(el),
                    format!(
                        "{}: {} conductor(s) fill {}% of a {}\" conduit (limit {}%)",
                        el.label(),
                        w.count(),
                        r2(fill * 100.0),
                        trade,
                        r2(limit * 100.0)
                    ),
                    json!({"conductors": w.count(), "fill_pct": r2(fill * 100.0), "limit_pct": r2(limit * 100.0), "trade_size_in": trade}),
                );
            }
        }
        out.checked("conduit_fill", n);
    }
    if wants(&set, "voltage_drop") {
        let mut n = 0;
        for c in cs.values() {
            let Some(size) = c.wire.as_deref().and_then(codes::Wire::parse).and_then(|w| w.phase_size().map(str::to_string)) else { continue };
            if c.load_va <= 0.0 {
                continue;
            }
            let volts = c.voltage.unwrap_or(120.0);
            let (len, estimated) = circuit_length(&m, c);
            let Some(vd) = codes::voltage_drop_pct(c.load_va / volts, len, &size, volts) else { continue };
            n += 1;
            if vd > lim.voltage_drop_pct {
                out.add(
                    "voltage_drop",
                    "warning",
                    c.devices.first().copied(),
                    format!("circuit {}: {}% voltage drop over {} ft of #{size} at {} VA (limit {}%)", c.id.key, r2(vd), r2(len), r2(c.load_va), lim.voltage_drop_pct),
                    json!({"circuit": c.id.key, "drop_pct": r2(vd), "length_ft": r2(len), "route": if estimated { "estimated route" } else { "conduit" }}),
                );
            }
        }
        out.checked("voltage_drop", n);
    }
    if wants(&set, "equipment_mocp") {
        let mut n = 0;
        for el in els.iter().filter(|e| e.kind == "equipment") {
            let brk = el.num(&["breaker", "ocpd"]);
            let (mocp, mca) = (el.num(&["mocp"]), el.num(&["mca"]));
            let Some(brk) = brk else { continue };
            if mocp.is_none() && mca.is_none() {
                continue;
            }
            n += 1;
            if let Some(mo) = mocp.filter(|mo| brk > *mo) {
                out.add(
                    "equipment_mocp",
                    "error",
                    Some(el),
                    format!("{}: the {}A breaker is over its MOCP of {}A", el.label(), r2(brk), r2(mo)),
                    json!({"breaker_a": brk, "mocp_a": mo}),
                );
            }
            if let Some(ma) = mca.filter(|ma| brk < *ma) {
                out.add(
                    "equipment_mocp",
                    "error",
                    Some(el),
                    format!("{}: the {}A breaker is under its MCA of {}A", el.label(), r2(brk), r2(ma)),
                    json!({"breaker_a": brk, "mca_a": ma}),
                );
            }
        }
        out.checked("equipment_mocp", n);
    }

    // ── Mechanical ──
    let (by_unit, unserved) = super::air_systems(&m, level.as_deref());
    if wants(&set, "terminal_cfm") {
        out.checked("terminal_cfm", by_unit.len());
        for (u, terms) in &by_unit {
            let Some(unit) = m.get(*u) else { continue };
            let Some(ucfm) = unit
                .num(&["cfm", "airflow"])
                .or_else(|| unit.text("capacity").filter(|c| c.to_ascii_lowercase().contains("cfm")).and_then(|c| codes::lead_num(&c)))
            else {
                continue;
            };
            let sum: f64 = terms.iter().filter_map(|t| t.num(&["cfm", "airflow"])).sum();
            let diff = if ucfm > 0.0 { (sum - ucfm) / ucfm * 100.0 } else { 0.0 };
            if diff.abs() > lim.cfm_tolerance_pct {
                out.add(
                    "terminal_cfm",
                    "warning",
                    Some(unit),
                    format!(
                        "{}: its {} terminal(s) total {} cfm against the unit's {} cfm ({}%)",
                        unit.label(),
                        terms.len(),
                        r2(sum),
                        r2(ucfm),
                        r2(diff)
                    ),
                    json!({"unit_cfm": ucfm, "terminal_cfm": r2(sum), "difference_pct": r2(diff), "terminals": terms.len()}),
                );
            }
        }
    }
    if wants(&set, "duct_velocity") {
        let mut n = 0;
        for el in els.iter().filter(|e| e.kind == "duct") {
            let (Some(cfm), Some(sec)) = (el.num(&["cfm", "airflow"]), el.text("size").and_then(|t| Section::parse(&t))) else { continue };
            let a = sec.area_sf();
            if a <= 0.0 {
                continue;
            }
            n += 1;
            let fpm = cfm / a;
            let limit = el.num(&["velocity_limit", "max_velocity"]).unwrap_or(lim.duct_velocity_fpm);
            if fpm > limit {
                out.add(
                    "duct_velocity",
                    "warning",
                    Some(el),
                    format!(
                        "{}: {} cfm through {} is {} fpm (limit {} fpm)",
                        el.label(),
                        r2(cfm),
                        el.text("size").unwrap_or_default(),
                        r2(fpm),
                        r2(limit)
                    ),
                    json!({"cfm": cfm, "velocity_fpm": r2(fpm), "limit_fpm": limit}),
                );
            }
        }
        out.checked("duct_velocity", n);
    }
    if wants(&set, "fire_damper") {
        let rated_walls: Vec<(&El, f64, String)> =
            m.els.iter().filter(|e| e.kind == "wall").filter_map(|w| rated(w).map(|(h, t)| (w, h, t))).collect();
        let dampers: Vec<&El> = m.els.iter().filter(|e| is_damper(e)).collect();
        let tol = 0.05 * m.upf;
        let mut n = 0;
        for duct in els.iter().filter(|e| e.kind == "duct" && !is_damper(e)) {
            for (wall, hours, text) in rated_walls.iter().filter(|(w, _, _)| same_level(w, duct) && crosses(duct, w, tol)) {
                n += 1;
                let own =
                    duct.first(&["damper", "fire_damper"]).is_some_and(|t| !matches!(t.to_ascii_lowercase().as_str(), "none" | "no" | "false" | "0"));
                let at = wall.plan().intersection(&duct.plan());
                let near = dampers.iter().any(|dm| same_level(dm, duct) && overlap(&dm.plan(), &at, 2.0 * m.upf));
                if !own && !near {
                    out.add(
                        "fire_damper",
                        "error",
                        Some(duct),
                        format!("{} passes through a {text} rated wall ({}) with no fire damper", duct.label(), wall.label()),
                        json!({"wall": wall.handle().hex(), "wall_rating_hr": hours}),
                    );
                }
            }
        }
        out.checked("fire_damper", n);
    }
    if wants(&set, "terminal_unserved") {
        out.checked("terminal_unserved", unserved.len() + by_unit.values().map(Vec::len).sum::<usize>());
        for t in &unserved {
            out.add("terminal_unserved", "warning", Some(t), format!("{} is not served by any unit", t.label()), Value::Null);
        }
    }
    if wants(&set, "equipment_power") {
        let eq: Vec<&El> = els.iter().copied().filter(|e| e.kind == "equipment" && !is_panel(e)).collect();
        out.checked("equipment_power", eq.len());
        for el in eq {
            let powered = el.first(&["circuit", "fed_from", "fed_from_handle"]).is_some();
            let none = el.text("power").is_some_and(|t| matches!(t.to_ascii_lowercase().as_str(), "none" | "gas" | "no"));
            if !powered && !none {
                out.add("equipment_power", "warning", Some(el), format!("{} has no power circuit", el.label()), Value::Null);
            }
        }
    }

    // ── Plumbing ──
    let fixtures: Vec<&El> = els.iter().copied().filter(|e| is_plumbing_fixture(e)).collect();
    let no_drain = |el: &El| {
        let w = el.words();
        ["hose bibb", "hydrant", "cleanout", "water heater", "eyewash", "meter", "backflow", "valve"].iter().any(|x| w.contains(x))
    };
    if wants(&set, "fixture_waste_vent") {
        let need: Vec<&&El> = fixtures.iter().filter(|e| !no_drain(e)).collect();
        out.checked("fixture_waste_vent", need.len());
        for el in need {
            let waste = el.first(&["waste", "drains_to", "drain"]).is_some();
            let vent = el.first(&["vent", "vented_by"]).is_some();
            if !waste || !vent {
                let what = match (waste, vent) {
                    (false, false) => "no waste and no vent connection",
                    (false, true) => "no waste connection",
                    _ => "no vent connection",
                };
                out.add("fixture_waste_vent", "warning", Some(el), format!("{} has {what}", el.label()), json!({"waste": waste, "vent": vent}));
            }
        }
    }
    if wants(&set, "drain_dfu") {
        // Each fixture's units go down its chain of drains.
        let mut dfu: BTreeMap<usize, (f64, usize)> = BTreeMap::new();
        let mut assumed = 0usize;
        for (i, el) in m.els.iter().enumerate().filter(|(_, e)| is_plumbing_fixture(e) && !no_drain(e)) {
            let units = el.num(&["dfu", "fixture_units", "drainage_fixture_units"]).or_else(|| {
                let v = codes::dfu_of(&el.words());
                assumed += usize::from(v.is_some());
                v
            });
            let Some(units) = units else { continue };
            for pipe in chain(&m, i, &["waste", "drains_to", "drain", "fed_from"]) {
                let e = dfu.entry(pipe).or_insert((0.0, 0));
                e.0 += units;
                e.1 += 1;
            }
        }
        let mut rows = Vec::new();
        let mut n = 0;
        for (pi, (units, fixtures_n)) in &dfu {
            let Some(pipe) = m.get(*pi).filter(|p| p.kind == "pipe" && on(p)) else { continue };
            let role = drain_role(&m, pipe);
            let size = pipe.text("size").and_then(|t| codes::inches(&t));
            let slope = pipe.text("slope").and_then(|t| codes::slope_in_ft(&t));
            let max = size.and_then(|sz| codes::max_dfu(role, sz, slope));
            rows.push(json!({"pipe": pipe.handle().hex(), "tag": pipe.tag(), "role": role.as_str(), "size_in": size, "dfu": r2(*units), "fixtures": fixtures_n, "max_dfu": max}));
            let Some(max) = max else { continue };
            n += 1;
            if *units > max {
                out.add(
                    "drain_dfu",
                    "error",
                    Some(pipe),
                    format!(
                        "{}: {} drainage fixture units on a {}\" {} (at most {})",
                        pipe.label(),
                        r2(*units),
                        size.unwrap_or(0.0),
                        role.as_str(),
                        max
                    ),
                    json!({"dfu": r2(*units), "max_dfu": max, "role": role.as_str()}),
                );
            }
        }
        totals["dfu"] = json!(rows);
        if assumed > 0 {
            totals["dfu_note"] = json!(format!("{assumed} fixture(s) carry no fixture units: IPC Table 709.1's value for their type (assumed)"));
        }
        out.checked("drain_dfu", n);
    }
    if wants(&set, "pipe_slope") {
        let mut n = 0;
        for el in els.iter().filter(|e| e.kind == "pipe" && drain_system(e) && drain_role(&m, e) != DrainRole::Stack) {
            let (Some(slope), Some(size)) = (el.text("slope").and_then(|t| codes::slope_in_ft(&t)), el.text("size").and_then(|t| codes::inches(&t)))
            else {
                continue;
            };
            n += 1;
            let min = codes::min_slope(size);
            if slope + 1e-9 < min {
                out.add(
                    "pipe_slope",
                    "error",
                    Some(el),
                    format!("{}: a {size}\" drain at {} in/ft is under the least slope of {min} in/ft", el.label(), r2(slope * 1000.0) / 1000.0),
                    json!({"slope_in_ft": slope, "min_in_ft": min, "size_in": size}),
                );
            }
        }
        out.checked("pipe_slope", n);
    }
    if wants(&set, "water_heater") {
        let heaters: Vec<(usize, &El)> = m.els.iter().enumerate().filter(|(_, e)| is_water_heater(e) && on(e)).take(MAX_HEATERS).collect();
        out.checked("water_heater", heaters.len());
        for (hi, heater) in &heaters {
            // The fixtures it serves: linked to it (directly or down a chain), else all of them
            // when it is the only heater.
            let served: Vec<&El> = m
                .els
                .iter()
                .enumerate()
                .filter(|(_, e)| is_plumbing_fixture(e) && !is_water_heater(e))
                .filter(|(i, _)| {
                    chain(&m, *i, &["hw_from", "hot_water", "fed_from"]).contains(hi)
                        || heaters.len() == 1 && m.els.get(*i).is_some_and(|e| e.first(&["hw_from", "hot_water"]).is_none())
                })
                .map(|(_, e)| e)
                .collect();
            let demand: f64 =
                served.iter().filter_map(|f| f.num(&["hw_gph"]).or_else(|| codes::hw_gph_of(&f.words()))).sum::<f64>() * lim.hw_demand_factor;
            let recovery = heater
                .num(&["recovery_gph", "gph", "recovery"])
                .or_else(|| heater.text("capacity").filter(|c| c.to_ascii_lowercase().contains("gph")).and_then(|c| codes::lead_num(&c)));
            match recovery {
                None => out.add(
                    "water_heater",
                    "info",
                    Some(heater),
                    format!("{} has no recovery rate (gph) to check against {} gph of demand", heater.label(), r2(demand)),
                    json!({"demand_gph": r2(demand), "fixtures": served.len()}),
                ),
                Some(r) if r < demand => out.add(
                    "water_heater",
                    "warning",
                    Some(heater),
                    format!(
                        "{}: recovery {} gph is under the {} fixture(s)' demand of {} gph (demand factor {})",
                        heater.label(),
                        r2(r),
                        served.len(),
                        r2(demand),
                        lim.hw_demand_factor
                    ),
                    json!({"recovery_gph": r, "demand_gph": r2(demand), "fixtures": served.len()}),
                ),
                _ => {}
            }
        }
    }
    if wants(&set, "gas_size") {
        // Each appliance's load goes up its chain of gas pipes; each pipe's length is the
        // longest path through it from an appliance to the meter (the branch-length method).
        let mut load: BTreeMap<usize, (f64, f64)> = BTreeMap::new();
        for (i, el) in m.els.iter().enumerate() {
            let Some(btuh) = el.num(&["btuh", "gas_load", "input_btuh"]).or_else(|| el.num(&["mbh"]).map(|v| v * 1000.0)) else { continue };
            if el.kind == "pipe" {
                continue;
            }
            let path: Vec<usize> = chain(&m, i, &["gas_from", "fed_from"])
                .into_iter()
                .filter(|p| m.get(*p).is_some_and(|e| e.kind == "pipe" && gas_system(e)))
                .collect();
            let total: f64 = path.iter().filter_map(|p| m.get(*p)).map(|e| m.length_ft(e)).sum();
            for pi in path {
                let e = load.entry(pi).or_insert((0.0, 0.0));
                e.0 += btuh;
                e.1 = e.1.max(total);
            }
        }
        let mut rows = Vec::new();
        let mut n = 0;
        for (pi, (btuh, len)) in &load {
            let Some(pipe) = m.get(*pi).filter(|p| on(p)) else { continue };
            let cfh = btuh / lim.gas_btu_per_cf;
            let size = pipe.text("size").and_then(|t| codes::inches(&t));
            let cap = size.and_then(|s| codes::gas_cfh(s, *len));
            rows.push(
                json!({"pipe": pipe.handle().hex(), "size_in": size, "btuh": r2(*btuh), "cfh": r2(cfh), "length_ft": r2(*len), "capacity_cfh": cap}),
            );
            let Some(sz) = size else { continue };
            n += 1;
            match cap {
                None if *len > 100.0 => out.add(
                    "gas_size",
                    "info",
                    Some(pipe),
                    format!("{}: the run is {} ft, past the 100 ft the check's table covers", pipe.label(), r2(*len)),
                    json!({"length_ft": r2(*len)}),
                ),
                None => out.add(
                    "gas_size",
                    "info",
                    Some(pipe),
                    format!("{}: no capacity for {sz}\" pipe in the check's table", pipe.label()),
                    Value::Null,
                ),
                Some(c) if cfh > c => out.add(
                    "gas_size",
                    "error",
                    Some(pipe),
                    format!("{}: {} cfh ({} Btu/h) on {sz}\" pipe at {} ft (capacity {} cfh)", pipe.label(), r2(cfh), r2(*btuh), r2(*len), c),
                    json!({"cfh": r2(cfh), "capacity_cfh": c, "length_ft": r2(*len), "size_in": sz}),
                ),
                _ => {}
            }
        }
        totals["gas"] = json!(rows);
        out.checked("gas_size", n);
    }

    let total: usize = out.counts.values().sum();
    let empty = m.els.is_empty();
    let mut r = json!({
        "findings": out.findings,
        "count": total,
        "counts": out.counts,
        "checked": out.checked,
        "totals": totals,
        "limits": lim.json(),
    });
    if empty {
        r["next_step"] = json!("the model has no elements yet: build it with model.build or bim.build");
    }
    if total > 0 {
        s.echo(format!("Property checks: {total} finding(s)."));
    }
    Ok(r)
}
