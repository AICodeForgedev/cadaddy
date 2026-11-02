//! `bim.coderules`: the code rules the build applies to the model's devices on its first run
//! (Validate checks them again from the same data, `coderules.json`).
//! - Tamper-resistant receptacles (NEC 406.12): every receptacle in a guest room, guest suite or
//!   dwelling unit, GFCI ones included, and whether the documents say so themselves.
//! - GFCI by circuit (NEC 210.8): under a GFCI breaker a receptacle is a standard device (a GFCI
//!   one there would be double protection); where the print tags GFCI and the breaker isn't, the
//!   first device on the circuit is the GFCI and those after it are on its load side. Never a
//!   blanket conversion; a breaker of unknown type leaves the decision `undecided`.
//!
//! Every value carries its source (the rule, the room, the schedule row); a person's values stay.

use std::collections::{BTreeMap, BTreeSet};

use cadaddy_doc::Handle;
use cadaddy_doc::props::{self, Prop, Source, SourceKind};

use super::propfill::WithNote;
use serde::Deserialize;
use serde_json::{Value, json};

use super::*;
use crate::{Result, Session};

const CMD: &str = "bim.coderules";
/// Elements looked at, at most.
const MAX_ELEMENTS: usize = 500_000;
/// The rules (shared with Validate).
const RULES: &str = include_str!("coderules.json");
/// Half a sink's width: a fixture's location is its middle, the reach is from its edge.
const SINK_HALF_FT: f64 = 1.0;
/// Two elements whose heights differ by no more than this are on one storey (a level not stored).
const SAME_STOREY_FT: f64 = 8.0;

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
struct RuleFile {
    rules: Vec<CodeRule>,
}

/// One code rule as data.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub(crate) struct CodeRule {
    pub id: String,
    pub cite: String,
    pub kinds: Vec<String>,
    pub sets: String,
    pub rule: String,
    pub rooms: RoomScope,
    pub tags: Vec<String>,
    /// GFCI where a receptacle is within this far of a sink's edge (NEC 210.8), feet.
    pub sink_reach_ft: Option<f64>,
    /// The words that name a sink (a fixture's type, description or mark).
    pub sink_words: Vec<String>,
    /// Working space (NEC 110.26): rows of [volts to ground up to, depth for condition 1, 2, 3]
    /// inches; its least width and its height; where a panel's top is mounted; an entry plate's
    /// margins inside the box (sides, front and back) when its usable size isn't given.
    pub depth_in: Vec<[f64; 4]>,
    pub width_in: Option<f64>,
    pub height_in: Option<f64>,
    pub mount_top_in: Option<f64>,
    pub plate_margin_in: Option<[f64; 2]>,
    /// Equipment connections: the conductor factor over nameplate / full-load amps, and the
    /// ampacities by material and size (75 C column).
    pub conductor_factor: Option<f64>,
    pub ampacity_75c: BTreeMap<String, BTreeMap<String, f64>>,
}

/// The rooms a rule applies in: a typical unit's rooms (guest rooms), or rooms named so.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub(crate) struct RoomScope {
    pub unit_rooms: bool,
    pub name_words: Vec<String>,
}

/// The code rules (`coderules.json`).
pub(crate) fn rules() -> Vec<CodeRule> {
    serde_json::from_str::<RuleFile>(RULES).map(|f| f.rules).unwrap_or_default()
}

pub fn specs() -> Vec<CommandSpec> {
    vec![CommandSpec::new(CMD, "Apply Code Rules", run)
        .params("{} → {receptacles, tamper_resistant: n (in guest rooms / dwellings: NEC 406.12), tr_documented: {stated, not_stated}, gfci: {device, load_side, breaker, double_protection_avoided, none, undecided}, rules: [id]}: each receptacle gets `tamper_resistant` (yes, with the rule and its room) and `tr_documented` (where the documents say it, or that they don't); and by its circuit (its breaker's type from the panel schedule, bim.props.fill's `gfci_breaker`) `gfci` (device | load side | breaker | none | undecided), `receptacle_type` (GFCI | standard) and `protected_by`, each with its source; a person's values stay")
        .menu(&["Tools", "Building", "Apply Code Rules"])]
}

/// A receptacle and what its rules need.
struct Rec {
    handle: Handle,
    store: props::Props,
    tagged: bool,
    /// Why its location needs GFCI though the print doesn't tag it (beside a sink).
    needs: Option<String>,
    level: String,
    /// Its breaker is GFCI (`Some(true)`), isn't (`Some(false)`), or not known.
    gfci_breaker: Option<bool>,
    breaker_src: Option<Source>,
    circuit: Option<(String, String)>,
    at: Option<[f64; 3]>,
    room: String,
}

fn text(store: &props::Props, key: &str) -> String {
    store.get(key).filter(|p| p.has_value()).map(|p| p.text().trim().to_string()).unwrap_or_default()
}

fn xyz(v: &str) -> Option<[f64; 3]> {
    let n: Vec<f64> = v.split(',').filter_map(|p| p.trim().parse::<f64>().ok()).filter(|x| x.is_finite()).collect();
    match n.as_slice() {
        [x, y, z] => Some([*x, *y, *z]),
        [x, y] => Some([*x, *y, 0.0]),
        _ => None,
    }
}

fn run(s: &mut Session, _p: &Value) -> Result<Value> {
    let all = rules();
    let tr = all.iter().find(|r| r.sets == "tamper_resistant").cloned().unwrap_or_default();
    let gf = all.iter().find(|r| r.sets == "gfci").cloned().unwrap_or_default();
    let sp = super::bspec::current(s, CMD).ok().filter(|(_, stored)| *stored).map(|(sp, _)| sp);
    // Guest rooms: the rooms a typical unit's layout was fitted into, and rooms named so.
    let mut unit_rooms: BTreeMap<String, String> = BTreeMap::new();
    let mut names: BTreeMap<String, String> = BTreeMap::new();
    if let Some(sp) = &sp {
        for i in sp.unit_instances.as_array().into_iter().flatten() {
            if let Some(n) = i.get("number").and_then(Value::as_str) {
                unit_rooms.insert(n.trim().to_ascii_uppercase(), i.get("type").and_then(Value::as_str).unwrap_or_default().to_string());
            }
        }
        for r in &sp.rooms {
            if let Some(n) = &r.number {
                names.insert(n.trim().to_ascii_uppercase(), r.name.to_ascii_uppercase());
            }
        }
    }
    let guest = |room: &str| -> Option<String> {
        let key = room.trim().to_ascii_uppercase();
        if key.is_empty() {
            return None;
        }
        if tr.rooms.unit_rooms
            && let Some(t) = unit_rooms.get(&key)
        {
            return Some(format!("room {room} is a guest room (unit type {t})"));
        }
        let name = names.get(&key).cloned().unwrap_or_else(|| key.clone());
        tr.rooms.name_words.iter().find(|w| name.contains(w.as_str())).map(|w| format!("room {room} ({name}) is a guest room or dwelling ({w})"))
    };
    let upf = sp.as_ref().map(|sp| sp.units_per_foot).filter(|u| u.is_finite() && *u > 0.0).unwrap_or(12.0);
    let d = s.doc()?;
    // Where each panel element is (the first device on a circuit is the one nearest it).
    let mut panel_at: BTreeMap<String, [f64; 3]> = BTreeMap::new();
    let mut recs: Vec<Rec> = Vec::new();
    // Sinks: (where, level, handle).
    let mut sinks: Vec<([f64; 3], String, Handle)> = Vec::new();
    for (e, base) in super::elemprops::elements(d).take(MAX_ELEMENTS) {
        let store = props::read(&e.common.xdata);
        if base == "fixture" || base == "plumbing_fixture" {
            let name = format!("{} {} {} {}", text(&store, "type_mark"), text(&store, "type"), text(&store, "description"), text(&store, "name"))
                .to_ascii_uppercase();
            let words: BTreeSet<&str> = name.split(|c: char| !c.is_ascii_alphanumeric()).collect();
            if gf.sink_words.iter().any(|w| words.contains(w.as_str()))
                && let Some(at) = xyz(&text(&store, "location"))
            {
                sinks.push((at, text(&store, "level"), e.handle));
            }
            continue;
        }
        if base == "equipment" || base == "gear" || base == "panel" {
            // Where it is: its location, else its geometry's middle (a room's panel placed by the
            // build carries no location).
            let at = xyz(&text(&store, "location")).or_else(|| {
                super::propfill::extent(d, e)
                    .map(|(b, z0, z1)| [(b.min.x + b.max.x) / 2.0 / upf, (b.min.y + b.max.y) / 2.0 / upf, (z0 + z1) / 2.0 / upf])
            });
            if let Some(at) = at {
                panel_at.insert(e.handle.hex(), at);
            }
            continue;
        }
        if base != "device" {
            continue;
        }
        let kind = super::elemprops::reported_kind(e, base.clone());
        if !tr.kinds.iter().chain(gf.kinds.iter()).any(|k| *k == kind) {
            continue;
        }
        let mark = format!("{} {} {}", text(&store, "type_mark"), text(&store, "type"), text(&store, "description")).to_ascii_uppercase();
        let words: BTreeSet<&str> = mark.split(|c: char| !c.is_ascii_alphanumeric()).collect();
        let tagged = kind == "gfci_receptacle" || gf.tags.iter().any(|t| words.contains(t.as_str()));
        let gb = store.get("gfci_breaker").filter(|p| p.has_value());
        let gfci_breaker = gb.map(|p| p.text().eq_ignore_ascii_case("yes"));
        let breaker_src = gb.map(|p| p.source.clone());
        let fed = text(&store, "fed_from_handle");
        let circuit = text(&store, "circuit");
        let room = text(&store, "room");
        let key = if fed.is_empty() { format!("{}@{room}", text(&store, "panel")) } else { fed };
        recs.push(Rec {
            handle: e.handle,
            at: xyz(&text(&store, "location")),
            level: text(&store, "level"),
            needs: None,
            store,
            tagged,
            gfci_breaker,
            breaker_src,
            circuit: (!circuit.is_empty()).then_some((key, circuit)),
            room,
        });
    }
    // Where the location needs GFCI though the print doesn't tag it: within reach of a sink on its
    // level (a fixture's location is its middle: half a sink's width is added).
    if let Some(reach) = gf.sink_reach_ft {
        for r in recs.iter_mut().filter(|r| !r.tagged) {
            let Some(a) = r.at else { continue };
            let near =
                sinks
                    .iter()
                    // The same level; a level not stored, the same storey by height.
                    .filter(|(p, lvl, _)| {
                        if lvl.is_empty() || r.level.is_empty() { (p[2] - a[2]).abs() <= SAME_STOREY_FT } else { lvl.eq_ignore_ascii_case(&r.level) }
                    })
                    .map(|(p, _, h)| (((a[0] - p[0]).powi(2) + (a[1] - p[1]).powi(2)).sqrt(), *h))
                    .filter(|(d, _)| *d <= reach + SINK_HALF_FT)
                    .min_by(|x, y| x.0.total_cmp(&y.0));
            if let Some((dist, h)) = near {
                r.needs = Some(format!("{dist:.1} ft from sink {} (within {reach} ft of its edge)", h.hex()));
            }
        }
    }
    let mut tally: BTreeMap<&str, usize> = BTreeMap::new();
    let mut tr_count = 0usize;
    let (mut stated, mut not_stated) = (0usize, 0usize);
    let rule_src = |r: &CodeRule, why: &str| Source::from_doc(SourceKind::Rule, &format!("{} {}: {why}", r.cite, r.id));
    // GFCI: the tagged receptacles on a circuit whose breaker isn't GFCI, the first one the GFCI.
    let mut by_circuit: BTreeMap<(String, String), Vec<usize>> = BTreeMap::new();
    for (i, r) in recs.iter().enumerate() {
        if (r.tagged || r.needs.is_some())
            && r.gfci_breaker == Some(false)
            && let Some(c) = &r.circuit
        {
            by_circuit.entry(c.clone()).or_default().push(i);
        }
    }
    let mut first_of: BTreeMap<usize, Option<Handle>> = BTreeMap::new();
    for ((panel, _), ix) in &by_circuit {
        let pat = panel_at.get(panel);
        let dist = |i: &usize| -> Option<f64> {
            let a = recs.get(*i)?.at?;
            let p = pat?;
            Some(((a[0] - p[0]).powi(2) + (a[1] - p[1]).powi(2)).sqrt())
        };
        let ordered: Option<Vec<(f64, usize)>> = ix.iter().map(|i| dist(i).map(|d| (d, *i))).collect();
        match ordered {
            Some(mut o) if !o.is_empty() => {
                o.sort_by(|a, b| a.0.total_cmp(&b.0));
                let first = o.first().map(|f| f.1);
                for (_, i) in &o {
                    first_of.insert(*i, first.filter(|f| f != i).and_then(|f| recs.get(f).map(|r| r.handle)));
                }
            }
            _ if ix.len() == 1 => {
                if let Some(i) = ix.first() {
                    first_of.insert(*i, None);
                }
            }
            // Which is first on the circuit isn't known (no panel or device location).
            _ => {}
        }
    }
    let mut changes: Vec<(Handle, props::Props)> = Vec::new();
    for (i, r) in recs.iter().enumerate() {
        let mut store = r.store.clone();
        let before = store.clone();
        let mut set = |key: &str, value: Value, source: Source, note: Option<&str>| {
            let keep = store.get(key).is_some_and(|old| old.source.kind == SourceKind::User);
            if !keep {
                let p = Prop::new(value, source);
                store.set(key, if let Some(n) = note { p.with_note(n) } else { p });
            }
        };
        // Tamper-resistant: where the documents say it, and the rule in guest rooms.
        let req = text(&r.store, "required_features").to_ascii_uppercase();
        let prod = text(&r.store, "product").to_ascii_uppercase();
        let mark = format!("{} {}", text(&r.store, "type_mark"), text(&r.store, "type")).to_ascii_uppercase();
        let documented = if req.contains("TAMPER") || prod.contains("TAMPER") {
            r.store.get("required_features").filter(|p| p.has_value() && req.contains("TAMPER")).map_or_else(
                || ("the specification (its product record)".to_string(), Source::from_doc(SourceKind::Spec, "product record")),
                |p| (p.source.doc.clone().unwrap_or_else(|| "the specification".into()), p.source.clone()),
            )
        } else if mark.split(|c: char| !c.is_ascii_alphanumeric()).any(|w| w == "TR") {
            ("its type tag (TR)".to_string(), r.store.get("type_mark").map_or_else(Source::computed, |p| p.source.clone()))
        } else {
            ("not stated in the documents".to_string(), Source::computed())
        };
        if documented.0 == "not stated in the documents" {
            not_stated += 1;
        } else {
            stated += 1;
        }
        set("tr_documented", json!(documented.0), documented.1, None);
        if let Some(why) = guest(&r.room) {
            set("tamper_resistant", json!("yes"), rule_src(&tr, &why), Some("every receptacle in a guest room, GFCI included"));
            tr_count += 1;
        }
        // GFCI by circuit.
        let circuit = r.circuit.as_ref().map_or_else(|| "its circuit".to_string(), |c| format!("circuit {}", c.1));
        let breaker_src = r.breaker_src.clone().unwrap_or_else(Source::computed);
        let why = match (&r.needs, r.tagged) {
            (_, true) => "tagged GFCI".to_string(),
            (Some(n), false) => format!("needs GFCI ({n})"),
            (None, false) => String::new(),
        };
        match (r.tagged || r.needs.is_some(), r.gfci_breaker) {
            (needs, Some(true)) => {
                let note = if r.tagged {
                    *tally.entry("double_protection_avoided").or_default() += 1;
                    format!(
                        "the print tags it GFCI, but {circuit}'s breaker is GFCI: a GFCI receptacle there would be double protection, so it is built standard"
                    )
                } else if needs {
                    format!("{why}; {circuit}'s breaker is GFCI, which protects it")
                } else {
                    format!("{circuit}'s breaker is GFCI")
                };
                set("gfci", json!("breaker"), breaker_src.clone(), Some(&note));
                set("receptacle_type", json!("standard"), rule_src(&gf, &note), None);
                set("protected_by", json!(format!("breaker, {circuit}")), breaker_src, None);
                *tally.entry("breaker").or_default() += 1;
            }
            (true, Some(false)) => match first_of.get(&i) {
                Some(None) => {
                    let note = format!("{why} on {circuit}, whose breaker isn't GFCI: the first device on the circuit (nearest its panel)");
                    set("gfci", json!("device"), rule_src(&gf, &note), None);
                    set("receptacle_type", json!("GFCI"), rule_src(&gf, &note), None);
                    *tally.entry("device").or_default() += 1;
                }
                Some(Some(first)) => {
                    let note = format!("{why} on {circuit}: on the load side of the circuit's first GFCI receptacle ({})", first.hex());
                    set("gfci", json!("load side"), rule_src(&gf, &note), None);
                    set("receptacle_type", json!("standard"), rule_src(&gf, &note), None);
                    set("protected_by", json!(format!("GFCI receptacle {}", first.hex())), rule_src(&gf, &note), None);
                    *tally.entry("load_side").or_default() += 1;
                }
                None => {
                    let note =
                        format!("{why} on {circuit}, whose breaker isn't GFCI, with others that need it: which is first on the circuit isn't known");
                    set("gfci", json!("undecided"), rule_src(&gf, &note), None);
                    *tally.entry("undecided").or_default() += 1;
                }
            },
            (true, None) => {
                let note = format!("{why}; {circuit}'s breaker type isn't in the panel schedule");
                set("gfci", json!("undecided"), rule_src(&gf, &note), None);
                *tally.entry("undecided").or_default() += 1;
            }
            (false, Some(false)) => {
                set("gfci", json!("none"), breaker_src, Some("not tagged GFCI; its breaker isn't GFCI"));
                *tally.entry("none").or_default() += 1;
            }
            (false, None) => {}
        }
        if store != before {
            changes.push((r.handle, store));
        }
    }
    let d = s.doc_mut()?;
    let mut changed = 0usize;
    for (h, store) in changes {
        if d.modify_entity(h, |e| e.common.xdata = props::write(&e.common.xdata, &store).into()).is_ok() {
            changed += 1;
        }
    }
    if changed > 0 {
        s.touch();
    }
    Ok(json!({
        "receptacles": recs.len(),
        "elements": changed,
        "tamper_resistant": tr_count,
        "tr_documented": {"stated": stated, "not_stated": not_stated},
        "gfci": tally,
        "rules": all.iter().map(|r| r.id.clone()).collect::<Vec<_>>(),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_rules_file_reads() {
        let r = rules();
        assert!(r.iter().any(|r| r.sets == "tamper_resistant" && r.cite == "NEC 406.12" && r.rooms.unit_rooms));
        assert!(r.iter().any(|r| r.sets == "gfci" && r.tags.iter().any(|t| t == "GFCI")));
    }
}
