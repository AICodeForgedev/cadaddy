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
    /// A wireway's sizes, for raceways an enclosure's plate can't take (depth, length step, most
    /// length, the share of its side a plate can use) and the rule.
    pub wireway_depths_in: Vec<f64>,
    pub wireway_length_step_in: Option<f64>,
    pub wireway_max_length_in: Option<f64>,
    pub wireway_usable: Option<f64>,
    pub wireway_cite: String,
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
    /// Places a receptacle needs GFCI wherever it is in them, and the words that make a panel
    /// schedule's circuit a receptacle circuit.
    pub places: Vec<Place>,
    pub receptacle_words: Vec<String>,
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
    /// Equipment connections' disconnects: the factor over full-load amps, the standard switch
    /// sizes (amps), the articles, and the words that say the equipment brings its own.
    pub disconnect_factor: Option<f64>,
    pub disconnect_sizes_a: Vec<f64>,
    pub disconnect_cite: String,
    pub unit_disconnect_words: Vec<String>,
}

/// A place by the words its name has (all of them), and the rule's article.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub(crate) struct Place {
    pub all: Vec<String>,
    pub cite: String,
    /// Words a room's name has when it is such a place: where a circuit nothing is drawn on is
    /// built.
    #[serde(default)]
    pub rooms: Vec<String>,
}

/// Whether what a room's receptacles' breakers are known to be says all are GFCI: one at least
/// known, none known not to be.
fn all_under_gfci(breakers: &[Option<bool>]) -> bool {
    breakers.iter().any(|b| *b == Some(true)) && !breakers.iter().any(|b| *b == Some(false))
}

/// A spot for something the drawings don't place: in the first room whose name has one of `words`,
/// on the lowest level with one, along its longest wall (a quarter of the way for the first, on
/// from there for the next, `used` keeping count): its level, the room's name and the point.
pub(crate) fn spot_in_room(
    words: &[String],
    rooms: &[(String, String, Vec<cadaddy_geom::Vec2>)],
    levels: &[String],
    used: &mut BTreeMap<usize, usize>,
) -> Option<(String, String, cadaddy_geom::Vec2)> {
    let same_level = |a: &str, b: &str| {
        let (a, b) = (a.trim().to_ascii_uppercase(), b.trim().to_ascii_uppercase());
        a == b || a.ends_with(&format!(" {b}")) || b.ends_with(&format!(" {a}"))
    };
    let names_it = |name: &str| {
        let up = name.to_ascii_uppercase();
        up.split(|ch: char| !ch.is_ascii_alphanumeric()).any(|w| words.iter().any(|r| r == w))
    };
    let (i, level) =
        levels.iter().find_map(|l| rooms.iter().position(|(n, rl, o)| same_level(rl, l) && o.len() >= 3 && names_it(n)).map(|i| (i, l.clone())))?;
    let (name, _, outline) = rooms.get(i)?;
    let edges = outline.iter().zip(outline.iter().cycle().skip(1));
    let (a, b) = edges.max_by(|x, y| x.0.dist(*x.1).total_cmp(&y.0.dist(*y.1)))?;
    let nth = used.entry(i).or_insert(0);
    let t = (*nth as f64 + 1.0) / 4.0;
    *nth += 1;
    Some((level, name.clone(), *a + (*b - *a) * t.min(0.9)))
}

/// The receptacle circuits a panel schedule calls for in a place the code names (`RCPT - ELEVATOR
/// CAB`) with no receptacle on any plan: the schedule says it is there, the plans are silent, so
/// it is built in a room of that place, on its longest wall, from the schedule and the code, its
/// basis stored on it (GFCI unless its breaker is). `rooms`: name, level, outline (model units).
/// What was built, a line each, and the circuits with no room to put them in.
pub(crate) fn silent_place_receptacles(
    sp: &mut cadaddy_walk3d::BuildingSpec,
    rooms: &[(String, String, Vec<cadaddy_geom::Vec2>)],
) -> (Vec<String>, Vec<String>) {
    let Some(gf) = rules().into_iter().find(|r| r.sets == "gfci") else { return (Vec::new(), Vec::new()) };
    let upf = if sp.units_per_foot.is_finite() && sp.units_per_foot > 0.0 { sp.units_per_foot } else { 12.0 };
    let on_plans: BTreeSet<String> = sp
        .tags
        .iter()
        .filter(|t| t.kind == "receptacle")
        .filter_map(|t| t.fields.get("circuit"))
        .map(|f| f.value.split(',').next().unwrap_or_default().trim().to_ascii_uppercase())
        .collect();
    let levels: Vec<String> = sp.levels.iter().map(|l| l.name.clone()).collect();
    let (mut built, mut no_room) = (Vec::new(), Vec::new());
    let mut new_tags = Vec::new();
    let mut used: BTreeMap<usize, usize> = BTreeMap::new();
    for p in &sp.mep.panel_schedules {
        for c in &p.circuits {
            let up = c.description.to_ascii_uppercase();
            let receptacles = up.split(|ch: char| !ch.is_ascii_alphanumeric()).any(|w| gf.receptacle_words.iter().any(|r| r == w));
            let Some(place) = place_of(&gf.places, &c.description).filter(|_| receptacles) else { continue };
            let circuit = format!("{}-{}", p.panel.trim(), c.number.trim());
            if place.rooms.is_empty() || on_plans.contains(&circuit.to_ascii_uppercase()) {
                continue;
            }
            let Some((level, name, at)) = spot_in_room(&place.rooms, rooms, &levels, &mut used) else {
                no_room.push(format!("{circuit} {}", c.description.trim()));
                continue;
            };
            let breaker_gf = c.protection.iter().any(|code| gf.tags.iter().any(|t| t.eq_ignore_ascii_case(code)));
            let (mark, item) = if breaker_gf { ("REC", "duplex receptacle") } else { ("GFCI", "GFCI receptacle") };
            let source = if p.source.trim().is_empty() { "panel schedule".to_string() } else { p.source.trim().to_string() };
            let mut tag = cadaddy_walk3d::tags::ElementTag {
                id: format!("device:{level}:{:.1},{:.1}", at.x / upf, at.y / upf),
                kind: "receptacle".into(),
                level: level.clone(),
                at_ft: [at.x / upf, at.y / upf],
                ..Default::default()
            };
            tag.set("type", mark, "rule");
            tag.set("device_type", item, "rule");
            tag.set("circuit", circuit.clone(), "schedule");
            tag.set("sheet", source.clone(), "schedule");
            tag.set("symbol", "0", "rule");
            tag.set("design_value", "no receptacle drawn", "plan");
            tag.set("corrected_value", item, "rule");
            tag.set(
                "correction_reason",
                format!("{source} panel {} circuit {} calls for {} but no plan draws it", p.panel.trim(), c.number.trim(), c.description.trim()),
                "rule",
            );
            tag.set("correction_rule", place.cite.clone(), "rule");
            built.push(format!("{circuit} {}: {item} in {name} on {level} ({})", c.description.trim(), place.cite));
            new_tags.push(tag);
        }
    }
    sp.tags.extend(new_tags);
    (built, no_room)
}

/// The place a name is in (`RCPT - ELEVATOR PIT` → elevator pit), if any.
pub(crate) fn place_of<'a>(places: &'a [Place], name: &str) -> Option<&'a Place> {
    let up = name.to_ascii_uppercase();
    let words: BTreeSet<&str> = up.split(|c: char| !c.is_ascii_alphanumeric()).filter(|w| !w.is_empty()).collect();
    places.iter().find(|p| !p.all.is_empty() && p.all.iter().all(|w| words.contains(w.as_str())))
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
        .params("{} → {receptacles, tamper_resistant: n (in guest rooms / dwellings: NEC 406.12), tr_documented: {stated, not_stated}, gfci: {device, load_side, breaker, double_protection_avoided, none, undecided}, gfci_circuits: [{panel, circuit, description, rule, needs}] (the panel schedules' receptacle circuits in a place needing GFCI, an elevator pit, a roof, whose breaker isn't GFCI), rules: [id]}: each receptacle gets `tamper_resistant` (yes, with the rule and its room) and `tr_documented` (where the documents say it, or that they don't); and by its circuit (its breaker's type from the panel schedule, bim.props.fill's `gfci_breaker`) `gfci` (device | load side | breaker | none | undecided), `receptacle_type` (GFCI | standard) and `protected_by`, each with its source; a person's values stay")
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
    // A receptacle with no circuit printed, in a room whose other receptacles are all under GFCI
    // breakers: the same. (Not a guess at its circuit: any of them would protect it.)
    let mut in_room: BTreeMap<String, Vec<Option<bool>>> = BTreeMap::new();
    // (A two-pole's breaker says nothing of the room's 120 V receptacles: a PTAC's.)
    let two_pole = |r: &Rec| r.circuit.as_ref().is_some_and(|c| c.1.contains(','));
    for r in recs.iter().filter(|r| !r.room.trim().is_empty() && !two_pole(r)) {
        in_room.entry(r.room.trim().to_ascii_uppercase()).or_default().push(r.gfci_breaker);
    }
    let src_of: BTreeMap<String, Source> = recs
        .iter()
        .filter(|r| r.gfci_breaker == Some(true))
        .filter_map(|r| r.breaker_src.clone().map(|s| (r.room.trim().to_ascii_uppercase(), s)))
        .collect();
    for r in recs.iter_mut().filter(|r| r.gfci_breaker.is_none() && r.circuit.is_none()) {
        let room = r.room.trim().to_ascii_uppercase();
        if in_room.get(&room).is_some_and(|others| all_under_gfci(others)) {
            r.gfci_breaker = Some(true);
            r.breaker_src = src_of.get(&room).cloned();
        }
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
    // In a place that needs GFCI (its circuit's description, else its room's name).
    let circuit_desc = |panel: &str, number: &str| -> String {
        sp.as_ref()
            .and_then(|sp| {
                sp.mep
                    .panel_schedules
                    .iter()
                    .find(|p| p.panel.eq_ignore_ascii_case(panel))
                    .and_then(|p| p.circuits.iter().find(|c| c.number == number))
            })
            .map(|c| c.description.clone())
            .unwrap_or_default()
    };
    for r in recs.iter_mut().filter(|r| !r.tagged && r.needs.is_none()) {
        let desc = circuit_desc(&text(&r.store, "panel"), &text(&r.store, "circuit"));
        let room_name = names.get(&r.room.trim().to_ascii_uppercase()).cloned().unwrap_or_default();
        if let Some(pl) = place_of(&gf.places, &desc) {
            r.needs = Some(format!("{}: its circuit is {desc}", pl.cite));
        } else if let Some(pl) = place_of(&gf.places, &room_name) {
            r.needs = Some(format!("{}: in room {} ({room_name})", pl.cite, r.room));
        }
    }
    // The schedules' receptacle circuits in such a place whose breaker isn't GFCI: their first
    // receptacle is a GFCI device (modelled or not).
    let mut gfci_circuits: Vec<Value> = Vec::new();
    for p in sp.as_ref().map(|sp| sp.mep.panel_schedules.as_slice()).unwrap_or_default() {
        for c in &p.circuits {
            let up = c.description.to_ascii_uppercase();
            let receptacles = up.split(|ch: char| !ch.is_ascii_alphanumeric()).any(|w| gf.receptacle_words.iter().any(|r| r == w));
            let Some(pl) = place_of(&gf.places, &c.description).filter(|_| receptacles) else { continue };
            let breaker_gf = c.protection.iter().any(|code| gf.tags.iter().any(|t| t.eq_ignore_ascii_case(code)));
            // One the model has decides for itself below; this is what it has none of.
            let built = recs.iter().any(|r| {
                text(&r.store, "panel").eq_ignore_ascii_case(p.panel.trim())
                    && text(&r.store, "circuit").rsplit(['-', '/', '.']).next().is_some_and(|n| n.trim() == c.number.trim())
            });
            if !breaker_gf && !built {
                gfci_circuits.push(json!({
                    "panel": p.panel, "circuit": c.number, "description": c.description, "rule": pl.cite,
                    "needs": "a GFCI receptacle (the first on the circuit): its breaker isn't GFCI",
                }));
            }
        }
    }
    let mut tally: BTreeMap<&str, usize> = BTreeMap::new();
    let mut corrections = 0usize;
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
            // A person's edit stays; the rule run again from the same word (breakers as built) doesn't.
            let keep = store.get(key).is_some_and(|old| old.source.kind == SourceKind::User && old.source != source);
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
        // What the drawing says, and what is built instead of it: side by side, never silently.
        let decided = store.get("receptacle_type").filter(|p| p.has_value()).map(Prop::text).unwrap_or_default();
        let printed = if r.tagged { "GFCI" } else { "standard" };
        if !decided.is_empty() && !decided.eq_ignore_ascii_case(printed) {
            let why = store.get("receptacle_type").map(|p| p.source.clone()).unwrap_or_else(Source::computed);
            let reason = store.get("gfci").and_then(|p| p.note.clone()).or_else(|| why.doc.clone()).unwrap_or_default();
            let mut put = |k: &str, v: String| {
                if store.get(k).is_none_or(|old| old.source.kind != SourceKind::User) {
                    store.set(k, Prop::new(json!(v), why.clone()));
                }
            };
            put("design_value", printed.to_string());
            put("corrected_value", decided);
            put("correction_reason", reason);
            put("correction_rule", gf.cite.clone());
            corrections += 1;
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
    tags_follow_decisions(s)?;
    Ok(json!({
        "receptacles": recs.len(),
        "elements": changed,
        "tamper_resistant": tr_count,
        "tr_documented": {"stated": stated, "not_stated": not_stated},
        "gfci": tally,
        "corrections": corrections,
        "gfci_circuits": gfci_circuits,
        "rules": all.iter().map(|r| r.id.clone()).collect::<Vec<_>>(),
    }))
}

/// The item of a receptacle's bill-of-materials line after the code rules' decisions: GFCI or
/// standard by its circuit (what the print tagged it, or a sink nearby, only asks), and
/// tamper-resistant where the rule says. An item that names no receptacle of these stays.
fn decided_item(item: &str, receptacle_type: &str, tamper_resistant: bool) -> String {
    const GFCI: &str = "GFCI receptacle";
    const STANDARD: &str = "duplex receptacle";
    let rest = item.strip_prefix(GFCI).or_else(|| item.strip_prefix(STANDARD));
    let Some(rest) = rest else { return item.to_string() };
    let base = match receptacle_type {
        t if t.eq_ignore_ascii_case("GFCI") => GFCI,
        t if t.eq_ignore_ascii_case("standard") => STANDARD,
        _ => item.strip_suffix(rest).unwrap_or(item),
    };
    let tr = if tamper_resistant && !rest.contains("tamper-resistant") { ", tamper-resistant" } else { "" };
    format!("{base}{rest}{tr}")
}

/// The take-off reads the building spec's device tags: they carry each built receptacle's decision
/// too, so a GFCI breaker's receptacles aren't bought as GFCI devices.
fn tags_follow_decisions(s: &mut Session) -> Result<()> {
    let Ok((mut sp, true)) = super::bspec::current(s, CMD) else { return Ok(()) };
    let mut decided: BTreeMap<String, (String, bool, String)> = BTreeMap::new();
    for e in s.doc()?.model.iter() {
        let Some(id) = super::bimlook::made_tag(e, super::propfill::DEVID) else { continue };
        let store = props::read(&e.common.xdata);
        let value = |k: &str| store.get(k).filter(|p| p.has_value()).map(Prop::text).unwrap_or_default();
        let gfci = value("gfci");
        let tr = value("tamper_resistant").eq_ignore_ascii_case("yes");
        if !gfci.is_empty() || tr {
            decided.insert(id.to_string(), (value("receptacle_type"), tr, gfci));
        }
    }
    let mut changed = false;
    for t in sp.tags.iter_mut().filter(|t| t.kind == "receptacle") {
        let Some((kind, tr, gfci)) = decided.get(&t.id) else { continue };
        let Some(item) = t.fields.get("device_type").map(|f| f.value.clone()) else { continue };
        let next = decided_item(&item, kind, *tr);
        if next != item {
            t.set("device_type", next, "rule");
            changed = true;
        }
        if !gfci.is_empty() && t.fields.get("gfci").is_none_or(|f| f.value != *gfci) {
            t.set("gfci", gfci.clone(), "rule");
            changed = true;
        }
    }
    if changed {
        s.doc_mut()?.building_spec = Some(sp.to_json());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A schedule's receptacle circuit in a code place that no plan draws is built in such a room,
    /// its basis on it; one the plans have, or with no such room, isn't.
    #[test]
    fn a_receptacle_a_schedule_calls_for_and_no_plan_draws_is_built() {
        let v = |x: f64, y: f64| cadaddy_geom::Vec2::new(x, y);
        let mut sp = cadaddy_walk3d::BuildingSpec::from_json(
            r#"{"version": 1, "units_per_foot": 12, "levels": [{"name": "Level 1", "elevation_ft": 0, "height_ft": 10, "ceiling_ft": 9, "base": [0, 0]}]}"#,
        )
        .unwrap_or_else(|e| panic!("{e}"));
        let ckt = |n: &str, d: &str, codes: &[&str]| cadaddy_walk3d::PanelCircuit {
            number: n.into(),
            description: d.into(),
            protection: codes.iter().map(|c| (*c).to_string()).collect(),
            ..Default::default()
        };
        sp.mep.panel_schedules.push(cadaddy_walk3d::PanelSchedule {
            panel: "E".into(),
            source: "E-2".into(),
            circuits: vec![
                ckt("7", "RCPT - ELEVATOR CAB", &[]),
                ckt("9", "RCPT - ELEVATOR SHAFT", &["GF"]),
                ckt("11", "RCPT - PUBLIC RESTROOM", &[]),
                ckt("13", "RCPT - ROOF", &[]),
                ckt("15", "LTG - ELEVATOR PIT", &[]),
            ],
            ..Default::default()
        });
        let rooms = vec![("ELEV.".to_string(), "1".to_string(), vec![v(0.0, 0.0), v(120.0, 0.0), v(120.0, 60.0), v(0.0, 60.0)])];
        let (built, no_room) = silent_place_receptacles(&mut sp, &rooms);
        assert_eq!(built.len(), 2, "{built:?}");
        assert_eq!(no_room, ["E-11 RCPT - PUBLIC RESTROOM"]);
        let t = |c: &str| sp.tags.iter().find(|t| t.fields.get("circuit").is_some_and(|f| f.value == c)).expect("built");
        assert_eq!(t("E-7").fields["device_type"].value, "GFCI receptacle");
        assert_eq!(t("E-7").fields["design_value"].value, "no receptacle drawn");
        assert!(t("E-7").fields["correction_rule"].value.contains("620.6"));
        // Under a GFCI breaker it is a standard one; two in a room don't share a spot.
        assert_eq!(t("E-9").fields["device_type"].value, "duplex receptacle");
        assert_ne!(t("E-7").id, t("E-9").id);
        // The plans have it now: built once.
        let (again, _) = silent_place_receptacles(&mut sp, &rooms);
        assert!(again.is_empty(), "{again:?}");
    }

    #[test]
    fn a_room_all_under_gfci_breakers_protects_the_one_with_no_circuit_printed() {
        assert!(all_under_gfci(&[Some(true), Some(true), None]));
        assert!(!all_under_gfci(&[Some(true), Some(false), None]), "one on a plain breaker");
        assert!(!all_under_gfci(&[None, None]), "nothing known");
    }

    #[test]
    fn a_receptacles_item_follows_its_decisions() {
        // Under a GFCI breaker a GFCI-tagged receptacle is bought standard; what else it is stays.
        assert_eq!(decided_item("GFCI receptacle", "standard", false), "duplex receptacle");
        assert_eq!(decided_item("GFCI receptacle, weatherproof", "standard", true), "duplex receptacle, weatherproof, tamper-resistant");
        assert_eq!(decided_item("duplex receptacle", "GFCI", true), "GFCI receptacle, tamper-resistant");
        // Tamper-resistant once; undecided keeps the print's.
        assert_eq!(decided_item("duplex receptacle, tamper-resistant", "", true), "duplex receptacle, tamper-resistant");
        assert_eq!(decided_item("GFCI receptacle", "", false), "GFCI receptacle");
        // Not one of these: left alone.
        assert_eq!(decided_item("quad receptacle", "standard", true), "quad receptacle");
    }

    #[test]
    fn the_rules_file_reads() {
        let r = rules();
        assert!(r.iter().any(|r| r.sets == "tamper_resistant" && r.cite == "NEC 406.12" && r.rooms.unit_rooms));
        assert!(r.iter().any(|r| r.sets == "gfci" && r.tags.iter().any(|t| t == "GFCI")));
    }

    #[test]
    fn an_elevator_pit_or_a_roof_is_a_place_needing_gfci_but_a_kitchen_is_not() {
        let gf = rules().into_iter().find(|r| r.sets == "gfci").unwrap_or_default();
        assert!(place_of(&gf.places, "RCPT - ELEVATOR PIT").is_some_and(|p| p.cite.contains("620")));
        assert!(place_of(&gf.places, "RCPT - ELEVATOR CAB").is_some());
        assert!(place_of(&gf.places, "RCPT - ROOF").is_some_and(|p| p.cite.contains("210.8")));
        assert!(place_of(&gf.places, "KITCHEN").is_none());
        assert!(place_of(&gf.places, "PITCH ELEVATOR").is_none(), "whole words");
        assert!(gf.receptacle_words.iter().any(|w| w == "RCPT"));
    }
}
