//! The associative model's state saved with the drawing, and restored when it is opened (Q6378).
//!
//! What makes a model associative lives in the session: the settings it builds again from
//! ([`WallGen`]: the building spec or the walk rules, whether it follows the plan, the slab
//! thickness, the wall layers) and the record of the plan it was built from with a fingerprint of
//! every object the build made ([`ModelRecord`]). A drawing opened again had neither: the model
//! was there, but `model.update` found nothing behind and `model.revise` said there was no model
//! until `model.build` ran once more.
//!
//! Saving writes both into the drawing, as one embedded file ([`FILE`]); opening reads it back, so
//! the reopened model follows its plan (or not) as it did before saving, and a revision compares
//! with the plan it was built from. A drawing saved before this file existed but built by
//! `model.build` (built elements and a stored building spec) is restored from its spec (and the
//! build record the spec keeps, when it has one): the plan as opened is taken for the one it was
//! built from, and its rooms are read at the first revision.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use cadaddy_doc::{Drawing, Handle};
use cadaddy_geom::Vec2;
use cadaddy_walk3d::Rules;
use serde::{Deserialize, Serialize};

use super::{ModelRecord, PlanItem, TagKey, made_of, plan_items};
use crate::Session;
use crate::cmd::wallgen::{self, WallGen};

/// The embedded file the associative state is saved in.
pub(crate) const FILE: &str = "model/associative.json";
/// The format's version.
const VERSION: u32 = 1;
/// A larger saved state isn't read (hostile file).
const MAX_BYTES: usize = 512 * 1024 * 1024;
/// Most plan items, generated objects and hand edits read back.
const MAX_PLAN: usize = super::MAX_ITEMS + super::MAX_ROOM_PAGES * 200;
const MAX_MADE: usize = 4_000_000;
const MAX_EDITS: usize = 1_000_000;
/// Longest wall-layer filter read back.
const MAX_LAYERS_TEXT: usize = 4096;
/// The plan item kinds a record holds.
const KINDS: [&str; 4] = ["wall", "door", "window", "room"];

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(default)]
struct Saved {
    version: u32,
    /// Built again when the plan changes.
    auto: bool,
    slab_ft: f64,
    layers: Option<String>,
    /// Built from the drawing's building spec (`model.build`); else from `rules` (WALLGEN).
    from_spec: bool,
    rules: Option<Rules>,
    /// The plan the model was built from and what that build made.
    record: Option<SavedRecord>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(default)]
struct SavedRecord {
    plan: Vec<SavedItem>,
    made: Vec<SavedMade>,
    hand_deleted: Vec<TagKey>,
    hand_edited: Vec<String>,
    rooms_unread: bool,
}

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(default)]
struct SavedItem {
    kind: String,
    handle: String,
    layer: String,
    name: Option<String>,
    signature: String,
    at: [f64; 2],
}

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(default)]
struct SavedMade {
    handle: String,
    fingerprint: u64,
    tag: Option<TagKey>,
}

impl Saved {
    fn of(g: &WallGen, rec: Option<&ModelRecord>) -> Self {
        Saved {
            version: VERSION,
            auto: g.auto,
            slab_ft: g.slab_ft,
            layers: g.layers.clone(),
            from_spec: g.spec.is_some(),
            rules: if g.spec.is_some() { None } else { Some(g.rules.clone()) },
            record: rec.map(SavedRecord::of),
        }
    }
}

impl SavedRecord {
    fn of(r: &ModelRecord) -> Self {
        let plan = r
            .plan
            .iter()
            .map(|i| SavedItem {
                kind: i.kind.to_string(),
                handle: i.handle.hex(),
                layer: i.layer.clone(),
                name: i.name.clone(),
                signature: i.signature.clone(),
                at: [i.at.x, i.at.y],
            })
            .collect();
        // In handle order, so the same record writes the same bytes (an unchanged state is not
        // written again).
        let mut made: Vec<(&Handle, &(u64, Option<TagKey>))> = r.made.iter().collect();
        made.sort_by_key(|(h, _)| h.0);
        let made = made.into_iter().map(|(h, (f, k))| SavedMade { handle: h.hex(), fingerprint: *f, tag: k.clone() }).collect();
        let mut edited: Vec<Handle> = r.hand_edited.iter().copied().collect();
        edited.sort_by_key(|h| h.0);
        SavedRecord {
            plan,
            made,
            hand_deleted: r.hand_deleted.clone(),
            hand_edited: edited.into_iter().map(|h| h.hex()).collect(),
            rooms_unread: r.rooms_unread,
        }
    }

    /// The record, against the drawing as opened (`opened`: its generated objects). When not one
    /// saved fingerprint matches an object the drawing holds (a file format that doesn't keep the
    /// built objects exactly, or renumbers them), the objects as opened are the build's: every
    /// one would otherwise be taken for an edit by hand.
    fn into_record(self, opened: HashMap<Handle, (u64, Option<TagKey>)>) -> ModelRecord {
        let plan: Vec<PlanItem> = self
            .plan
            .into_iter()
            .take(MAX_PLAN)
            .filter_map(|i| {
                let kind = KINDS.iter().copied().find(|k| *k == i.kind)?;
                let at = Vec2::new(i.at[0], i.at[1]);
                at.is_finite().then_some(())?;
                Some(PlanItem { kind, handle: Handle::parse_hex(&i.handle)?, layer: i.layer, name: i.name, signature: i.signature, at })
            })
            .collect();
        let saved: HashMap<Handle, (u64, Option<TagKey>)> =
            self.made.into_iter().take(MAX_MADE).filter_map(|m| Some((Handle::parse_hex(&m.handle)?, (m.fingerprint, m.tag)))).collect();
        let kept = saved.iter().any(|(h, (f, _))| opened.get(h).is_some_and(|(o, _)| o == f));
        let (made, hand_edited, hand_deleted) = if kept || opened.is_empty() {
            let edited: HashSet<Handle> = self.hand_edited.iter().take(MAX_EDITS).filter_map(|h| Handle::parse_hex(h)).collect();
            (saved, edited, self.hand_deleted.into_iter().take(MAX_EDITS).collect())
        } else {
            (opened, HashSet::new(), Vec::new())
        };
        ModelRecord { plan, made, hand_deleted, hand_edited, built: None, rooms_unread: self.rooms_unread }
    }
}

/// The settings saved, against the drawing as opened: none when they can't be restored (a spec
/// build whose spec can no longer be read, a WALLGEN with no rules saved).
fn settings(sv: &Saved, d: &Drawing) -> Option<WallGen> {
    let slab_ft = Some(sv.slab_ft).filter(|t| t.is_finite() && (0.0..=10.0).contains(t)).unwrap_or(wallgen::SLAB_FT);
    let layers = sv.layers.clone().filter(|l| l.len() <= MAX_LAYERS_TEXT && !l.trim().is_empty());
    if sv.from_spec {
        let sp = crate::cmd::bspec::stored(d).ok().flatten()?;
        Some(WallGen { rules: sp.to_rules(), auto: sv.auto, slab_ft, layers, spec: Some(sp) })
    } else {
        Some(WallGen { rules: sv.rules.clone()?, auto: sv.auto, slab_ft, layers, spec: None })
    }
}

/// A drawing saved before its associative state was, with built elements and a stored building
/// spec: a `model.build` model (the command that builds from a stored spec), following its plan
/// as `model.build` does by default, with the slab its build record keeps (drawings saved before
/// that record have none: the default slab).
fn settings_of_build(d: &Drawing) -> Option<WallGen> {
    let sp = crate::cmd::bspec::stored(d).ok().flatten()?;
    let rec = crate::cmd::buildrec::read(d).unwrap_or_default();
    let slab_ft = rec.switches.get("slab_ft").and_then(serde_json::Value::as_f64).filter(|t| t.is_finite() && (0.0..=10.0).contains(t));
    Some(WallGen { rules: sp.to_rules(), auto: true, slab_ft: slab_ft.unwrap_or(wallgen::SLAB_FT), layers: None, spec: Some(sp) })
}

/// Before the active drawing is saved (SAVE, SAVEAS, and a program writing the drawing to a file
/// itself, as `cadaddy-cli run --save` does): its associative state goes into it (or out of it,
/// when it has none any more). Nothing is written when the drawing already holds the same state.
pub fn before_save(s: &mut Session) {
    let Ok(st) = s.state() else { return };
    // In the Block Editor the drawing open is the editor's working copy.
    if st.block_edit.is_some() {
        return;
    }
    let bytes = match &st.wallgen {
        Some(g) => match serde_json::to_vec(&Saved::of(g, st.model_records.for_state(&st.doc))) {
            Ok(b) => Some(b),
            Err(_) => return,
        },
        None => None,
    };
    let old = Arc::clone(&st.doc);
    match (&bytes, old.embedded.get(FILE)) {
        (None, None) => return,
        (Some(b), Some(c)) if c.as_slice() == b.as_slice() => return,
        _ => {}
    }
    let Ok(d) = s.doc_mut() else { return };
    match bytes {
        Some(b) => {
            d.embedded.insert(FILE.into(), Arc::new(b));
        }
        None => {
            d.embedded.remove(FILE);
        }
    }
    // No edit of the model or its plan: the new drawing state is behind, and comes from the
    // build, as the one before.
    if let Ok(st) = s.state_mut() {
        let now = Arc::clone(&st.doc);
        wallgen::carry_state(st, &old, &now);
    }
}

/// After a drawing was opened (the active one): an associative model saved with it follows its
/// plan again, and `model.revise` compares with the plan it was built from. A plan changed
/// since the build (saved with a model that didn't follow it) leaves a following model behind.
pub(crate) fn after_open(s: &mut Session) {
    let Ok(st) = s.state() else { return };
    let d = Arc::clone(&st.doc);
    if st.wallgen.is_some() || !d.model.iter().any(|e| wallgen::made_as(e).is_some()) {
        return;
    }
    let saved: Option<Saved> = d.embedded.get(FILE).filter(|b| b.len() <= MAX_BYTES).and_then(|b| serde_json::from_slice(b).ok());
    let Some(g) = (match &saved {
        Some(sv) => settings(sv, &d),
        None => settings_of_build(&d),
    }) else {
        return;
    };
    let now = plan_items(&d, &g);
    let mut rec = match saved.and_then(|sv| sv.record) {
        Some(r) => r.into_record(made_of(&d)),
        // No record saved: the plan as opened is the one built from; its rooms aren't traced on
        // opening (minutes on a whole building), but read at the first revision.
        None => ModelRecord { plan: now.clone(), made: made_of(&d), rooms_unread: true, ..ModelRecord::default() },
    };
    let key = |i: &PlanItem| (i.kind, i.handle, i.signature.clone());
    let built_from: Vec<_> = rec.plan.iter().filter(|i| i.kind != "room").map(key).collect();
    let same_plan = built_from == now.iter().map(key).collect::<Vec<_>>();
    if same_plan {
        rec.built = Some(super::BuiltDrawing(Arc::clone(&d)));
    }
    let auto = g.auto;
    let Ok(st) = s.state_mut() else { return };
    st.model_records.push(rec, &d, false);
    st.wallgen = Some(g);
    if auto && !same_plan {
        wallgen::mark_behind(st, &d);
    }
}
