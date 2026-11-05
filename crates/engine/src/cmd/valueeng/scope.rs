//! What an alternative would change (Q6154 (a), (b), (d)): the model's routed branch circuits
//! and feeders, each with its area classified from the model's rooms, its pieces as designed
//! (the routed takeoff's) and the pieces the alternative method makes of the same routes.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use cadaddy_doc::{Drawing, Handle};
use cadaddy_geom::{Vec2, Vec3};
use cadaddy_walk3d::{BuildingSpec, MepRun};

use super::super::conduitroute::{BRANCH_SYSTEM, mep_piece};
use super::super::gearbuild::FEEDER_SYSTEM;
use super::super::mepatoms::{self, Atom};
use super::facts::CodeFacts;
use super::price::Line;
use super::{Alternative, Rules};
use cadaddy_doc::props::{self, Prop};

/// Circuits, pieces and runs looked at, at most (hostile models).
const MAX_ITEMS: usize = 20_000;
const MAX_PIECES: usize = 2_000_000;
const MAX_RUNS: usize = 200_000;
/// Why an item isn't weighed: its conductors don't read as conductors.
pub(crate) const NOT_READ: &str = "conductors_not_read";
/// A feeder piece belongs to the feeder run it lies on, within this many feet.
const FEEDER_REACH_FT: f64 = 1.5;

/// Where a circuit or feeder is: its area (`unit`, `common`, `exposed`) and what else its rooms
/// are (`suspended`, `plenum`, `rated_corridor`, `below_slab`), with the rooms.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Area {
    pub base: String,
    pub flags: BTreeSet<String>,
    pub rooms: BTreeSet<String>,
}

/// One branch circuit or feeder as routed.
#[derive(Clone, Debug, Default)]
pub(crate) struct Item {
    /// `branch` or `feeder`.
    pub kind: &'static str,
    pub id: String,
    pub label: String,
    pub level: String,
    pub panel: String,
    /// What it feeds (a feeder: its gear).
    pub to: String,
    pub conductors: String,
    /// Its method as designed (`EMT`) and where it comes from.
    pub method: String,
    pub method_source: String,
    pub area: Area,
    /// Its protection (A), when the schedules give it.
    pub breaker_a: Option<f64>,
    /// The model's pieces as designed.
    pub design: Vec<Handle>,
    /// Its routed runs (spec indices).
    pub runs: Vec<usize>,
    /// A homerun's circuits: label and conductors.
    pub members: Vec<(String, String)>,
    /// A feeder: its index in the spec's feeders.
    pub feeder: Option<usize>,
    /// Rated walls its runs pass through (each a firestop).
    pub rated_penetrations: usize,
    /// Its routed length, feet.
    pub feet: f64,
    /// Its raceway, inches, and the size its conductors need (fill), when larger than needed.
    pub oversized: Option<(f64, f64)>,
}

fn field<'a>(t: &'a cadaddy_walk3d::tags::ElementTag, k: &str) -> &'a str {
    t.fields.get(k).map_or("", |f| f.value.trim())
}

fn has(text: &str, words: &[String]) -> bool {
    let up = text.to_ascii_uppercase();
    words.iter().any(|w| !w.is_empty() && up.contains(w.as_str()))
}

/// A conductor size's area in a raceway (THHN/THWN-2), sq in.
fn wire_area(size: &str) -> Option<f64> {
    super::super::conduitroute::rules().fill.wire_sq_in.get(size.trim().trim_start_matches('#')).copied()
}

/// The smallest raceway (of `kind`) its conductors fill no more than the code's percent of.
fn needed_raceway(kind: &str, conductors: &[(u32, String)]) -> Option<f64> {
    let rl = super::super::conduitroute::rules();
    let n: u32 = conductors.iter().map(|c| c.0).sum();
    let area: f64 = conductors.iter().map(|(k, s)| wire_area(s).map(|a| a * f64::from(*k))).sum::<Option<f64>>()?;
    let pct = match n {
        0 => return None,
        1 => rl.fill.one_pct,
        2 => rl.fill.two_pct,
        _ => rl.fill.over_two_pct,
    } / 100.0;
    let table = rl.fill.area_sq_in.get(kind).or_else(|| rl.fill.area_sq_in.get("EMT"))?;
    table.iter().find(|[_, a]| area <= a * pct + 1e-9).map(|[t, _]| *t)
}

/// A branch circuit's or feeder's breaker (A) from its panel's schedule row.
fn trip(text: &str) -> Option<f64> {
    let digits: String = text.chars().skip_while(|c| !c.is_ascii_digit()).take_while(char::is_ascii_digit).collect();
    digits.parse::<f64>().ok().filter(|a| (5.0..=4000.0).contains(a))
}

fn branch_trip(sp: &BuildingSpec, panel: &str, label: &str) -> Option<f64> {
    let num = super::super::printcond::parse_circuit(label)?.1;
    let first = num.split(',').next()?.trim().to_string();
    let s = sp.mep.panel_schedules.iter().find(|p| p.panel.trim().eq_ignore_ascii_case(panel.trim()))?;
    trip(&s.circuits.iter().find(|c| c.number.trim() == first)?.breaker)
}

/// A feeder's breaker: its source panel's row naming the gear it feeds (or the feeder in its
/// wire column), else the fed panel's main.
fn feeder_trip(sp: &BuildingSpec, f: &cadaddy_walk3d::Feeder) -> Option<(f64, String)> {
    let to = f.to.trim().to_ascii_uppercase();
    let tag = f.tag.trim().to_ascii_uppercase();
    if let Some(s) = sp.mep.panel_schedules.iter().find(|p| p.panel.trim().eq_ignore_ascii_case(f.from.trim())) {
        for c in s.circuits.iter().take(2_000) {
            let words: Vec<String> =
                c.description.split(|ch: char| !(ch.is_ascii_alphanumeric() || ch == '-' || ch == '/')).map(str::to_ascii_uppercase).collect();
            let wire = c.wire.trim().to_ascii_uppercase();
            let by_wire = !tag.is_empty() && wire.strip_prefix("FEEDER ").is_some_and(|w| w.split([':', ' ', ',']).next() == Some(tag.as_str()));
            if ((!to.is_empty() && words.contains(&to)) || by_wire)
                && let Some(a) = trip(&c.breaker)
            {
                return Some((a, format!("panel {} circuit {}: {}", s.panel, c.number, c.breaker)));
            }
        }
    }
    let s = sp.mep.panel_schedules.iter().find(|p| p.panel.trim().eq_ignore_ascii_case(f.to.trim()))?;
    s.header
        .iter()
        .filter(|(k, _)| k.to_ascii_uppercase().contains("MAIN"))
        .find_map(|(k, v)| trip(v).map(|a| (a, format!("panel {} {k}: {v}", s.panel))))
}

/// The rooms of the model by the label its pieces carry (`room`): name, outline, level.
struct RoomInfo {
    name: String,
    centre: Vec2,
    level: String,
}

fn room_infos(d: &Drawing) -> HashMap<String, Vec<RoomInfo>> {
    let mut out: HashMap<String, Vec<RoomInfo>> = HashMap::new();
    for r in super::super::bspec::room_elements(d).into_iter().filter(|r| r.outline.len() >= 3).take(MAX_ITEMS) {
        let label = r.number.clone().filter(|n| !n.trim().is_empty()).unwrap_or_else(|| r.name.trim().to_string());
        let n = r.outline.len() as f64;
        let centre = r.outline.iter().fold(Vec2::new(0.0, 0.0), |s, p| s + *p) * (1.0 / n);
        out.entry(label.to_ascii_uppercase()).or_default().push(RoomInfo { name: r.name.clone(), centre, level: r.level.clone() });
    }
    out
}

/// The ceiling over a plan point: suspended (a grid) and its words (type and description).
fn ceiling_at(sp: &BuildingSpec, level: &str, p: Vec2) -> Option<(bool, String)> {
    let c = sp.rcp.ceilings.iter().take(MAX_ITEMS).find(|c| {
        c.level.eq_ignore_ascii_case(level) && c.outline.len() >= 3 && {
            let o: Vec<Vec2> = c.outline.iter().map(|q| Vec2::new(q[0], q[1])).collect();
            cadaddy_geom::point_in_polygon(&o, p)
        }
    })?;
    let ty = c.ceiling_type.as_deref().and_then(|t| sp.ceiling_types.iter().find(|x| x.name.eq_ignore_ascii_case(t)));
    let grid = c.grid.is_some() || ty.is_some_and(|t| matches!(t.kind, cadaddy_walk3d::CeilingKind::Grid));
    let words = format!("{} {}", c.ceiling_type.clone().unwrap_or_default(), ty.map(|t| t.description.clone()).unwrap_or_default());
    Some((grid, words))
}

/// Whether the code sheet or the wall types give the corridors a rating.
fn corridors_rated(sp: &BuildingSpec, facts: &CodeFacts) -> bool {
    facts.rated.iter().any(|r| r.hours > 0.0 && r.assembly.contains("CORRIDOR"))
        || sp.wall_types.iter().any(|w| {
            w.assembly.as_ref().is_some_and(|a| {
                a.fire_rating.as_deref().is_some_and(|f| !f.trim().is_empty() && !f.trim().starts_with('0'))
                    && (has(&a.description, &["CORRIDOR".to_string()]) || has(&w.name, &["CORRIDOR".to_string()]))
            })
        })
}

/// The pieces' level, room, kind and a point on them.
struct Piece {
    handle: Handle,
    level: String,
    room: String,
    system: String,
    circuit: String,
    /// The circuit tag its run was routed for (the run's id up to its leg: `<tag id>#<leg>`).
    tag: String,
    at: Vec3,
}

fn pieces(d: &Drawing) -> Vec<Piece> {
    let mut out = Vec::new();
    for e in d.model.iter().filter(|e| mep_piece(e)).take(MAX_PIECES) {
        let ps = props::read(&e.common.xdata);
        let g = |k: &str| ps.get(k).map(Prop::text).unwrap_or_default();
        let system = g("system");
        if system.is_empty() {
            continue;
        }
        let at = super::super::mepatoms::build::stick_ends(&ps).map_or_else(
            || {
                let b = cadaddy_doc::entity_bounds(d, e, 0);
                if b.is_empty() {
                    Vec3::new(f64::NAN, f64::NAN, f64::NAN)
                } else {
                    Vec3::new((b.min.x + b.max.x) * 0.5, (b.min.y + b.max.y) * 0.5, 0.0)
                }
            },
            |(a, b)| (a + b) * 0.5,
        );
        let tag = g("run").split('#').next().unwrap_or_default().to_string();
        out.push(Piece {
            handle: e.handle,
            level: super::super::bspec::made_level(e).unwrap_or_default(),
            room: g("room"),
            system,
            circuit: g("circuit"),
            tag,
            at,
        });
    }
    out
}

/// A run's plan length, feet (a riser its height).
fn run_feet(r: &MepRun, upf: f64) -> f64 {
    if let Some(top) = r.top_ft {
        return (top - r.bottoms_ft.first().copied().unwrap_or(top)).abs();
    }
    let plan: f64 = r.points.windows(2).map(|w| if let [a, b] = w { (b[0] - a[0]).hypot(b[1] - a[1]) } else { 0.0 }).sum::<f64>() / upf;
    let vert: f64 = r.bottoms_ft.windows(2).map(|w| if let [a, b] = w { (b - a).abs() } else { 0.0 }).sum();
    if (plan + vert).is_finite() { plan + vert } else { 0.0 }
}

/// Distance from a point to a run's plan path, drawing units.
fn dist_to_run(r: &MepRun, p: Vec2) -> f64 {
    if r.points.len() == 1 {
        return r.points.first().map_or(f64::INFINITY, |q| Vec2::new(q[0], q[1]).dist(p));
    }
    r.points
        .windows(2)
        .filter_map(|w| if let [a, b] = w { Some((Vec2::new(a[0], a[1]), Vec2::new(b[0], b[1]))) } else { None })
        .map(|(a, b)| {
            let ab = b - a;
            let l2 = ab.x * ab.x + ab.y * ab.y;
            let t = if l2 > 1e-12 { (((p - a).x * ab.x + (p - a).y * ab.y) / l2).clamp(0.0, 1.0) } else { 0.0 };
            (a + ab * t).dist(p)
        })
        .fold(f64::INFINITY, f64::min)
}

/// The routed branch circuits and the feeders, classified.
pub(crate) fn items(d: &Drawing, sp: &BuildingSpec, facts: &CodeFacts, rules: &Rules) -> Vec<Item> {
    let upf = if sp.units_per_foot.is_finite() && sp.units_per_foot > 0.0 { sp.units_per_foot } else { 12.0 };
    let rl = super::super::conduitroute::rules();
    let rooms = room_infos(d);
    let all = pieces(d);
    let report = super::super::conduitroute::stored_report(d).unwrap_or_default();
    let rated_corr = corridors_rated(sp, facts);
    let corridor_words = rules.areas.get("rated_corridor").map(|a| a.words.clone()).unwrap_or_default();
    let plenum_words = rules.areas.get("plenum").map(|a| a.words.clone()).unwrap_or_default();
    // Penetrations of rated walls: level, circuit (a homerun's circuits joined), where.
    let mut pens: Vec<(String, String, Vec2)> = Vec::new();
    for p in report["penetrations"].as_array().into_iter().flatten().take(MAX_PIECES) {
        let rated = p["rating"].as_str().is_some_and(|r| !r.trim().is_empty() && !r.trim().starts_with('0'));
        let at = Vec2::new(p["at"][0].as_f64().unwrap_or(f64::NAN), p["at"][1].as_f64().unwrap_or(f64::NAN));
        if rated {
            pens.push((p["level"].as_str().unwrap_or_default().to_ascii_lowercase(), p["circuit"].as_str().unwrap_or_default().to_string(), at));
        }
    }
    // Below the slab on grade: under the floor of the lowest level (a riser through an upper
    // floor is not).
    let lowest = sp
        .levels
        .iter()
        .filter(|l| l.elevation_ft.is_finite())
        .min_by(|a, b| a.elevation_ft.total_cmp(&b.elevation_ft))
        .map(|l| l.name.to_ascii_lowercase());
    let below_slab = |r: &MepRun| lowest.as_deref() == Some(r.level.to_ascii_lowercase().as_str()) && r.bottoms_ft.iter().any(|b| *b < -0.1);
    // The runs of each circuit tag.
    let mut runs_of: HashMap<&str, Vec<usize>> = HashMap::new();
    for (i, r) in sp.mep.runs.iter().enumerate().take(MAX_RUNS).filter(|(_, r)| r.system == BRANCH_SYSTEM) {
        if let Some(c) = super::super::printcond::circuit_of_run(r) {
            runs_of.entry(c).or_default().push(i);
        }
    }
    // Pieces by the circuit tag their run was routed for, else by level and circuit label.
    let mut by_tag: HashMap<String, Vec<usize>> = HashMap::new();
    let mut by_circuit: HashMap<(String, String), Vec<usize>> = HashMap::new();
    for (i, p) in all.iter().enumerate().filter(|(_, p)| p.system == BRANCH_SYSTEM) {
        by_tag.entry(p.tag.clone()).or_default().push(i);
        by_circuit.entry((p.level.to_ascii_lowercase(), p.circuit.clone())).or_default().push(i);
    }
    // A label more than one routed circuit carries (a typical unit's circuit in each unit) is
    // no way to tell their pieces apart.
    let mut label_n: HashMap<(String, String), usize> = HashMap::new();
    for t in sp.tags.iter().filter(|t| t.kind == "circuit" && field(t, "route") == super::super::conduitroute::ROUTED).take(MAX_ITEMS) {
        *label_n.entry((t.level.to_ascii_lowercase(), field(t, "circuit").to_string())).or_default() += 1;
    }
    let routed_tags: Vec<&cadaddy_walk3d::tags::ElementTag> =
        sp.tags.iter().filter(|t| t.kind == "circuit" && field(t, "route") == super::super::conduitroute::ROUTED).take(MAX_ITEMS).collect();
    let is_homerun = |t: &cadaddy_walk3d::tags::ElementTag| t.fields.contains_key("circuits") || t.id.contains(":home:");
    // Each homerun group's circuits (`homerun_group`, the homerun tag's id after `:home:`):
    // label, conductors, area.
    let mut group_members: HashMap<(String, String), Vec<(String, String, String)>> = HashMap::new();
    for t in routed_tags.iter().filter(|t| !is_homerun(t)) {
        let g = field(t, "homerun_group").replace(['/', '#', '@'], "-");
        if !g.is_empty() {
            group_members.entry((t.level.to_ascii_lowercase(), g)).or_default().push((
                field(t, "circuit").to_string(),
                field(t, "conductors").to_string(),
                field(t, "area").to_string(),
            ));
        }
    }
    // The conductors of each circuit label on a level (a homerun's members).
    let mut conductors_of: HashMap<(String, String), String> = HashMap::new();
    for t in sp.tags.iter().filter(|t| t.kind == "circuit").take(MAX_ITEMS) {
        conductors_of.entry((t.level.to_ascii_lowercase(), field(t, "circuit").to_string())).or_insert_with(|| field(t, "conductors").to_string());
    }
    // Each rated penetration to the routed circuit whose runs pass through it (a typical unit's
    // circuit carries one label in every unit).
    let mut pen_of: HashMap<&str, usize> = HashMap::new();
    {
        let mut keyed: HashMap<(String, String), Vec<&str>> = HashMap::new();
        for t in &routed_tags {
            let label = if is_homerun(t) {
                field(t, "circuits").split(", ").map(str::trim).filter(|l| !l.is_empty()).collect::<Vec<_>>().join(",")
            } else {
                field(t, "circuit").to_string()
            };
            keyed.entry((t.level.to_ascii_lowercase(), label)).or_default().push(t.id.as_str());
        }
        for (level, circuit, at) in &pens {
            let Some(cands) = keyed.get(&(level.clone(), circuit.clone())) else { continue };
            let best = if cands.len() == 1 {
                cands.first().copied()
            } else {
                cands
                    .iter()
                    .map(|id| {
                        let d = runs_of
                            .get(id)
                            .into_iter()
                            .flatten()
                            .filter_map(|i| sp.mep.runs.get(*i))
                            .map(|r| dist_to_run(r, *at))
                            .fold(f64::INFINITY, f64::min);
                        (*id, d)
                    })
                    .filter(|(_, d)| d.is_finite())
                    .min_by(|a, b| a.1.total_cmp(&b.1))
                    .map(|(id, _)| id)
            };
            if let Some(id) = best {
                *pen_of.entry(id).or_default() += 1;
            }
        }
    }
    let classify = |level: &str, base: &str, idx: &[usize], below: bool, rated_here: bool| -> Area {
        let mut a = Area { base: base.to_string(), ..Area::default() };
        if below {
            a.flags.insert("below_slab".into());
        }
        let mut ceiling_known = false;
        for &i in idx.iter().take(10_000) {
            let Some(p) = all.get(i) else { continue };
            let at = Vec2::new(p.at.x, p.at.y);
            if let Some((grid, words)) = ceiling_at(sp, level, at) {
                ceiling_known = true;
                if grid {
                    a.flags.insert("suspended".into());
                }
                if has(&words, &plenum_words) {
                    a.flags.insert("plenum".into());
                }
            }
            let mut corridor = sp
                .circulation
                .iter()
                .any(|c| matches!(c.kind, cadaddy_walk3d::CirculationKind::Corridor) && c.level.eq_ignore_ascii_case(level) && c.holds([at.x, at.y]));
            if !p.room.is_empty() {
                a.rooms.insert(p.room.clone());
                let info = rooms.get(&p.room.to_ascii_uppercase()).and_then(|v| v.iter().find(|r| r.level.eq_ignore_ascii_case(level)).or(v.first()));
                if let Some(info) = info {
                    if has(&info.name, &corridor_words) {
                        corridor = true;
                    }
                    if has(&info.name, &plenum_words) {
                        a.flags.insert("plenum".into());
                    }
                    if let Some((grid, words)) = ceiling_at(sp, level, info.centre) {
                        ceiling_known = true;
                        if grid {
                            a.flags.insert("suspended".into());
                        }
                        if has(&words, &plenum_words) {
                            a.flags.insert("plenum".into());
                        }
                    }
                }
            }
            // A corridor is rated where the code sheet or its walls' types say so, or where the
            // run passes through a rated wall from it.
            if corridor && (rated_corr || rated_here) && base != "unit" {
                a.flags.insert("rated_corridor".into());
            }
        }
        // No RCP ceiling and no room finish over the run: whether it is a suspended ceiling's
        // cavity is not documented.
        if !ceiling_known && !idx.is_empty() {
            a.flags.insert("ceiling_unknown".into());
        }
        a
    };
    let mut out: Vec<Item> = Vec::new();
    // -- Branch circuits (and homeruns) as routed. --
    for t in routed_tags.iter().copied() {
        let label = field(t, "circuit").to_string();
        let level = t.level.clone();
        // A homerun's own tag (its circuits listed), not a circuit's (which says where its homerun goes).
        let homerun = is_homerun(t);
        let key = (level.to_ascii_lowercase(), label.clone());
        let idx = match by_tag.get(t.id.as_str()) {
            Some(v) => v.clone(),
            None if label_n.get(&key).copied().unwrap_or(0) <= 1 => by_circuit.get(&key).cloned().unwrap_or_default(),
            None => Vec::new(),
        };
        let runs = runs_of.get(t.id.as_str()).cloned().unwrap_or_default();
        // A tag no run or piece is routed for changes nothing.
        if runs.is_empty() && idx.is_empty() {
            continue;
        }
        let below = runs.iter().filter_map(|i| sp.mep.runs.get(*i)).any(below_slab);
        // A homerun's circuits: the ones that name its group (each unit's own, with its own
        // conductors), else its list (`A-1, A-3,5`: a two-pole label holds a comma).
        let members: Vec<(String, String)> = if homerun {
            let grp = t.id.split(":home:").nth(1).unwrap_or_default();
            match group_members.get(&(level.to_ascii_lowercase(), grp.to_string())) {
                Some(m) if !grp.is_empty() => m.iter().take(12).map(|(l, c, _)| (l.clone(), c.clone())).collect(),
                _ => field(t, "circuits")
                    .split(", ")
                    .map(str::trim)
                    .filter(|l| !l.is_empty())
                    .take(12)
                    .map(|l| (l.to_string(), conductors_of.get(&(level.to_ascii_lowercase(), l.to_string())).cloned().unwrap_or_default()))
                    .collect(),
            }
        } else {
            Vec::new()
        };
        // A homerun's area: its circuits' (a unit's when all are a unit's).
        let area_word = |tag_area: &str| match tag_area {
            "unit" => "unit",
            "exposed" => "exposed",
            _ => "common",
        };
        let base = if homerun {
            let grp = t.id.split(":home:").nth(1).unwrap_or_default();
            let areas: BTreeSet<&str> = match group_members.get(&(level.to_ascii_lowercase(), grp.to_string())) {
                Some(m) if !grp.is_empty() => m.iter().map(|(_, _, a)| area_word(a)).collect(),
                _ => members
                    .iter()
                    .filter_map(|(l, _)| {
                        sp.tags.iter().find(|x| x.kind == "circuit" && x.level.eq_ignore_ascii_case(&level) && field(x, "circuit") == l)
                    })
                    .map(|x| area_word(field(x, "area")))
                    .collect(),
            };
            if areas.len() == 1 {
                areas.into_iter().next().unwrap_or("common")
            } else if areas.contains("exposed") {
                "exposed"
            } else {
                "common"
            }
        } else {
            area_word(field(t, "area"))
        };
        let feet: f64 = runs.iter().filter_map(|i| sp.mep.runs.get(*i)).map(|r| run_feet(r, upf)).sum();
        let pens = pen_of.get(t.id.as_str()).copied().unwrap_or(0);
        let mut area = classify(&level, base, &idx, below, pens > 0);
        if has(field(t, "wiring_method"), &rl.methods.exposed_words) && area.base == "common" {
            area.base = "exposed".into();
        }
        out.push(Item {
            kind: "branch",
            id: t.id.clone(),
            label: label.clone(),
            level: level.clone(),
            panel: field(t, "panel").to_string(),
            to: String::new(),
            conductors: field(t, "conductors").to_string(),
            method: field(t, "conduit").to_string(),
            method_source: t.fields.get("conduit").map(|f| f.source.clone()).unwrap_or_default(),
            area,
            breaker_a: if homerun { None } else { branch_trip(sp, field(t, "panel"), &label) },
            design: idx.iter().filter_map(|i| all.get(*i)).map(|p| p.handle).collect(),
            runs,
            members,
            feeder: None,
            rated_penetrations: pens,
            feet,
            oversized: None,
        });
    }
    // -- Feeders: each one's routed runs (by their source), its pieces by where they lie (a piece
    // under the slab only to a feeder's run under the slab). --
    let feeder_runs: Vec<(usize, &MepRun)> =
        sp.mep.runs.iter().enumerate().take(MAX_RUNS).filter(|(_, r)| super::super::ugplan::is_feeder_run(r)).collect();
    let feeder_pieces: Vec<usize> =
        all.iter().enumerate().filter(|(_, p)| p.system == FEEDER_SYSTEM || p.system == super::super::ugplan::E_UG).map(|(i, _)| i).collect();
    let mut owner: HashMap<usize, usize> = HashMap::new();
    for &pi in &feeder_pieces {
        let Some(p) = all.get(pi) else { continue };
        let at = Vec2::new(p.at.x, p.at.y);
        let best = feeder_runs
            .iter()
            .filter(|(_, r)| r.level.eq_ignore_ascii_case(&p.level) && r.system == p.system)
            .map(|(i, r)| (*i, dist_to_run(r, at)))
            .filter(|(_, dd)| dd.is_finite() && *dd <= FEEDER_REACH_FT * upf)
            .min_by(|a, b| a.1.total_cmp(&b.1));
        if let Some((ri, _)) = best {
            owner.insert(pi, ri);
        }
    }
    let typical: Vec<String> = sp.mep.panel_schedules.iter().filter(|p| p.typical_of.is_some()).map(|p| p.panel.to_ascii_uppercase()).collect();
    let mut used: BTreeSet<usize> = BTreeSet::new();
    for (fi, f) in sp.mep.feeders.iter().enumerate().take(MAX_ITEMS) {
        let head = format!("feeder {} {}→", f.tag, f.from);
        let to_up = f.to.to_ascii_uppercase();
        let fam = format!("{to_up}-");
        // Its routes: the runs whose source names it (a typical panel's copies each have their own).
        let mine: Vec<usize> = feeder_runs
            .iter()
            .filter(|(i, r)| {
                !used.contains(i)
                    && r.source_sheet.strip_prefix(head.as_str()).is_some_and(|rest| {
                        let first = rest.split('→').next().unwrap_or_default().to_ascii_uppercase();
                        first == to_up || first.starts_with(&fam)
                    })
            })
            .map(|(i, _)| *i)
            .collect();
        if mine.is_empty() {
            continue;
        }
        // A typical panel's feeders: each copy's route is one feeder (`sources` of one tag).
        let copies = if typical.contains(&to_up) {
            mine.iter().filter_map(|i| sp.mep.runs.get(*i)).map(|r| r.source_sheet.clone()).collect::<BTreeSet<_>>()
        } else {
            BTreeSet::new()
        };
        let groups: Vec<Vec<usize>> = if copies.len() > 1 {
            copies.iter().map(|src| mine.iter().copied().filter(|i| sp.mep.runs.get(*i).is_some_and(|r| &r.source_sheet == src)).collect()).collect()
        } else {
            vec![mine.clone()]
        };
        for g in groups.into_iter().take(MAX_ITEMS) {
            used.extend(g.iter().copied());
            let level = g.first().and_then(|i| sp.mep.runs.get(*i)).map(|r| r.level.clone()).unwrap_or_default();
            let idx: Vec<usize> = owner.iter().filter(|(_, ri)| g.contains(ri)).map(|(pi, _)| *pi).collect();
            let below = g.iter().filter_map(|i| sp.mep.runs.get(*i)).any(below_slab);
            let feet: f64 = g.iter().filter_map(|i| sp.mep.runs.get(*i)).map(|r| run_feet(r, upf)).sum::<f64>() / f64::from(f.conduits.max(1));
            let dest = g
                .first()
                .and_then(|i| sp.mep.runs.get(*i))
                .map(|r| r.source_sheet.rsplit('→').next().unwrap_or_default().trim().to_string())
                .unwrap_or_default();
            let base = if typical.contains(&to_up) {
                "common"
            } else if has(&f.to, &rl.methods.exposed_rooms) {
                "exposed"
            } else {
                "common"
            };
            let mut area = classify(&level, base, &idx, below, false);
            area.base = base.to_string();
            // No pieces in the model to read its ceilings from: not documented either.
            if idx.is_empty() && !g.is_empty() && !area.flags.contains("suspended") {
                let known = g
                    .iter()
                    .filter_map(|i| sp.mep.runs.get(*i))
                    .flat_map(|r| r.points.iter())
                    .take(2_000)
                    .any(|q| ceiling_at(sp, &level, Vec2::new(q[0], q[1])).is_some());
                if !known {
                    area.flags.insert("ceiling_unknown".into());
                }
            }
            // Fill: the raceway against what its conductors need.
            let sizes = |n: u32, s: &str| (n, super::super::ugsheets::wire_size(s));
            let mut conds = vec![sizes(f.phases, &f.size), sizes(f.neutrals, if f.neutral_size.is_empty() { &f.size } else { &f.neutral_size })];
            if f.grounds > 0 {
                conds.push(sizes(f.grounds, if f.ground_size.is_empty() { &f.size } else { &f.ground_size }));
            }
            let need = needed_raceway("EMT", &conds);
            let oversized = need.filter(|n| f.conduit_in.is_finite() && f.conduit_in > *n + 1e-9).map(|n| (f.conduit_in, n));
            let (breaker, _) = feeder_trip(sp, f).map_or((None, String::new()), |(a, s)| (Some(a), s));
            let stub_pieces: Vec<Handle> =
                all.iter().filter(|p| p.system.eq_ignore_ascii_case(&f.tag) && !f.tag.is_empty()).map(|p| p.handle).collect();
            let mut design: Vec<Handle> = idx.iter().filter_map(|i| all.get(*i)).map(|p| p.handle).collect();
            // A typical panel's stubs belong to its feeders together: each copy its share.
            if copies.len() <= 1 {
                design.extend(stub_pieces);
            }
            out.push(Item {
                kind: "feeder",
                id: format!("feeder {} {}→{}", f.tag, f.from, dest),
                label: f.tag.clone(),
                level,
                panel: f.from.clone(),
                to: if dest.is_empty() { f.to.clone() } else { dest },
                conductors: feeder_makeup(f),
                method: rl.methods.raceway.clone(),
                method_source: if f.source.is_empty() { "the feeder schedule".into() } else { f.source.clone() },
                area,
                breaker_a: breaker,
                design,
                runs: g,
                members: Vec::new(),
                feeder: Some(fi),
                rated_penetrations: 0,
                feet,
                oversized,
            });
        }
    }
    out
}

/// A feeder's conductors per set as text (`3#2/0 AL + 1#4G`).
pub(crate) fn feeder_makeup(f: &cadaddy_walk3d::Feeder) -> String {
    let al = f.material.trim().eq_ignore_ascii_case("al");
    let w = |n: u32, s: &str, g: bool| {
        let size = super::super::ugsheets::wire_size(s);
        let size = size.trim_start_matches('#').split_whitespace().next().unwrap_or_default().to_string();
        format!(
            "{n}#{size}{}",
            if g {
                "G"
            } else if al {
                " AL"
            } else {
                ""
            }
        )
    };
    let mut parts = Vec::new();
    if f.neutral_size.is_empty() || f.neutral_size == f.size || f.neutrals == 0 {
        parts.push(w(f.phases + f.neutrals, &f.size, false));
    } else {
        parts.push(w(f.phases, &f.size, false));
        parts.push(w(f.neutrals, &f.neutral_size, false));
    }
    if f.grounds > 0 {
        parts.push(w(f.grounds, if f.ground_size.is_empty() { &f.size } else { &f.ground_size }, true));
    }
    parts.join(" + ")
}

/// Whether a conductors text reads as conductors (`2#12 + 1#12G`, `3#10, 1#10G in 3/4" EMT`):
/// every part a count, `#` and a size (a schedule's description read as one, `DRYER #1`, isn't).
pub(crate) fn conductors_read(text: &str) -> bool {
    let head = text.split(" in ").next().unwrap_or_default();
    let parts: Vec<&str> = head.split(['+', ',']).map(str::trim).filter(|p| !p.is_empty()).collect();
    !parts.is_empty()
        && parts.iter().all(|p| {
            p.split_once('#').is_some_and(|(n, size)| {
                let n = n.trim();
                !n.is_empty() && n.chars().all(|c| c.is_ascii_digit()) && size.trim_start().starts_with(|c: char| c.is_ascii_digit())
            })
        })
}

/// Whether an alternative takes an item, or why not (the excluded area's name and cite).
pub(crate) fn takes(alt: &Alternative, it: &Item) -> std::result::Result<(), String> {
    if it.kind != alt.circuit {
        return Err("another kind".into());
    }
    // Conductors the documents don't give as conductors: nothing to size a cable from.
    let members_read = it.members.iter().all(|(_, c)| c.is_empty() || conductors_read(c));
    if !conductors_read(&it.conductors) || !members_read {
        return Err(NOT_READ.into());
    }
    if !alt.areas.iter().any(|a| a == &it.area.base) {
        return Err(it.area.base.clone());
    }
    if alt.to == "unit_panel" && it.kind == "feeder" && it.to.is_empty() {
        return Err("not to a unit's panel".into());
    }
    for f in &it.area.flags {
        if alt.excluded_areas.contains_key(f) {
            return Err(f.clone());
        }
    }
    if alt.excluded_areas.contains_key(&it.area.base) {
        return Err(it.area.base.clone());
    }
    // Only what is designed in raceway changes.
    let raceway = mepatoms::rules().raceway_named(&it.method).is_some() || it.method.is_empty();
    if !raceway {
        return Err(format!("already {}", it.method));
    }
    Ok(())
}

/// The design's pieces as takeoff lines (a junction box of its runs too: the scenario's takeoff
/// leaves it out) and its firestops; a feeder's conductors from its schedule where the model has
/// no wire pieces. The device boxes a cable's work replaces are [`super::cablework`]'s.
pub(crate) fn design_lines(d: &Drawing, sp: &BuildingSpec, items: &[&Item], fs_item: &str) -> Vec<Line> {
    let mut by: BTreeMap<(String, String, String, String), f64> = BTreeMap::new();
    for it in items {
        let feeder = it.feeder.and_then(|i| sp.mep.feeders.get(i));
        let mut conduit = false;
        for h in it.design.iter().take(MAX_PIECES) {
            let Some(e) = d.model.get(*h) else { continue };
            let ps = props::read(&e.common.xdata);
            let g = |k: &str| ps.get(k).map(Prop::text).unwrap_or_default();
            let kind = g(mepatoms::ATOM_KEY);
            // A feeder's conductors come from its schedule (its raceways in the model carry one
            // set's only at each gear).
            if kind.is_empty() || (feeder.is_some() && kind == "wire") {
                continue;
            }
            conduit |= kind == "conduit";
            let by_ft = matches!(kind.as_str(), "conduit" | "cable" | "wire");
            let qty = if by_ft { g("length_ft").parse::<f64>().ok().filter(|v| v.is_finite() && *v >= 0.0).unwrap_or(0.0) } else { 1.0 };
            *by.entry((kind.clone(), g("item"), g("size"), if by_ft { "ft".into() } else { "ea".into() })).or_default() += qty;
        }
        if let Some(f) = feeder {
            let al = f.material.trim().eq_ignore_ascii_case("al");
            let wire = if al { "XHHW aluminum" } else { "THHN copper" };
            let sets = f64::from(f.sets.max(1));
            let mut add = |n: u32, s: &str| {
                if n > 0 {
                    *by.entry(("wire".into(), wire.into(), super::super::ugsheets::wire_size(s), "ft".into())).or_default() +=
                        it.feet * sets * f64::from(n);
                }
            };
            add(f.phases, &f.size);
            add(f.neutrals, if f.neutral_size.is_empty() { &f.size } else { &f.neutral_size });
            add(f.grounds, if f.ground_size.is_empty() { &f.size } else { &f.ground_size });
            // No pieces in the model (its feeders not built): its raceways cut from its routes as
            // the atom builder cuts them (sticks, couplings, connectors, straps, elbows).
            if !conduit {
                let upf = if sp.units_per_foot.is_finite() && sp.units_per_foot > 0.0 { sp.units_per_foot } else { 12.0 };
                let ar = mepatoms::rules();
                let size = if f.conduit_in.is_finite() && f.conduit_in > 0.0 { f.conduit_in } else { 1.0 };
                let name = if ar.raceways.contains_key(&it.method) { it.method.clone() } else { ar.default_raceway.clone() };
                if let Some(rw) = ar.raceways.get(&name) {
                    let large = mepatoms::large_conductors(&feeder_makeup(f), ar.bushing_min_awg);
                    for r in it.runs.iter().filter_map(|i| sp.mep.runs.get(*i)) {
                        let Some(level) = sp.level(&r.level) else { continue };
                        let path = mepatoms::build::run_path(r, level.elevation_ft * upf, upf);
                        for a in mepatoms::raceway_pieces(&path, upf, size, &name, rw, large, ar).atoms {
                            let by_ft = matches!(a.kind, "conduit" | "cable" | "wire");
                            let qty = if by_ft { a.length_ft } else { 1.0 };
                            if qty.is_finite() && qty > 0.0 && a.kind != "wire" {
                                *by.entry((a.kind.to_string(), a.item.clone(), a.size.clone(), if by_ft { "ft".into() } else { "ea".into() }))
                                    .or_default() += qty;
                            }
                        }
                    }
                }
            }
        }
        if it.rated_penetrations > 0 {
            let size = mepatoms::trade_size(sp.mep.runs.get(it.runs.first().copied().unwrap_or(usize::MAX)).map_or(0.75, |r| r.size_in[0]));
            *by.entry(("firestop".into(), fs_item.into(), size, "ea".into())).or_default() += it.rated_penetrations as f64;
        }
    }
    by.into_iter()
        .filter(|(_, q)| *q > 0.0)
        .map(|((kind, item, size, unit), qty)| Line { kind, item, size, unit, qty: (qty * 100.0).round() / 100.0 })
        .collect()
}

/// The alternative's pieces along the same routes: one cable per circuit (a homerun's circuits
/// each their own), a feeder's per set, with the cable's supports and connectors; each with its
/// item's level and circuit.
/// `paths`: each routed leg's path as a cable runs it, by run id, where the routing found one
/// (through the studs of a room's walls, [`super::super::conduitroute::cable_paths`]); the
/// raceway's path elsewhere.
pub(crate) fn alt_atoms(sp: &BuildingSpec, items: &[&Item], cable: &str, paths: &HashMap<String, MepRun>) -> Vec<(Atom, String, String, String)> {
    let upf = if sp.units_per_foot.is_finite() && sp.units_per_foot > 0.0 { sp.units_per_foot } else { 12.0 };
    let ar = mepatoms::rules();
    let Some(cr) = ar.cables.get(cable) else { return Vec::new() };
    let mut out = Vec::new();
    for it in items {
        let sets: Vec<(String, String)> = match (it.kind, it.feeder.and_then(|i| sp.mep.feeders.get(i))) {
            // A feeder's sets: one cable each, its raceways' routes sharing them.
            ("feeder", Some(f)) => {
                std::iter::repeat_n((it.label.clone(), feeder_makeup(f)), (f.sets.max(1) / f.conduits.max(1)).clamp(1, 12) as usize).collect()
            }
            _ if !it.members.is_empty() => {
                it.members.iter().map(|(l, c)| (l.clone(), if c.is_empty() { it.conductors.clone() } else { c.clone() })).collect()
            }
            _ => vec![(it.label.clone(), it.conductors.clone())],
        };
        for ri in it.runs.iter().take(MAX_RUNS) {
            let Some(r) = sp.mep.runs.get(*ri) else { continue };
            let r = paths.get(&r.id).filter(|_| !r.id.is_empty()).unwrap_or(r);
            let Some(level) = sp.level(&r.level) else { continue };
            let path = mepatoms::build::run_path(r, level.elevation_ft * upf, upf);
            if path.len() < 2 {
                continue;
            }
            for (circuit, makeup) in &sets {
                for a in mepatoms::cable_pieces(&path, upf, cable, cr, makeup, ar).atoms {
                    out.push((a, it.level.clone(), circuit.clone(), it.id.clone()));
                }
            }
        }
    }
    out
}

/// The alternative's pieces as takeoff lines, with a cable firestop at each rated penetration.
/// A cable line's size is its conductors (`12/2 w/G`; a feeder's `2/0-2/0-2/0-4`).
pub(crate) fn alt_lines(sp: &BuildingSpec, items: &[&Item], atoms: &[(Atom, String, String, String)], cable: &str, fs_item: &str) -> Vec<Line> {
    let mut by: BTreeMap<(String, String, String, String), f64> = BTreeMap::new();
    let feeders: HashMap<&str, &cadaddy_walk3d::Feeder> =
        items.iter().filter_map(|it| it.feeder.and_then(|i| sp.mep.feeders.get(i)).map(|f| (it.id.as_str(), f))).collect();
    for (a, _, _, of) in atoms {
        let by_ft = matches!(a.kind, "cable" | "conduit" | "wire");
        let (item, size) = if a.kind == "cable" {
            match feeders.get(of.as_str()) {
                Some(f) => {
                    let al = f.material.trim().eq_ignore_ascii_case("al");
                    let bare = |s: &str| super::super::ugsheets::wire_size(s).trim_start_matches('#').to_string();
                    let mut sizes: Vec<String> = std::iter::repeat_n(bare(&f.size), f.phases.clamp(1, 3) as usize).collect();
                    sizes.extend(std::iter::repeat_n(
                        bare(if f.neutral_size.is_empty() { &f.size } else { &f.neutral_size }),
                        f.neutrals.min(2) as usize,
                    ));
                    if f.grounds > 0 {
                        sizes.push(bare(if f.ground_size.is_empty() { &f.size } else { &f.ground_size }));
                    }
                    (format!("{} {}", a.item, if al { "aluminum" } else { "copper" }), sizes.join("-"))
                }
                None => (a.item.clone(), a.ty.strip_prefix(cable).map(str::trim).unwrap_or(&a.ty).to_string()),
            }
        } else {
            (a.item.clone(), a.size.clone())
        };
        let qty = if by_ft { a.length_ft } else { 1.0 };
        if qty.is_finite() && qty > 0.0 {
            *by.entry((a.kind.to_string(), item, size, if by_ft { "ft".into() } else { "ea".into() })).or_default() += qty;
        }
    }
    let pens: usize = items.iter().map(|it| it.rated_penetrations).sum();
    if pens > 0 {
        *by.entry(("firestop".into(), fs_item.into(), String::new(), "ea".into())).or_default() += pens as f64;
    }
    by.into_iter().map(|((kind, item, size, unit), qty)| Line { kind, item, size, unit, qty: (qty * 100.0).round() / 100.0 }).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tag(id: &str, level: &str, label: &str, area: &str) -> serde_json::Value {
        serde_json::json!({"id": id, "kind": "circuit", "level": level, "at_ft": [0, 0], "fields": {
            "circuit": {"value": label, "source": "plan"}, "panel": {"value": "P", "source": "plan"},
            "conductors": {"value": "2#12 + 1#12G", "source": "default"}, "conduit": {"value": "EMT", "source": "spec"},
            "route": {"value": "routed", "source": "computed"}, "area": {"value": area, "source": "computed"}
        }})
    }

    #[test]
    fn only_runs_under_the_lowest_floor_are_below_the_slab_and_untyped_ceilings_are_said() {
        // Two storeys: L2's circuit drops through its floor to a box below (a riser from -1 ft);
        // L1's circuit runs under the slab on grade (-1.5 ft).
        let sp: BuildingSpec = serde_json::from_value(serde_json::json!({
            "version": 1, "units_per_foot": 12,
            "levels": [{"name": "L1", "elevation_ft": 0, "height_ft": 10}, {"name": "L2", "elevation_ft": 10, "height_ft": 10}],
            "tags": [tag("circuit:L2:E-201:P-1", "L2", "P-1", "unit"), tag("circuit:L1:E-101:P-3", "L1", "P-3", "concealed")],
            "mep": {"runs": [
                {"system": "E-BRANCH", "level": "L2", "id": "circuit:L2:E-201:P-1#0", "points": [[0, 0], [120, 0]], "size_in": [0.75, 0], "bottoms_ft": [9, 9]},
                {"system": "E-BRANCH", "level": "L2", "id": "circuit:L2:E-201:P-1#1", "points": [[120, 0]], "size_in": [0.75, 0], "bottoms_ft": [-1.0], "top_ft": 9},
                {"system": "E-BRANCH", "level": "L1", "id": "circuit:L1:E-101:P-3#0", "points": [[0, 0], [240, 0]], "size_in": [0.75, 0], "bottoms_ft": [-1.5, -1.5]}
            ]}
        }))
        .unwrap();
        let d = Drawing::new_imperial();
        let rules = super::super::rules();
        let items = items(&d, &sp, &CodeFacts::default(), &rules);
        assert_eq!(items.len(), 2, "{items:?}");
        let up = items.iter().find(|i| i.label == "P-1").unwrap();
        assert!(!up.area.flags.contains("below_slab"), "a riser through an upper floor: {:?}", up.area);
        assert_eq!(up.area.base, "unit");
        assert!((up.feet - 20.0).abs() < 1e-6, "10 ft across and 10 ft up: {}", up.feet);
        let low = items.iter().find(|i| i.label == "P-3").unwrap();
        assert!(low.area.flags.contains("below_slab"), "{:?}", low.area);
        assert_eq!(low.area.base, "common");
        // NM never goes below the slab; MC neither.
        for id in ["nm_common_branch", "mc_concealed_branch"] {
            let alt = rules.alternatives.iter().find(|a| a.id == id).unwrap();
            assert_eq!(takes(alt, low), Err("below_slab".to_string()), "{id}");
        }
        let nm = rules.alternatives.iter().find(|a| a.id == "nm_unit_branch").unwrap();
        assert_eq!(takes(nm, up), Ok(()));
        // A feeder's conductors as one makeup (`3#2/0 AL + 1#4G`), kcmil sizes by their number.
        let f = cadaddy_walk3d::Feeder {
            phases: 2,
            neutrals: 1,
            grounds: 1,
            size: "2/0".into(),
            ground_size: "4".into(),
            material: "al".into(),
            ..Default::default()
        };
        assert_eq!(feeder_makeup(&f), "3#2/0 AL + 1#4G");
        let big = cadaddy_walk3d::Feeder {
            phases: 3,
            neutrals: 1,
            grounds: 1,
            size: "500 kcmil".into(),
            neutral_size: "250".into(),
            ground_size: "1/0".into(),
            ..Default::default()
        };
        assert_eq!(feeder_makeup(&big), "3#500 + 1#250 + 1#1/0G");
        assert_eq!(mepatoms::conductor_parts(&feeder_makeup(&big)).iter().filter(|p| p.2).count(), 1);
        // Conductors as a schedule's description (a misread cell) are not weighed.
        for ok in ["2#12 + 1#12G", "3#10, 1#10G", "2#12 + 1#12G in 3/4\" EMT", "3#500 + 1#250 + 1#1/0G"] {
            assert!(conductors_read(ok), "{ok}");
        }
        for bad in ["DRYER #1", "", "#12", "2 #", "TWO#12", "2#G"] {
            assert!(!conductors_read(bad), "{bad}");
        }
        let mut odd = low.clone();
        odd.conductors = "DRYER #1".into();
        odd.area.flags.clear();
        let mc = rules.alternatives.iter().find(|a| a.id == "mc_concealed_branch").unwrap();
        assert_eq!(takes(mc, &odd), Err(NOT_READ.to_string()));
    }

    #[test]
    fn a_homeruns_circuits_are_the_ones_that_name_its_group_two_pole_labels_whole() {
        // A homerun to panel P carrying P-1 and the two-pole P-3,5 (its list: `P-1, P-3,5`); a
        // second unit's P-1 on another homerun has other conductors.
        let mut a = tag("circuit:L1:E-101:P-1@P1", "L1", "P-1", "unit");
        a["fields"]["homerun_group"] = serde_json::json!({"value": "P1:0", "source": "computed"});
        let mut b = tag("circuit:L1:E-101:P-3,5@P1", "L1", "P-3,5", "unit");
        b["fields"]["homerun_group"] = serde_json::json!({"value": "P1:0", "source": "computed"});
        b["fields"]["conductors"] = serde_json::json!({"value": "3#10 + 1#10G", "source": "plan"});
        let mut c = tag("circuit:L1:E-101:P-1@P2", "L1", "P-1", "unit");
        c["fields"]["homerun_group"] = serde_json::json!({"value": "P2:1", "source": "computed"});
        c["fields"]["conductors"] = serde_json::json!({"value": "2#8 + 1#8G", "source": "computed"});
        let mut h = tag("circuit:L1:home:P1:0", "L1", "P-1,3,5", "");
        h["fields"]["circuits"] = serde_json::json!({"value": "P-1, P-3,5", "source": "computed"});
        let sp: BuildingSpec = serde_json::from_value(serde_json::json!({
            "version": 1, "units_per_foot": 12,
            "levels": [{"name": "L1", "elevation_ft": 0, "height_ft": 10}],
            "tags": [a, b, c, h],
            "mep": {"runs": [
                {"system": "E-BRANCH", "level": "L1", "id": "circuit:L1:home:P1:0#home", "points": [[0, 0], [120, 0]], "size_in": [0.75, 0], "bottoms_ft": [9, 9]},
                {"system": "E-BRANCH", "level": "L1", "id": "circuit:L1:E-101:P-1@P1#0", "points": [[120, 0], [240, 0]], "size_in": [0.75, 0], "bottoms_ft": [9, 9]}
            ]}
        }))
        .unwrap();
        let items = items(&Drawing::new_imperial(), &sp, &CodeFacts::default(), &super::super::rules());
        let home = items.iter().find(|i| i.id == "circuit:L1:home:P1:0").unwrap();
        assert_eq!(home.members, [("P-1".to_string(), "2#12 + 1#12G".to_string()), ("P-3,5".to_string(), "3#10 + 1#10G".to_string())]);
        assert_eq!(home.area.base, "unit");
        let refs: Vec<&Item> = vec![home];
        let atoms = alt_atoms(&sp, &refs, "NM", &HashMap::new());
        let types: BTreeSet<String> = atoms.iter().filter(|a| a.0.kind == "cable").map(|a| a.0.ty.clone()).collect();
        assert_eq!(types, BTreeSet::from(["NM 10/3 w/G".to_string(), "NM 12/2 w/G".to_string()]), "one cable per circuit, each its own conductors");
    }

    #[test]
    fn a_cable_alternative_is_measured_on_the_cables_own_path_through_the_studs() {
        // P-1's joint between two wall boxes 10' apart: the raceway goes up to 9' and back down
        // (10' + 2 x 7'-6"), the cable through the studs 12" over them (10' + 2 x 1').
        let sp: BuildingSpec = serde_json::from_value(serde_json::json!({
            "version": 1, "units_per_foot": 12,
            "levels": [{"name": "L1", "elevation_ft": 0, "height_ft": 10}],
            "tags": [tag("circuit:L1:E-101:P-1", "L1", "P-1", "unit")],
            "mep": {"runs": [
                {"system": "E-BRANCH", "level": "L1", "id": "circuit:L1:E-101:P-1#1", "points": [[0, 0], [0, 0], [120, 0], [120, 0]], "size_in": [0.75, 0], "bottoms_ft": [1.5, 9, 1.5]}
            ]}
        }))
        .unwrap();
        let items = items(&Drawing::new_imperial(), &sp, &CodeFacts::default(), &super::super::rules());
        let refs: Vec<&Item> = items.iter().collect();
        let cable_ft = |atoms: &[(Atom, String, String, String)]| atoms.iter().filter(|a| a.0.kind == "cable").map(|a| a.0.length_ft).sum::<f64>();
        let makeup = 2.0 * mepatoms::rules().makeup_ft;
        assert!((cable_ft(&alt_atoms(&sp, &refs, "NM", &HashMap::new())) - (25.0 + makeup)).abs() < 1e-6, "the raceway's path");
        let mut stud = sp.mep.runs.first().cloned().unwrap();
        stud.bottoms_ft = vec![1.5, 2.5, 1.5];
        let paths: HashMap<String, MepRun> = [(stud.id.clone(), stud)].into_iter().collect();
        assert!((cable_ft(&alt_atoms(&sp, &refs, "NM", &paths)) - (12.0 + makeup)).abs() < 1e-6, "its own path");
    }
}
