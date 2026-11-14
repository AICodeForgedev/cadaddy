//! A model element's circuit links (Q6501), derived from its own `circuit` and `panel`: the
//! panel it is fed from (`fed_from`, and the panel element `fed_from_handle`), its breaker
//! (`breaker_handle`) and its schedule row's breaker (`breaker`, `breaker_type`, `gfci_breaker`,
//! `poles`, `circuit_description`).
//!
//! - **A circuit or panel set** (`bim.props.set`, an override applied, re-applied or taken off):
//!   every derived link is cleared and derived again from the new values. A circuit set without
//!   its panel takes the panel its label names (`LP-3,5`: panel `LP`). A person's own value of a
//!   derived key is theirs and stays.
//! - **A copy** (COPY, ARRAY, paste and the rest) keeps none of its source's links: the source's
//!   box, what it connects to and what feeds it are the source's place (the routing gives the
//!   copy its own), and its circuit links are derived for the copy from its own circuit.
//!
//! Nothing here knows one job: the panels are the model's equipment by tag, the rows the spec's
//! schedules, the breakers the model's.

use std::collections::{BTreeMap, HashMap, HashSet};

use cadaddy_doc::props::{self, Prop, Props, Source, SourceKind};
use cadaddy_doc::{Drawing, Handle};

use super::propfill::WithNote;

/// The keys the links are derived from.
pub(crate) const INPUTS: [&str; 2] = ["circuit", "panel"];
/// The links derived from them: cleared and derived again when either changes.
pub(crate) const DERIVED: [&str; 8] =
    ["fed_from", "fed_from_handle", "breaker_handle", "breaker", "breaker_type", "gfci_breaker", "poles", "circuit_description"];
/// Where a built element is: its box, what it connects to and what feeds it. A copy's are its
/// source's, so it keeps none of them.
pub(crate) const PLACE: [&str; 3] = ["box", "connects", "fed_by"];
/// Most elements one call relinks.
const MAX_RELINK: usize = 200_000;
/// Most model objects looked at for the panels and breakers.
const MAX_SCAN: usize = 2_000_000;

/// Whether `key` is one the links are derived from.
pub(crate) fn is_input(key: &str) -> bool {
    INPUTS.iter().any(|k| k.eq_ignore_ascii_case(key.trim()))
}

/// What the links are found by: the model's equipment by tag (upper case) and each one's tag,
/// its breakers by (panel tag, first circuit number), and the spec's equipment and schedules.
struct Lookup {
    gear: HashMap<String, Handle>,
    tag_of: HashMap<Handle, String>,
    breakers: HashMap<(String, u32), Handle>,
    spec: Option<cadaddy_walk3d::BuildingSpec>,
}

fn lookup(d: &Drawing) -> Lookup {
    let mut gear = HashMap::new();
    let mut tag_of = HashMap::new();
    let mut breakers = HashMap::new();
    for e in d.model.iter().take(MAX_SCAN) {
        if super::breakers::is_breaker(e) {
            let ps = props::read(&e.common.xdata);
            let text = |k: &str| ps.get(k).filter(|p| p.has_value()).map(|p| p.text().trim().to_string()).unwrap_or_default();
            let (panel, circuit) = (text("panel"), text("circuit"));
            if let Some(n) = super::breakers::circuit_numbers(&circuit, &panel).first() {
                breakers.entry((panel.to_ascii_uppercase(), *n)).or_insert(e.handle);
            }
            continue;
        }
        if super::elemprops::element_kind(e).as_deref() == Some("equipment")
            && let Some(t) = super::bimlook::made_tag(e, "tag").map(str::trim).filter(|t| !t.is_empty())
        {
            gear.entry(t.to_ascii_uppercase()).or_insert(e.handle);
            tag_of.insert(e.handle, t.to_string());
        }
    }
    // An unreadable spec has no schedules here: the links to elements are still derived.
    let spec = super::bspec::stored(d).ok().flatten();
    Lookup { gear, tag_of, breakers, spec }
}

/// Whether a store has anything this module derives from or links by.
fn has_links(ps: &Props) -> bool {
    INPUTS.iter().chain(DERIVED.iter()).chain(PLACE.iter()).any(|k| ps.get(k).is_some())
}

/// Derive the circuit links of elements `hs` again from their own circuit and panel. Returns
/// how many elements changed.
pub(crate) fn relink(d: &mut Drawing, hs: &[Handle]) -> usize {
    relink_with(d, hs, &[])
}

/// [`relink`], the copies' place links (`PLACE`) dropped first.
fn relink_with(d: &mut Drawing, hs: &[Handle], copies: &[Handle]) -> usize {
    let todo: Vec<Handle> =
        hs.iter().copied().filter(|h| d.model.get(*h).is_some_and(|e| has_links(&props::read(&e.common.xdata)))).take(MAX_RELINK).collect();
    if todo.is_empty() {
        return 0;
    }
    let copies: HashSet<Handle> = copies.iter().copied().collect();
    let lk = lookup(d);
    let equipment: HashMap<String, &cadaddy_walk3d::MepEquipment> =
        lk.spec.iter().flat_map(|sp| sp.mep.equipment.iter()).map(|e| (e.tag.trim().to_ascii_uppercase(), e)).collect();
    let schedules: HashMap<String, &cadaddy_walk3d::PanelSchedule> =
        lk.spec.iter().flat_map(|sp| sp.mep.panel_schedules.iter()).map(|p| (p.panel.trim().to_ascii_uppercase(), p)).collect();
    let mut changes: Vec<(Handle, Props)> = Vec::new();
    for h in &todo {
        let Some(e) = d.model.get(*h) else { continue };
        let before = props::read(&e.common.xdata);
        let mut ps = before.clone();
        if copies.contains(h) {
            for k in PLACE {
                ps.remove(k);
            }
        }
        // A typical room's device: the room it was stamped into, as the fill reads it.
        let stamped = super::bimlook::made_tag(e, "devid")
            .and_then(|id| lk.spec.as_ref().and_then(|sp| sp.tags.iter().find(|t| t.id == id)))
            .and_then(|t| t.fields.get("room"))
            .map(|f| f.value.trim().to_string())
            .filter(|r| !r.is_empty());
        derive(d, &mut ps, stamped.as_deref(), &lk, &equipment, &schedules);
        if ps != before {
            changes.push((*h, ps));
        }
    }
    let n = changes.len();
    for (h, ps) in changes {
        let _ = d.modify_entity(h, |e| e.common.xdata = props::write(&e.common.xdata, &ps).into());
    }
    n
}

/// One store's links derived again from its circuit and panel.
fn derive(
    d: &Drawing,
    ps: &mut Props,
    stamped: Option<&str>,
    lk: &Lookup,
    equipment: &HashMap<String, &cadaddy_walk3d::MepEquipment>,
    schedules: &HashMap<String, &cadaddy_walk3d::PanelSchedule>,
) {
    let value = |ps: &Props, k: &str| ps.get(k).filter(|p| p.has_value()).map(|p| p.text().trim().to_string()).filter(|t| !t.is_empty());
    let mine = |ps: &Props, k: &str| ps.get(k).is_some_and(|p| p.source.kind == SourceKind::User);
    let Some(circuit) = value(ps, "circuit") else {
        // No circuit: nothing to derive links from, and none of the old ones holds.
        if value(ps, "panel").is_none() {
            for k in DERIVED {
                if !mine(ps, k) {
                    ps.remove(k);
                }
            }
        }
        return;
    };
    // The panel its label names, unless the panel is a person's own.
    if !mine(ps, "panel")
        && let Some(p) = super::propfill::panel_of(&circuit)
        && value(ps, "panel").is_none_or(|old| !old.eq_ignore_ascii_case(&p))
    {
        ps.set("panel", Prop::new(p, Source::computed()).with_note("the panel its circuit names"));
    }
    for k in DERIVED {
        if !mine(ps, k) {
            ps.remove(k);
        }
    }
    let panel = value(ps, "panel");
    if !mine(ps, "fed_from") {
        let text = match &panel {
            Some(p) => format!("panel {p}, circuit {circuit}"),
            None => format!("circuit {circuit}"),
        };
        ps.set("fed_from", Prop::new(text, Source::computed()).with_note("from its circuit and panel"));
    }
    let mut tally = BTreeMap::new();
    super::propfill::circuit_links(d, ps, stamped, &lk.gear, equipment, schedules, &mut tally);
    // Its breaker: the one its panel holds for the circuit's first number.
    if mine(ps, "breaker_handle") {
        return;
    }
    let at = ps.get("fed_from_handle").and_then(|p| Handle::parse_hex(p.text().trim())).and_then(|h| lk.tag_of.get(&h).cloned()).or(panel);
    let Some(tag) = at else { return };
    let first = super::breakers::circuit_numbers(&circuit, &tag).first().copied().or_else(|| {
        // A room panel's own tag (`U-101`) is not on its label (`U-2`): the label's letter.
        let letter = super::propfill::panel_of(&circuit).unwrap_or_default();
        super::breakers::circuit_numbers(&circuit, &letter).first().copied()
    });
    if let Some(b) = first.and_then(|n| lk.breakers.get(&(tag.to_ascii_uppercase(), n))) {
        ps.set("breaker_handle", Prop::new(b.hex(), Source::computed()).with_note(&format!("its circuit's breaker: panel {tag} circuit {circuit}")));
    }
}

/// After objects were copied (`pairs`: source, copy): the model elements among the copies drop
/// their source's place links and take their own circuit links. Returns how many changed.
pub(crate) fn after_copy(d: &mut Drawing, pairs: &[(Handle, Handle)]) -> usize {
    let copies: Vec<Handle> = pairs.iter().filter(|(a, b)| a != b).map(|(_, b)| *b).filter(|h| d.model.get(*h).is_some()).collect();
    if copies.is_empty() {
        return 0;
    }
    relink_with(d, &copies, &copies)
}
