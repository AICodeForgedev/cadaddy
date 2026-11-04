//! The alternatives as model scenarios and RFIs (Q6154 (a), (e)): each proposed alternative's
//! pieces in the model on their own layer (hidden until shown; showing one hides the design's
//! pieces it replaces), its RFI drafted onto the RFI list (a review item of kind `rfi`), and the
//! decision that makes it the design: its pieces replace the design's and the router and the
//! atom builder keep its method on every rebuild.

use std::collections::{BTreeMap, HashMap, HashSet};

use cadaddy_doc::props::{self, Prop, Props, Source};
use cadaddy_doc::{Common, Drawing, EntityKind, Handle, Space};
use cadaddy_walk3d::{BuildingSpec, Rfi, RfiKind, RfiStatus};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::super::*;
use super::{Alternative, Inputs, Rules, price, scope, sketch};
use crate::{Result, Session};

/// Where the scenarios live in the drawing.
const STATE_FILE: &str = "valueeng.json";
/// The prop that names a scenario piece's kind (not `atom`: no takeoff, count or check sees it).
pub(crate) const VE_ATOM: &str = "ve_atom";
/// Pieces one scenario holds at most.
const MAX_PIECES: usize = 400_000;

/// A proposed alternative as the study found it.
pub(crate) struct Built {
    pub alt: Alternative,
    pub tier: String,
    pub items: Vec<scope::Item>,
    pub atoms: Vec<(super::super::mepatoms::Atom, String, String, String)>,
    pub value: Value,
}

/// One scenario as the drawing keeps it.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct Sc {
    pub id: String,
    pub name: String,
    pub tier: String,
    pub kind: String,
    /// The cable it uses (`NM`, `SER`, `MC`).
    pub cable: String,
    /// Branch circuits: the read circuit tags' ids; and each item's level and label.
    pub circuits: Vec<String>,
    pub labels: Vec<(String, String)>,
    /// Feeders: their routes (the runs' source).
    pub feeders: Vec<String>,
    /// The design's pieces it replaces, its own pieces (hex handles), its layer.
    pub replaces: Vec<String>,
    pub pieces: Vec<String>,
    pub layer: String,
    pub shown: bool,
    pub rfi: String,
    /// `approved`, `rejected`, `chosen`, or empty.
    pub decision: String,
    pub by: String,
    pub note: String,
    pub adopted: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct State {
    pub scenarios: Vec<Sc>,
}

pub(crate) fn state(d: &Drawing) -> State {
    d.embedded.get(STATE_FILE).and_then(|b| serde_json::from_slice(b).ok()).unwrap_or_default()
}

fn save(d: &mut Drawing, st: &State) {
    if let Ok(b) = serde_json::to_vec(st) {
        d.embedded.insert(STATE_FILE.into(), std::sync::Arc::new(b));
    }
}

fn handle_of(hex: &str) -> Option<Handle> {
    u64::from_str_radix(hex.trim(), 16).ok().map(Handle)
}

fn hash(t: &str) -> u32 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in t.bytes() {
        h = (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3);
    }
    h as u32
}

/// An alternative's RFI id: the same alternative keeps its id from one study to the next.
pub(crate) fn rfi_id(alt_id: &str) -> String {
    format!("RFI-V{:08x}", hash(alt_id))
}

fn set_visible(d: &mut Drawing, hexes: &[String], on: bool) {
    for h in hexes.iter().filter_map(|x| handle_of(x)) {
        let _ = d.modify_entity(h, |e| e.common.visible = on);
    }
}

fn layer_on(d: &mut Drawing, name: &str, on: bool) {
    if let Some(l) = d.layers.iter_mut().find(|l| l.name.eq_ignore_ascii_case(name)) {
        l.on = on;
        l.frozen = false;
    }
}

/// The scenarios into the drawing: the old (not adopted) ones' pieces removed, each proposed
/// alternative's pieces added on its own hidden layer, the decisions kept.
pub(crate) fn store(s: &mut Session, built: &[Built]) -> Result<Value> {
    let (sp, _) = super::super::bspec::current(s, super::CMD)?;
    let upf = if sp.units_per_foot.is_finite() && sp.units_per_foot > 0.0 { sp.units_per_foot } else { 12.0 };
    let d = s.doc_mut()?;
    let old = state(d);
    let mut st = State::default();
    for o in &old.scenarios {
        if o.adopted {
            st.scenarios.push(o.clone());
            continue;
        }
        if o.shown {
            set_visible(d, &o.replaces, true);
        }
        for h in o.pieces.iter().filter_map(|x| handle_of(x)) {
            d.remove_entity(h);
        }
    }
    let mut out = serde_json::Map::new();
    for b in built {
        if st.scenarios.iter().any(|x| x.id == b.alt.id && x.adopted) {
            continue;
        }
        let prev = old.scenarios.iter().find(|x| x.id == b.alt.id);
        let layer = format!("VE-{}", b.alt.id.to_ascii_uppercase());
        if d.layer(&layer).is_none() {
            d.layers.push(cadaddy_doc::Layer {
                name: layer.as_str().into(),
                color: cadaddy_doc::color::Color::Index(1),
                description: format!("value engineering scenario: {}", b.alt.name),
                ..cadaddy_doc::Layer::default()
            });
        }
        layer_on(d, &layer, false);
        let system = if b.alt.circuit == "feeder" { super::super::gearbuild::FEEDER_SYSTEM } else { super::super::conduitroute::BRANCH_SYSTEM };
        let mut pieces = Vec::new();
        for (a, level, circuit, of) in b.atoms.iter().take(MAX_PIECES) {
            let mesh = super::super::mepatoms::build::body(a, upf);
            if mesh.faces.is_empty() {
                continue;
            }
            let mut ps = Props::default();
            ps.set(VE_ATOM, Prop::new(a.kind, Source::computed()));
            ps.set("item", Prop::new(a.item.clone(), Source::computed()));
            ps.set("type", Prop::new(a.ty.clone(), Source::computed()));
            ps.set("size", Prop::new(a.size.clone(), Source::computed()));
            if matches!(a.kind, "cable" | "conduit" | "wire") {
                ps.set("length_ft", Prop::new(format!("{:.4}", a.length_ft), Source::computed()).with_unit("ft"));
            }
            ps.set("level", Prop::new(level.clone(), Source::computed()));
            ps.set("circuit", Prop::new(circuit.clone(), Source::computed()));
            ps.set("system", Prop::new(system, Source::computed()));
            ps.set("scenario", Prop::new(b.alt.id.clone(), Source::computed()));
            ps.set("of", Prop::new(of.clone(), Source::computed()));
            if !a.source.is_empty() {
                ps.set("source_rule", Prop::new(a.source.clone(), Source::computed()));
            }
            for (k, v, _) in &a.extra {
                ps.set(k, Prop::new(v.clone(), Source::computed()));
            }
            let xdata = props::write(&[], &ps);
            if let Ok(h) = d.add(&Space::Model, Common { layer: layer.as_str().into(), xdata: xdata.into(), ..Common::default() }, EntityKind::Mesh(mesh)) {
                pieces.push(h.hex());
            }
        }
        let mut circuits: Vec<String> = Vec::new();
        for it in b.items.iter().filter(|i| i.kind == "branch") {
            // The circuits as read: a homerun's members, a split part's circuit.
            let ids: Vec<String> = if it.members.is_empty() {
                vec![sp.tags.iter().find(|t| t.id == it.id).and_then(|t| t.fields.get("split_of")).map_or_else(|| it.id.clone(), |f| f.value.clone())]
            } else {
                sp.tags
                    .iter()
                    .filter(|t| {
                        t.kind == "circuit"
                            && t.level.eq_ignore_ascii_case(&it.level)
                            && !t.fields.contains_key("circuits")
                            && it.members.iter().any(|(l, _)| t.fields.get("circuit").is_some_and(|f| f.value.trim() == l))
                    })
                    .map(|t| t.fields.get("split_of").map_or_else(|| t.id.clone(), |f| f.value.clone()))
                    .collect()
            };
            for id in ids {
                if !circuits.contains(&id) {
                    circuits.push(id);
                }
            }
        }
        let sc = Sc {
            id: b.alt.id.clone(),
            name: b.alt.name.clone(),
            tier: b.tier.clone(),
            kind: b.alt.circuit.clone(),
            cable: b.alt.cable.clone(),
            circuits,
            labels: b.items.iter().filter(|i| i.kind == "branch").map(|i| (i.level.clone(), i.label.clone())).collect(),
            feeders: b
                .items
                .iter()
                .filter(|i| i.kind == "feeder")
                .flat_map(|i| i.runs.iter().filter_map(|r| sp.mep.runs.get(*r)).map(|r| r.source_sheet.clone()))
                .collect::<std::collections::BTreeSet<_>>()
                .into_iter()
                .collect(),
            replaces: b.items.iter().flat_map(|i| i.design.iter().map(|h| h.hex())).collect::<std::collections::BTreeSet<_>>().into_iter().collect(),
            pieces,
            layer: layer.clone(),
            shown: false,
            rfi: b.value["rfi"]["id"].as_str().unwrap_or_default().to_string(),
            decision: prev.map(|p| p.decision.clone()).unwrap_or_default(),
            by: prev.map(|p| p.by.clone()).unwrap_or_default(),
            note: prev.map(|p| p.note.clone()).unwrap_or_default(),
            adopted: false,
        };
        out.insert(sc.id.clone(), json!({"layer": sc.layer, "pieces": sc.pieces.len(), "replaces": sc.replaces.len(), "shown": false, "decision": sc.decision, "show": format!("ve.scenario {{id: \"{}\", show: true}}", sc.id)}));
        st.scenarios.push(sc);
    }
    save(d, &st);
    Ok(Value::Object(out))
}

/// Drafts each NEEDS RFI alternative's RFI onto the spec's list (kept with its status and
/// answer from one study to the next), its sketch written when a folder is given.
pub(crate) fn draft_rfis(s: &mut Session, sp: &BuildingSpec, rules: &Rules, inp: &Inputs, built: &mut [Built], mp: &price::Manpower) -> Result<BTreeMap<String, Value>> {
    let upf = if sp.units_per_foot.is_finite() && sp.units_per_foot > 0.0 { sp.units_per_foot } else { 12.0 };
    let mut sp = sp.clone();
    let mut out = BTreeMap::new();
    let mut changed = false;
    for b in built.iter_mut().filter(|b| b.tier == "needs_rfi") {
        let v = &b.value;
        let id = rfi_id(&b.alt.id);
        // Where: the level with the most of its runs, their extent there.
        let mut by_level: BTreeMap<String, usize> = BTreeMap::new();
        for it in &b.items {
            *by_level.entry(it.level.clone()).or_default() += 1;
        }
        let level = by_level.iter().max_by_key(|(_, n)| **n).map(|(l, _)| l.clone()).unwrap_or_default();
        let pts: Vec<[f64; 2]> = b
            .items
            .iter()
            .filter(|i| i.level == level)
            .flat_map(|i| i.runs.iter().filter_map(|r| sp.mep.runs.get(*r)).flat_map(|r| r.points.iter().copied()))
            .filter(|p| p.iter().all(|x| x.is_finite()))
            .take(200_000)
            .collect();
        let extent = (!pts.is_empty()).then(|| {
            let (mut lo, mut hi) = ([f64::INFINITY; 2], [f64::NEG_INFINITY; 2]);
            for p in &pts {
                lo = [lo[0].min(p[0]), lo[1].min(p[1])];
                hi = [hi[0].max(p[0]), hi[1].max(p[1])];
            }
            [lo, hi]
        });
        let at = extent.map_or([0.0, 0.0], |[a, c]| [(a[0] + c[0]) / 2.0, (a[1] + c[1]) / 2.0]);
        // The question and what it rests on.
        let app = &v["application"];
        let spec_ref = format!("{} §{}", app["section"].as_str().unwrap_or_default(), app["item"].as_str().unwrap_or_default());
        let conflict = format!(
            "Specification {spec_ref} ({}) says: \"{}\" — {} only for this work.",
            app["application"].as_str().unwrap_or_default(),
            app["text"].as_str().filter(|t| !t.is_empty()).unwrap_or("(its text)"),
            v["designed"].as_str().unwrap_or("raceway")
        );
        let facts = &inp.facts;
        let mut building = Vec::new();
        if let Some(c) = &facts.construction_type {
            building.push(format!("Type {} construction ({})", c.value, c.source));
        }
        if let Some(sk) = &facts.sprinklered {
            building.push(format!("{} ({})", if sk.value { "sprinklered" } else { "not sprinklered" }, sk.source));
        }
        if !facts.occupancies.is_empty() {
            building.push(format!("occupancy {}", facts.occupancies.iter().map(|o| o.value.as_str()).collect::<Vec<_>>().join(", ")));
        }
        if let Some(n) = &facts.stories {
            building.push(format!("{} stories", n.value));
        }
        if let Some(e) = &facts.electrical_code {
            building.push(format!("{} in force", e.value));
        }
        let cites: Vec<String> = b.alt.code.iter().map(|c| format!("{}: {}", c.cite, c.says)).collect();
        let excluded: Vec<String> = v["excluded"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|e| {
                let n = e["kept_as_designed"].as_u64().unwrap_or(0);
                format!("{} ({}){}", e["name"].as_str().unwrap_or_default(), e["why"].as_str().unwrap_or_default(), if n > 0 { format!(": {n} kept as designed") } else { String::new() })
            })
            .collect();
        let intent: Vec<String> = v["intent"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|i| i["likely"].as_bool() == Some(true))
            .map(|i| {
                let how = i["met"].as_str().map(|m| format!("met: {m}")).or_else(|| i["risk"].as_str().map(|r| format!("risk: {r}"))).unwrap_or_default();
                format!("{} ({}); {how}", i["reason"].as_str().unwrap_or_default(), i["evidence"].as_str().unwrap_or_default())
            })
            .collect();
        let sv = &v["saves"];
        let savings = if sv.is_object() {
            format!(
                "saves ${:.0} (material ${:.0}, labor ${:.0}) and {:.0} man-hours ({:.1} crew-days at a crew of {} × {} h); {}{}",
                sv["cost"].as_f64().unwrap_or(0.0),
                sv["material"].as_f64().unwrap_or(0.0),
                sv["labor_dollars"].as_f64().unwrap_or(0.0),
                sv["man_hours"].as_f64().unwrap_or(0.0),
                sv["crew_days"].as_f64().unwrap_or(0.0),
                mp.crew_size,
                mp.hours_per_day,
                v["as_designed"]["basis"].as_str().unwrap_or_default(),
                inp.cost_db.as_ref().filter(|d| !d.price_date.is_empty()).map(|d| format!("; prices of {}{}", d.price_date, if d.escalation.is_empty() { String::new() } else { format!(", escalation {}", d.escalation) })).unwrap_or_default()
            )
        } else {
            format!(
                "savings not priced: no price for {}",
                v["unpriced"].as_array().map(|u| u.iter().filter_map(Value::as_str).take(8).collect::<Vec<_>>().join(", ")).unwrap_or_default()
            )
        };
        let q = &v["quantity"];
        let question = format!(
            "May {} be used in place of {} for {} {} ({:.0} ft)?",
            v["alternative_method"].as_str().unwrap_or_default(),
            v["designed"].as_str().unwrap_or_default(),
            q["items"].as_u64().unwrap_or(0),
            if b.alt.circuit == "feeder" { "feeders" } else { "branch circuits" },
            q["feet"].as_f64().unwrap_or(0.0)
        );
        let sketch_path = match &inp.sketches {
            Some(dir) => {
                let path = std::path::Path::new(dir).join(format!("{id}.svg")).to_string_lossy().to_string();
                let d = s.doc()?;
                if std::path::Path::new(&path).exists() && !inp.force {
                    s.echo(format!("value engineering: sketch {path} exists (force: true replaces it)"));
                    None
                } else {
                    match sketch::write(d, &sp, &b.items, &level, &format!("{id}: {}", b.alt.name), &question, &path, upf) {
                        Ok(()) => Some(path),
                        Err(e) => {
                            s.echo(format!("value engineering: sketch {path} not written: {e}"));
                            None
                        }
                    }
                }
            }
            None => None,
        };
        let issue = format!(
            "{question}\nConflict: {conflict}\nCode: {} — the building: {}.\nConditions: {}.\nNot in: {}.\nIntent: {}.\nSavings: {savings}.{}",
            cites.join("; "),
            if building.is_empty() { "(the code sheet not read)".to_string() } else { building.join(", ") },
            b.alt.conditions.join("; "),
            if excluded.is_empty() { "—".to_string() } else { excluded.join("; ") },
            if intent.is_empty() { "no reason for the specified method found in the documents".to_string() } else { intent.join(" | ") },
            sketch_path.as_ref().map(|p| format!("\nSketch: {p}")).unwrap_or_default()
        );
        let prev = sp.rfis.iter().find(|r| r.id == id).cloned();
        let mut sheets = vec![spec_ref.clone()];
        if !facts.sheet.is_empty() {
            sheets.push(facts.sheet.clone());
        }
        let r = Rfi {
            id: id.clone(),
            kind: Some(RfiKind::MethodChoice),
            severity: RfiKind::MethodChoice.severity(),
            level: level.clone(),
            at,
            extent,
            sheets,
            about: b.alt.name.clone(),
            issue: issue.chars().take(6_000).collect(),
            assumed: format!("The model stays as designed; the alternative is model scenario {} (ve.scenario) until this RFI is approved.", b.alt.id),
            proposed: Some(format!("Approve {} where the conditions above hold (ve.decide {{id: \"{id}\", decision: \"approved\"}} once answered)", b.alt.name.to_lowercase())),
            status: prev.as_ref().map_or(RfiStatus::Open, |p| p.status),
            answer: prev.as_ref().and_then(|p| p.answer.clone()),
            sketch: sketch_path.clone(),
        };
        sp.rfis.retain(|x| x.id != id);
        if sp.rfis.len() < cadaddy_walk3d::rfi::MAX_RFIS {
            sp.rfis.push(r.clone());
            changed = true;
        }
        let j = json!({
            "id": id, "status": r.status.name(), "question": question, "conflict": conflict, "citations": b.alt.code,
            "building": building, "intent": intent, "savings": savings, "sketch": sketch_path, "level": level, "extent": extent,
        });
        b.value["rfi"] = j.clone();
        out.insert(b.alt.id.clone(), j);
    }
    // An alternative no longer needing an RFI leaves no open one behind.
    let live: HashSet<String> = built.iter().filter(|b| b.tier == "needs_rfi").map(|b| rfi_id(&b.alt.id)).collect();
    let before = sp.rfis.len();
    sp.rfis.retain(|r| r.kind != Some(RfiKind::MethodChoice) || live.contains(&r.id) || r.status != RfiStatus::Open);
    changed |= sp.rfis.len() != before;
    if changed {
        s.doc_mut()?.building_spec = Some(sp.to_json());
    }
    let _ = rules;
    Ok(out)
}

/// Scenario `id` (or its RFI's): the state's index.
fn find(st: &State, id: &str) -> Option<usize> {
    let id = id.trim();
    st.scenarios.iter().position(|x| x.id.eq_ignore_ascii_case(id) || (!x.rfi.is_empty() && x.rfi.eq_ignore_ascii_case(id)))
}

fn sc_json(x: &Sc) -> Value {
    json!({
        "id": x.id, "name": x.name, "tier": x.tier, "kind": x.kind, "cable": x.cable, "layer": x.layer, "shown": x.shown,
        "pieces": x.pieces.len(), "replaces": x.replaces.len(), "circuits": x.circuits.len(), "feeders": x.feeders.len(),
        "rfi": x.rfi, "decision": x.decision, "adopted": x.adopted,
    })
}

pub(crate) fn run_scenario(s: &mut Session, p: &Value) -> Result<Value> {
    const C: &str = "ve.scenario";
    let Some(id) = str_param(p, "id") else {
        let st = state(s.doc()?);
        return Ok(json!({"scenarios": st.scenarios.iter().map(sc_json).collect::<Vec<_>>()}));
    };
    let d = s.doc_mut()?;
    let mut st = state(d);
    let i = find(&st, id).ok_or_else(|| bad(C, format!("there is no scenario {id} (ve.study makes them)")))?;
    let Some(x) = st.scenarios.get_mut(i) else { return Err(bad(C, "no such scenario")) };
    if let Some(show) = p.get("show").and_then(Value::as_bool) {
        if x.adopted {
            return Err(bad(C, format!("{} is the design now (adopted): nothing to show", x.id)));
        }
        if show != x.shown {
            layer_on(d, &x.layer, show);
            set_visible(d, &x.replaces, !show);
            x.shown = show;
        }
    }
    let out = sc_json(x);
    save(d, &st);
    Ok(out)
}

/// The scenario's pieces become the design's: the design's pieces it replaces removed, its own
/// tagged as built MEP pieces on their system's layers.
fn adopt(d: &mut Drawing, x: &mut Sc) -> usize {
    for h in x.replaces.iter().filter_map(|h| handle_of(h)) {
        d.remove_entity(h);
    }
    let mut n = 0usize;
    for (k, h) in x.pieces.iter().filter_map(|h| handle_of(h)).enumerate() {
        let Some(e) = d.model.get(h) else { continue };
        let ps = props::read(&e.common.xdata);
        let g = |k: &str| ps.get(k).map(Prop::text).unwrap_or_default();
        let (kind, level, system) = (g(VE_ATOM), g("level"), g("system"));
        let mut out = Props::default();
        for (key, prop) in ps.iter() {
            if !matches!(key, "ve_atom" | "scenario" | "of" | "level") {
                out.set(key, prop.clone());
            }
        }
        out.set(super::super::mepatoms::ATOM_KEY, Prop::new(kind.clone(), Source::computed()));
        out.set("adopted_from", Prop::new(format!("value engineering {} ({})", x.id, if x.rfi.is_empty() { x.decision.clone() } else { format!("{} {}", x.rfi, x.decision) }), Source::computed()));
        let mut xd = super::super::mepatoms::build::mep_tags(&level, Handle(0), &system, &kind, &g("size"), &g("item"), &format!("ve:{}/{kind}{k}", x.id));
        xd = props::write(&xd, &out);
        let layer = super::super::mep::mep_layer_name(&system, &level);
        if d.layer(&layer).is_none() {
            d.layers.push(cadaddy_doc::Layer { name: layer.as_str().into(), color: cadaddy_doc::color::Color::Index(6), ..cadaddy_doc::Layer::default() });
        }
        if d
            .modify_entity(h, |e| {
                e.common.xdata = xd.into();
                e.common.layer = layer.as_str().into();
                e.common.visible = true;
            })
            .is_ok()
        {
            n += 1;
        }
    }
    x.adopted = true;
    x.shown = false;
    n
}

pub(crate) fn run_decide(s: &mut Session, p: &Value) -> Result<Value> {
    const C: &str = "ve.decide";
    let id = str_param(p, "id").ok_or_else(|| bad(C, "`id` is required (a scenario's or its RFI's)"))?.to_string();
    let decision = str_param(p, "decision").map(|t| t.trim().to_ascii_lowercase()).ok_or_else(|| bad(C, "`decision` is approved, rejected or chosen"))?;
    if !matches!(decision.as_str(), "approved" | "rejected" | "chosen") {
        return Err(bad(C, "`decision` is approved, rejected or chosen"));
    }
    let by: String = str_param(p, "by").unwrap_or_default().chars().take(200).collect();
    let note: String = str_param(p, "note").unwrap_or_default().chars().take(2_000).collect();
    decide(s, &id, &decision, &by, &note, C)
}

/// Records a decision on a scenario (by its id or its RFI's) and, approved or chosen, adopts it.
pub(crate) fn decide(s: &mut Session, id: &str, decision: &str, by: &str, note: &str, cmd: &str) -> Result<Value> {
    let (mut sp, _) = super::super::bspec::current(s, cmd)?;
    let d = s.doc_mut()?;
    let mut st = state(d);
    let i = find(&st, id).ok_or_else(|| bad(cmd, format!("there is no value engineering scenario {id} (ve.study makes them)")))?;
    let Some(x) = st.scenarios.get_mut(i) else { return Err(bad(cmd, "no such scenario")) };
    if x.adopted {
        return Err(bad(cmd, format!("{} is the design already", x.id)));
    }
    match (x.tier.as_str(), decision) {
        ("needs_rfi", "chosen") => return Err(bad(cmd, format!("{} needs its RFI ({}) approved: only the engineer's approval makes it the design", x.id, x.rfi))),
        ("allowed_now", "approved" | "rejected") if x.rfi.is_empty() => {
            return Err(bad(cmd, format!("{} is allowed now (no RFI): `chosen` makes it the design", x.id)));
        }
        _ => {}
    }
    x.decision = decision.to_string();
    x.by = by.to_string();
    x.note = note.to_string();
    let mut adopted = Value::Null;
    if matches!(decision, "approved" | "chosen") {
        if x.shown {
            layer_on(d, &x.layer, false);
        }
        let n = adopt(d, x);
        // The model's counts see the change.
        super::super::mepatoms::reindex(d);
        let why = if x.rfi.is_empty() { format!("value engineering {}: chosen (allowed now)", x.id) } else { format!("{} approved", x.rfi) };
        // The circuits keep the method on every rebuild (the tags' raceway, the router's choice).
        let mut tags = 0usize;
        for t in sp.tags.iter_mut().filter(|t| t.kind == "circuit") {
            let label = t.fields.get("circuit").map(|f| f.value.trim().to_string()).unwrap_or_default();
            if x.labels.iter().any(|(l, c)| l.eq_ignore_ascii_case(&t.level) && *c == label) || x.circuits.contains(&t.id) {
                t.set("conduit", x.cable.clone(), "rule");
                t.set("value_engineering", why.clone(), "rule");
                tags += 1;
            }
        }
        adopted = json!({"pieces": n, "circuit_tags": tags, "feeders": x.feeders.len(), "removed": x.replaces.len()});
    }
    let rfi = x.rfi.clone();
    let out_sc = sc_json(x);
    save(d, &st);
    if !rfi.is_empty()
        && let Some(r) = sp.rfis.iter_mut().find(|r| r.id == rfi)
    {
        r.status = match decision {
            "approved" => RfiStatus::Approved,
            "rejected" => RfiStatus::Rejected,
            _ => r.status,
        };
        let answer = [by, note].iter().filter(|t| !t.is_empty()).copied().collect::<Vec<_>>().join(": ");
        if !answer.is_empty() {
            r.answer = Some(answer);
        }
    }
    s.doc_mut()?.building_spec = Some(sp.to_json());
    Ok(json!({"id": out_sc["id"], "decision": decision, "adopted": adopted, "rfi": rfi, "scenario": out_sc}))
}

/// The branch circuits an adopted scenario changed: read circuit id → (cable, why).
pub(crate) fn approved_circuits(d: &Drawing) -> HashMap<String, (String, String)> {
    let mut out = HashMap::new();
    for x in state(d).scenarios.iter().filter(|x| x.adopted && x.kind == "branch") {
        let why = if x.rfi.is_empty() { format!("value engineering {}: chosen (allowed now)", x.id) } else { format!("{} approved ({})", x.rfi, x.id) };
        for c in &x.circuits {
            out.insert(c.clone(), (x.cable.clone(), why.clone()));
        }
    }
    out
}

/// The feeder routes an adopted scenario changed: route source → (cable, why).
pub(crate) fn approved_feeders(d: &Drawing) -> HashMap<String, (String, String)> {
    let mut out = HashMap::new();
    for x in state(d).scenarios.iter().filter(|x| x.adopted && x.kind == "feeder") {
        let why = if x.rfi.is_empty() { format!("value engineering {}: chosen (allowed now)", x.id) } else { format!("{} approved ({})", x.rfi, x.id) };
        for f in &x.feeders {
            out.insert(f.clone(), (x.cable.clone(), why.clone()));
        }
    }
    out
}

/// A scenario as the takeoff counts it: the design's pieces it replaces, its own pieces.
pub(crate) fn takeoff_view(d: &Drawing, id: &str) -> Option<(HashSet<Handle>, HashSet<Handle>)> {
    let st = state(d);
    let x = st.scenarios.get(find(&st, id)?)?;
    if x.adopted {
        return Some((HashSet::new(), HashSet::new()));
    }
    Some((x.replaces.iter().filter_map(|h| handle_of(h)).collect(), x.pieces.iter().filter_map(|h| handle_of(h)).collect()))
}
