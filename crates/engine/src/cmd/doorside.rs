//! Door hardware devices on the door's working side (Q6605): a card reader, a door position
//! contact or a magnetic door holder serves the leaf's free (latch) edge, so the field sets it on
//! the latch side, below the head — never at the hinge jamb, wherever the plan's symbol (or a
//! symbol block's insert) happened to put it.
//!
//! Each built door knows its swing: the hinge, the open leaf's end and the shut leaf's end (the
//! latch jamb). Each door hardware device (`doorside.json`: its kinds, the words of its type, its
//! legend mark) is matched to the door it serves on its level (on that door's wall within reach
//! of the opening, or for a holder within reach of the hinge; the nearest door, none when two
//! are as near). Then:
//!
//! - `latch_jamb` (card reader, door contact): on the hinge half of the opening (or past the hinge
//!   jamb) it is mirrored across the opening's middle: the same distance from the latch jamb as
//!   it stood from the hinge jamb, on the same wall.
//! - `open_leaf_edge` (door holder): nearer the hinge than the open leaf's end, it goes to where
//!   the open leaf's latch edge rests, set back along the leaf by the rule's backset.
//! - Any of them topped above the head (less the rule's clearance) comes down below it.
//!
//! The corrected spot is built; the device keeps the spot it was drawn at, the door, the reason
//! and the rule (`door_side`), so the correction is never passed off as drawn and field output
//! shows the difference. A device on a pair of doors (the latch is the meeting edge, the reader
//! goes by the active leaf) or with no door near is left as drawn, counted.
//!
//! General: kinds, words, marks, reaches and places are rules data, never a job's.

use std::collections::BTreeMap;

use cadaddy_doc::props::{self, Prop, Source, SourceKind};
use cadaddy_doc::{Drawing, Handle};
use cadaddy_geom::{Vec2, Vec3};
use serde_json::{Value, json};

use super::propfill::WithNote;
use super::{CommandSpec, bool_param};
use crate::{Result, Session};

pub const DOORSIDE: &str = "bim.doorside";
/// The device's record of the correction.
pub const DOOR_SIDE_KEY: &str = "door_side";
const RULES: &str = include_str!("doorside.json");
const RULES_DOC: &str = "rules data: doorside.json";

/// Doors and devices looked at, at most; corrections listed.
const MAX_DOORS: usize = 100_000;
const MAX_DEVICES: usize = 500_000;
const MAX_LISTED: usize = 40;
const MAX_WORDS: usize = 64;

fn rules() -> &'static Value {
    static R: std::sync::OnceLock<Value> = std::sync::OnceLock::new();
    R.get_or_init(|| serde_json::from_str(RULES).unwrap_or(Value::Null))
}

fn num(r: &Value, key: &str, default: f64) -> f64 {
    r.get(key).and_then(Value::as_f64).filter(|v| v.is_finite() && *v >= 0.0 && *v <= 1000.0).unwrap_or(default)
}

fn words(r: &Value, key: &str) -> Vec<String> {
    r.get(key)
        .and_then(Value::as_array)
        .map(|a| a.iter().take(MAX_WORDS).filter_map(Value::as_str).map(|w| w.trim().to_ascii_uppercase()).filter(|w| !w.is_empty()).collect())
        .unwrap_or_default()
}

/// A kind of door hardware device the rules know.
#[derive(Clone, Debug, PartialEq)]
struct Item {
    kind: String,
    kinds: Vec<String>,
    words: Vec<String>,
    marks: Vec<String>,
    place: String,
    backset_in: f64,
    basis: String,
}

fn items(r: &Value) -> Vec<Item> {
    r.get("items")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .take(MAX_WORDS)
                .map(|i| Item {
                    kind: i.get("kind").and_then(Value::as_str).unwrap_or_default().to_string(),
                    kinds: words(i, "kinds"),
                    words: words(i, "words"),
                    marks: words(i, "marks"),
                    place: i.get("place").and_then(Value::as_str).unwrap_or("latch_jamb").to_string(),
                    backset_in: num(i, "backset_in", 3.0),
                    basis: i.get("basis").and_then(Value::as_str).unwrap_or_default().to_string(),
                })
                .collect()
        })
        .unwrap_or_default()
}

/// The rules' item a device is: by its kind, its legend mark, else the words of its type and
/// description; never a power device (`not_kinds`: a receptacle on a door holders' circuit is no
/// door holder).
fn item_of<'a>(items: &'a [Item], not_kinds: &[String], kind: &str, mark: &str, text: &str) -> Option<&'a Item> {
    let (kind, mark, text) = (kind.trim().to_ascii_uppercase(), mark.trim().to_ascii_uppercase(), text.to_ascii_uppercase());
    if not_kinds.contains(&kind) {
        return None;
    }
    items
        .iter()
        .find(|i| i.kinds.contains(&kind))
        .or_else(|| items.iter().find(|i| !mark.is_empty() && i.marks.contains(&mark)))
        .or_else(|| items.iter().find(|i| i.words.iter().any(|w| text.contains(w.as_str()))))
}

/// A built door as hardware serves it (plan, drawing units): hinge, the open leaf's end, the shut
/// leaf's end (the latch jamb), its head's height and its level.
#[derive(Clone, Debug)]
struct Door {
    h: Handle,
    number: String,
    level: Option<String>,
    hinge: Vec2,
    open: Vec2,
    latch: Vec2,
    head: f64,
    /// Its opening is a pair's (two leaves).
    pair: bool,
}

fn doors(d: &Drawing, pair_ratio: f64) -> Vec<Door> {
    let mut out = Vec::new();
    for e in d.model.iter().filter(|e| super::wallgen::made_as(e).as_deref() == Some("door")).take(MAX_DOORS) {
        let Some([hinge, open, latch]) = super::wallgen::door_swing(e) else { continue };
        let Some((_, hi)) = super::fieldview::mesh_box(e) else { continue };
        let leaf = hinge.dist(latch);
        if !(leaf.is_finite() && leaf > 0.0 && hi[2].is_finite()) {
            continue;
        }
        let pair = super::wallgen::door_opening(e).is_some_and(|[a, b]| a.dist(b) > pair_ratio * leaf);
        let number = props::read(&e.common.xdata).get("number").map(Prop::text).unwrap_or_default();
        out.push(Door { h: e.handle, number, level: super::bspec::made_level(e), hinge, open, latch, head: hi[2], pair });
    }
    out
}

/// What [`side`] did.
#[derive(Clone, Debug, Default)]
pub(crate) struct Sided {
    /// Door hardware devices looked at.
    pub checked: usize,
    /// Moved to the latch side (or a holder to the open leaf's edge), and brought below the head.
    pub moved: usize,
    pub lowered: usize,
    /// Already on the latch side.
    pub right_side: usize,
    /// Left as drawn: no door near, two doors as near, a pair of doors.
    pub no_door: usize,
    pub ambiguous: usize,
    pub pair: usize,
    pub by_kind: BTreeMap<String, usize>,
    pub listed: Vec<Value>,
}

impl Sided {
    pub(crate) fn report(&self) -> Value {
        json!({
            "checked": self.checked,
            "moved": self.moved,
            "lowered": self.lowered,
            "right_side": self.right_side,
            "left_as_drawn": {"no_door_near": self.no_door, "two_doors_as_near": self.ambiguous, "pair_of_doors": self.pair},
            "moved_by_kind": self.by_kind,
            "moved_first": self.listed,
        })
    }
}

/// The door hardware devices of `d` on their doors' latch side, below the head (see the module
/// docs); with `apply` false only measured. `upf`: drawing units per foot.
pub(crate) fn side(d: &mut Drawing, upf: f64, apply: bool) -> Sided {
    let mut out = Sided::default();
    let r = rules();
    let inch = if upf.is_finite() && upf > 0.0 { upf / 12.0 } else { 1.0 };
    let (reach, wall_off, clear, tie) = (
        num(r, "door_reach_in", 36.0) * inch,
        num(r, "wall_off_in", 18.0) * inch,
        num(r, "head_clear_in", 2.0) * inch,
        num(r, "ambiguous_in", 6.0) * inch,
    );
    let list = items(r);
    let not_kinds = words(r, "not_kinds");
    let all_doors = doors(d, num(r, "pair_ratio", 1.5));
    if all_doors.is_empty() || list.is_empty() {
        return out;
    }
    let mut moves: Vec<(Handle, Vec3, Prop)> = Vec::new();
    for e in d.model.iter().filter(|e| super::wallgen::made_as(e).as_deref() == Some("device")).take(MAX_DEVICES) {
        let store = props::read(&e.common.xdata);
        let get = |k: &str| store.get(k).map(Prop::text).unwrap_or_default();
        let text = ["type", "description", "name", "submittal", "catalog_number"].iter().map(|k| get(k)).collect::<Vec<_>>().join(" ");
        let Some(item) = item_of(&list, &not_kinds, &get("kind"), &get("type_mark"), &text) else { continue };
        let Some((lo, hi)) = super::fieldview::mesh_box(e) else { continue };
        let c = Vec2::new((lo[0] + hi[0]) / 2.0, (lo[1] + hi[1]) / 2.0);
        if !c.is_finite() {
            continue;
        }
        out.checked += 1;
        let level = super::bspec::made_level(e);
        let same_level = |dr: &&Door| match (&dr.level, &level) {
            (Some(a), Some(b)) => a.eq_ignore_ascii_case(b),
            _ => true,
        };
        let holder = item.place == "open_leaf_edge";
        // The doors it may serve, by how far it is from each.
        let mut near: Vec<(f64, &Door)> = all_doors
            .iter()
            .filter(same_level)
            .filter_map(|dr| {
                let leaf = dr.hinge.dist(dr.latch);
                let u = (dr.latch - dr.hinge) / leaf;
                let t = (c - dr.hinge).dot(u);
                let across = (c - dr.hinge).cross(u).abs();
                let ok = if holder { c.dist(dr.hinge) <= leaf + reach } else { across <= wall_off && t >= -reach && t <= leaf + reach };
                ok.then(|| (c.dist(dr.hinge.mid(dr.latch)), dr))
            })
            .collect();
        near.sort_by(|a, b| a.0.total_cmp(&b.0));
        let Some(&(best, door)) = near.first() else {
            out.no_door += 1;
            continue;
        };
        if near.get(1).is_some_and(|(d2, _)| d2 - best <= tie) {
            out.ambiguous += 1;
            continue;
        }
        if door.pair {
            out.pair += 1;
            continue;
        }
        let leaf = door.hinge.dist(door.latch);
        let u = (door.latch - door.hinge) / leaf;
        let t = (c - door.hinge).dot(u);
        // Where it goes along the plan.
        let to = if holder {
            if c.dist(door.hinge) < c.dist(door.open) {
                let back = (door.hinge - door.open).normalized();
                Some(door.open + back * (item.backset_in * inch))
            } else {
                None
            }
        } else if t < leaf / 2.0 {
            Some(c + u * (leaf - 2.0 * t))
        } else {
            None
        };
        // Below the head.
        let dz = if hi[2] > door.head - clear { door.head - clear - hi[2] } else { 0.0 };
        if to.is_none() && dz == 0.0 {
            out.right_side += 1;
            continue;
        }
        let shift = to.map_or(Vec2::ZERO, |p| p - c);
        if !(shift.is_finite() && dz.is_finite()) {
            continue;
        }
        let ft = |v: f64| (v / upf.max(1e-9) * 100.0).round() / 100.0;
        let mut said = Vec::new();
        if to.is_some() {
            out.moved += 1;
            *out.by_kind.entry(item.kind.clone()).or_default() += 1;
            said.push(if holder {
                format!("moved from by the hinge of door {} to where its open leaf's latch edge rests ({} ft)", door_name(door), ft(shift.len()))
            } else {
                format!(
                    "moved from the hinge side of door {} to its latch side, as far from the latch jamb ({} ft)",
                    door_name(door),
                    ft(shift.len())
                )
            });
        }
        if dz < 0.0 {
            out.lowered += 1;
            said.push(format!("brought down below the door's head ({} in)", (dz.abs() / inch * 10.0).round() / 10.0));
        }
        let why = format!(
            "{}; drawn at [{}, {}, {}] ft: {}",
            said.join("; "),
            ft(c.x),
            ft(c.y),
            ft((lo[2] + hi[2]) / 2.0),
            item.basis.chars().take(300).collect::<String>()
        );
        if out.listed.len() < MAX_LISTED {
            out.listed.push(json!({"handle": e.handle.hex(), "kind": item.kind, "door": door_name(door), "door_handle": door.h.hex(), "moved_ft": ft(shift.len()), "lowered_in": (dz.abs() / inch * 10.0).round() / 10.0, "why": why}));
        }
        let mut p = Prop::new(why, Source::from_doc(SourceKind::Rule, RULES_DOC));
        p.note = Some("corrected for the field: the drawn spot is kept here, the device stands at the corrected one".into());
        moves.push((e.handle, Vec3::new(shift.x, shift.y, dz), p));
    }
    if apply {
        for (h, dv, p) in moves {
            let Some(e) = d.model.get(h) else { continue };
            let mut store = props::read(&e.common.xdata);
            store.set(DOOR_SIDE_KEY, p);
            if let Some((lo, hi)) = super::fieldview::mesh_box(e) {
                let c = Vec3::new((lo[0] + hi[0]) / 2.0 + dv.x, (lo[1] + hi[1]) / 2.0 + dv.y, (lo[2] + hi[2]) / 2.0 + dv.z);
                if store.get("location").is_some() {
                    store.set(
                        "location",
                        Prop::new(format!("{:.2}, {:.2}, {:.2}", c.x / upf, c.y / upf, c.z / upf), Source::computed())
                            .with_unit("ft")
                            .with_note("model coordinates: on its door's latch side (door_side)"),
                    );
                }
            }
            let xdata = props::write(&e.common.xdata, &store);
            let m = cadaddy_doc::translation(dv);
            let _ = d.modify_entity(h, |x| {
                x.kind.transform3(&m);
                x.common.xdata = xdata.into();
            });
        }
    }
    out
}

fn door_name(d: &Door) -> String {
    if d.number.trim().is_empty() { d.h.hex() } else { d.number.trim().to_string() }
}

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new(DOORSIDE, "Door Hardware on the Latch Side", run)
            .params("{apply?: bool (default true; false: measure only)} → {checked, moved, lowered, right_side, left_as_drawn: {no_door_near, two_doors_as_near, pair_of_doors}, moved_by_kind, moved_first: [{handle, kind, door, door_handle, moved_ft, lowered_in, why}]}: each door hardware device (card reader, door position contact, magnetic door holder: doorside.json) set on its door's latch side below the head — mirrored off the hinge side, a holder to where the open leaf's latch edge rests — its drawn spot, the door and the rule kept on it (door_side)")
            .menu(&["Tools", "Building", "Door Hardware on the Latch Side"]),
    ]
}

fn run(s: &mut Session, p: &Value) -> Result<Value> {
    let apply = bool_param(DOORSIDE, p, "apply")?.unwrap_or(true);
    let (sp, _) = super::bspec::current(s, DOORSIDE)?;
    let upf = if sp.units_per_foot.is_finite() && sp.units_per_foot > 0.0 { sp.units_per_foot } else { 12.0 };
    let done = side(s.doc_mut()?, upf, apply);
    if apply && done.moved + done.lowered > 0 {
        s.touch();
    }
    Ok(done.report())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_rules_read_and_a_power_device_on_a_door_holders_circuit_is_no_door_holder() {
        let r = rules();
        let list = items(r);
        let not_kinds = words(r, "not_kinds");
        assert!(list.len() >= 3, "{list:?}");
        let kind = |k: &str, m: &str, t: &str| item_of(&list, &not_kinds, k, m, t).map(|i| i.kind.clone());
        assert_eq!(kind("low_voltage_outlet", "CR", "low voltage outlet"), Some("card_reader".into()));
        assert_eq!(kind("fire_alarm_device", "", "Electromagnetic door holder, flush"), Some("door_holder".into()));
        assert_eq!(kind("device", "", "DOOR POSITION SWITCH"), Some("door_contact".into()));
        assert_eq!(kind("receptacle", "", "duplex receptacle DOOR HOLD OPEN"), None);
        assert_eq!(kind("switch", "DC", ""), None);
        assert_eq!(kind("smoke_detector", "S", "area smoke detector"), None);
    }
}
