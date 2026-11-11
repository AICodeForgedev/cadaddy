//! Which unit serves each air terminal (Q6318): the unit its duct system comes from, the way
//! the field traces it.
//!
//! In order of authority:
//! 1. **The duct network** (derived): the unit the terminal's ducts lead to, through the built
//!    model's `connects` (runs, fittings, risers and units the build joined), nearest by pieces.
//! 2. **The schedule** (derived): a unit whose schedule row names what it serves (`SERVES`,
//!    `AREA SERVED`: `CORRIDORS`) where that names the terminal's room.
//! 3. **The unit in its room** (inferred, trade practice): a room's own air unit (a PTAC, a fan
//!    coil, a bath exhaust fan) serves the room's terminals of its air.
//!
//! A unit serves a terminal of its air only: an exhaust terminal an exhaust unit (its kind or
//! schedule says exhaust, or it recovers energy), a supply or return one any other air unit.
//! A unit moves air when it is on a duct system or its schedule gives an airflow. A terminal
//! none of these settles is left unserved (`props.check` `terminal_unserved` lists it).

use cadaddy_doc::{Drawing, Entity, Handle};
use cadaddy_geom::Vec2;
use cadaddy_walk3d::{BuildingSpec, Made, MepEquipment, MepKind};
use std::collections::{BTreeMap, HashMap, VecDeque};

/// The trade practice rule a room's own unit serving its terminals is inferred by.
pub(crate) const ROOM_UNIT_RULE: &str = "M-TERMINAL-ROOM-UNIT";
/// Elements looked at, at most (hostile drawings).
const MAX_ELEMENTS: usize = 500_000;

/// The unit serving one terminal and why.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Served {
    pub unit: Handle,
    /// The unit's tag (`DOAS-1`).
    pub tag: String,
    /// Derived (the network, the schedule) or inferred (the room's unit).
    pub made: Made,
    pub basis: String,
}

/// One built air unit.
struct Unit<'a> {
    h: Handle,
    tag: String,
    level: String,
    at: Vec2,
    spec: Option<&'a MepEquipment>,
}

impl Unit<'_> {
    fn kind(&self) -> String {
        self.spec.map(|e| e.kind.to_ascii_lowercase()).unwrap_or_default()
    }

    fn exhausts(&self) -> bool {
        let k = self.kind();
        k.contains("exhaust") || self.spec.is_some_and(|e| e.title.to_ascii_uppercase().contains("EXHAUST"))
    }

    fn recovers(&self) -> bool {
        let k = self.kind();
        k.contains("recovery") || k.contains("erv") || self.spec.is_some_and(|e| e.title.to_ascii_uppercase().contains("RECOVERY"))
    }

    /// Whether it moves air: on a duct system, or its schedule gives an airflow.
    fn moves_air(&self, sp: &BuildingSpec) -> bool {
        self.spec.is_some_and(|e| {
            e.systems.iter().any(|s| sp.mep.systems.iter().any(|x| x.kind == MepKind::Duct && x.name.eq_ignore_ascii_case(s)))
                || e.properties.iter().any(|(k, v)| k.to_ascii_uppercase().contains("CFM") && v.chars().any(|c| c.is_ascii_digit()))
        })
    }

    /// Whether it serves a terminal of air system `system` (`EA`: exhaust).
    fn serves_air(&self, system: &str) -> bool {
        if system.eq_ignore_ascii_case("EA") { self.exhausts() || self.recovers() } else { !self.exhausts() }
    }

    /// What its schedule row says it serves (`SERVES`, `AREA SERVED`), upper case.
    fn serves_text(&self) -> String {
        self.spec
            .map(|e| {
                e.properties
                    .iter()
                    .filter(|(k, _)| k.to_ascii_uppercase().contains("SERV"))
                    .map(|(_, v)| v.to_ascii_uppercase())
                    .collect::<Vec<_>>()
                    .join(" ")
            })
            .unwrap_or_default()
    }
}

/// One built air terminal.
struct Term {
    h: Handle,
    level: String,
    at: Vec2,
    system: String,
}

fn made(e: &Entity, key: &str) -> Option<String> {
    super::bimlook::made_tag(e, key).map(str::to_string).filter(|v| !v.trim().is_empty())
}

/// Whether a built element is an air terminal (not a sprinkler head).
pub(crate) fn is_air_terminal(e: &Entity) -> bool {
    super::bimlook::made_tag(e, "part").is_some_and(|p| p.starts_with("terminal:") && !p.contains("sprinkler"))
}

/// The words of a room's name a schedule's `SERVES` might name it by (`CORRIDOR`), singular.
fn name_words(name: &str) -> Vec<String> {
    name.to_ascii_uppercase()
        .split(|c: char| !c.is_ascii_alphabetic())
        .filter(|w| w.len() >= 4)
        .map(|w| w.strip_suffix('S').unwrap_or(w).to_string())
        .collect()
}

/// Whether `text` names a room called `name` (one of its words, singular or plural).
fn names_room(text: &str, name: &str) -> bool {
    let said: Vec<String> = name_words(text);
    name_words(name).iter().any(|w| said.contains(w))
}

/// The unit serving each air terminal of the built model (see the module doc). `room_of`: the
/// room (its label) a point of a level stands in.
pub(crate) fn serve(d: &Drawing, sp: &BuildingSpec, room_of: &dyn Fn(Option<&str>, Vec2) -> Option<String>) -> HashMap<Handle, Served> {
    let by_tag: HashMap<String, &MepEquipment> = sp.mep.equipment.iter().map(|e| (e.tag.trim().to_ascii_uppercase(), e)).collect();
    let mut adj: HashMap<Handle, Vec<Handle>> = HashMap::new();
    let mut units: Vec<Unit> = Vec::new();
    let mut terms: Vec<Term> = Vec::new();
    // The whole drawing: a job's model comes after its imported sheets' objects; what is kept
    // is bounded instead.
    for e in d.model.iter() {
        let Some(made_as) = super::wallgen::made_as(e) else { continue };
        if adj.len() >= MAX_ELEMENTS || units.len() + terms.len() >= MAX_ELEMENTS {
            break;
        }
        // Every built piece's joins (a run built as its atoms, sticks and fittings, too).
        for c in super::mepatoms::connects_of(e) {
            adj.entry(e.handle).or_default().push(c);
            adj.entry(c).or_default().push(e.handle);
        }
        if made_as != "mep" {
            continue;
        }
        let part = made(e, "part").unwrap_or_default();
        let at = || super::propfill::extent(d, e).map(|(b, _, _)| b.center());
        let level = super::bspec::made_level(e).unwrap_or_default();
        if part.starts_with("equipment") {
            let tag = made(e, "tag").unwrap_or_default();
            if let Some(at) = at() {
                units.push(Unit { h: e.handle, spec: by_tag.get(&tag.trim().to_ascii_uppercase()).copied(), tag, level, at });
            }
        } else if is_air_terminal(e)
            && let Some(at) = at()
        {
            terms.push(Term { h: e.handle, level, at, system: made(e, "system").unwrap_or_default() });
        }
    }
    let unit_at: HashMap<Handle, usize> = units.iter().enumerate().map(|(i, u)| (u.h, i)).collect();
    let term_at: HashMap<Handle, usize> = terms.iter().enumerate().map(|(i, t)| (t.h, i)).collect();
    // 1. The network: out from every unit at once, each piece reached keeps the nearest unit (by
    // pieces); a terminal ends a path, another unit too.
    let mut nearest: HashMap<Handle, (usize, usize)> = HashMap::new();
    let mut queue: VecDeque<Handle> = VecDeque::new();
    for (i, u) in units.iter().enumerate() {
        nearest.insert(u.h, (i, 0));
        queue.push_back(u.h);
    }
    while let Some(x) = queue.pop_front() {
        let Some(&(u, hops)) = nearest.get(&x) else { continue };
        if nearest.len() > MAX_ELEMENTS || (term_at.contains_key(&x)) || (unit_at.contains_key(&x) && unit_at.get(&x) != Some(&u)) {
            continue;
        }
        for n in adj.get(&x).into_iter().flatten() {
            if !nearest.contains_key(n) && !unit_at.contains_key(n) {
                nearest.insert(*n, (u, hops + 1));
                queue.push_back(*n);
            }
        }
    }
    let mut out = HashMap::new();
    for t in &terms {
        // The network's unit, of the terminal's air.
        if let Some(u) = nearest.get(&t.h).and_then(|(u, _)| units.get(*u)).filter(|u| u.serves_air(&t.system)) {
            out.insert(
                t.h,
                Served { unit: u.h, tag: u.tag.clone(), made: Made::Derived, basis: format!("the duct network: its duct leads to {}", u.tag) },
            );
            continue;
        }
        let level = (!t.level.is_empty()).then_some(t.level.as_str());
        let room = room_of(level, t.at);
        // 2. A unit whose schedule row names the terminal's room as what it serves.
        if let Some(name) = room.as_deref() {
            let named: Vec<&Unit> =
                units.iter().filter(|u| u.serves_air(&t.system) && u.moves_air(sp) && names_room(&u.serves_text(), name)).collect();
            // One schedule row per unit (a row's tag built more than once is one unit's tag).
            let tags: BTreeMap<&str, &Unit> = named.iter().map(|u| (u.tag.as_str(), *u)).collect();
            if tags.len() == 1
                && let Some(u) = named.iter().min_by(|a, b| a.at.dist(t.at).total_cmp(&b.at.dist(t.at)))
            {
                out.insert(
                    t.h,
                    Served {
                        unit: u.h,
                        tag: u.tag.clone(),
                        made: Made::Derived,
                        basis: format!("{}'s schedule row says it serves {}; the terminal is in {name}", u.tag, u.serves_text().trim()),
                    },
                );
                continue;
            }
        }
        // 3. The air unit of its air in its room (trade practice).
        if let Some(name) = room.as_deref() {
            let own = units
                .iter()
                .filter(|u| u.level.eq_ignore_ascii_case(&t.level) && u.serves_air(&t.system) && u.moves_air(sp))
                .filter(|u| room_of(level, u.at).as_deref() == Some(name))
                .min_by(|a, b| a.at.dist(t.at).total_cmp(&b.at.dist(t.at)));
            if let Some(u) = own {
                out.insert(
                    t.h,
                    Served {
                        unit: u.h,
                        tag: u.tag.clone(),
                        made: Made::Inferred,
                        basis: super::provenance::rule(ROOM_UNIT_RULE).basis_for(&format!("{} in {name}", u.tag)),
                    },
                );
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_schedule_names_a_room_by_any_of_its_words() {
        assert!(names_room("CORRIDORS", "201 CORRIDOR"));
        assert!(names_room("CORRIDORS AND LOBBY", "LOBBY"));
        assert!(!names_room("GUEST ROOMS", "CORRIDOR"));
        assert!(!names_room("", "CORRIDOR"));
    }
}
