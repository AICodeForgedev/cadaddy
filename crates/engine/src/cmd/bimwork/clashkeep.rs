//! An approved clash fix kept (Q6381): the move `bim.clash.fix` makes is a model edit that
//! persists, so `model.clashes`, `bim.clash` and every rebuild agree with it.
//!
//! - A piece of a spec element (a duct or pipe run by its segment or fittings, a wiring chain by
//!   the runs its sticks lay along, an air terminal, equipment) moves its spec element: one
//!   `move` edit per element in the spec's MEP edits ([`mepedit`]), which every build applies
//!   again. The element keeps the documents' value beside the value installed, with the clash
//!   and the fix as the reason. Every piece built from those elements moves in place now, and
//!   when the model was built from the spec as it stood, the build record takes the edited spec
//!   as built (a `model.build` keeps the model; a rebuild lays it out the same).
//! - Any other piece a build made (a device, a riser's cut piece) is kept as an override
//!   (`override.add` move), which every build applies again.
//! - A piece no build made (a drawn object, a reference model's) is the drawing itself: the move
//!   is the edit.

use std::collections::{BTreeMap, BTreeSet};

use cadaddy_doc::{Handle, RawTag, props};
use cadaddy_geom::{Mat4, Vec2, Vec3};
use cadaddy_walk3d::MepRun;
use serde_json::{Value, json};

use super::El;
use crate::cmd::{bad, mepedit};
use crate::{Result, Session};

/// Pieces of other spec elements' chains moved with the mover's at most.
const MAX_PIECES: usize = 100_000;

/// Keep the fix that moves `mover` (and `movers`, the pieces moving with it) by `mv` (drawing
/// units): what keeps it, and the pieces moved.
pub(super) fn keep(s: &mut Session, cmd: &str, els: &[El], mover: &El, movers: &[Handle], mv: Vec3, reason: &str, by: &str) -> Result<Value> {
    let upf = super::units_per_foot(s);
    let inch = upf / 12.0;
    let (sp, stored) = crate::cmd::bspec::current(s, cmd)?;
    let mut v = sp.clone();
    let _ = mepedit::apply(&mut v);
    let d = s.doc()?;
    // The spec elements the mover was built from.
    let mut ats: BTreeSet<mepedit::At> = BTreeSet::new();
    if stored {
        if mover.run.is_empty() {
            if let Some(at) = d.model.get(mover.handle).and_then(|e| mepedit::built_from(&v, e)) {
                ats.insert(at);
            }
        } else {
            // A wiring chain's atoms: the runs its sticks lay along.
            let mut path: Vec<(Vec2, Vec2)> = Vec::new();
            let mut reach = inch;
            for e in movers.iter().filter_map(|h| d.model.get(*h)) {
                let ps = props::read(&e.common.xdata);
                if let Some((a, b)) = super::super::mepatoms::build::stick_ends(&ps) {
                    path.push((Vec2::new(a.x, a.y), Vec2::new(b.x, b.y)));
                    reach = reach.max((super::super::mepatoms::build::body_of(&ps) / 2.0 + 1.0) * inch);
                }
            }
            ats.extend(mepedit::runs_along(&v, &mover.system, &mover.level, &path, reach));
        }
    }
    let mut moved: BTreeSet<Handle> = movers.iter().copied().collect();
    if ats.is_empty() {
        // Not a spec element's: an override for what a build made, the drawing itself otherwise.
        let made: Vec<Handle> =
            moved.iter().copied().filter(|h| d.model.get(*h).is_some_and(|e| super::super::revise3d::tag_key(e).is_some())).collect();
        let mut ids = Vec::new();
        for h in &made {
            let r =
                s.execute("override.add", &json!({"handle": h.hex(), "kind": "move", "delta": [mv.x, mv.y, mv.z], "author": by, "note": reason}))?;
            if let Some(id) = r["override"]["id"].as_str() {
                ids.push(id.to_string());
            }
        }
        let rest: Vec<Handle> = moved.iter().copied().filter(|h| !made.contains(h)).collect();
        translate(s, &rest, mv, &BTreeMap::new())?;
        return Ok(
            json!({"kept_by": if made.is_empty() { "the drawing" } else { "overrides" }, "overrides": ids, "edits": [], "pieces_moved": moved.len()}),
        );
    }
    // Every piece built from those elements moves with them: a run's segment and fittings (by
    // the run they name) and the wiring chains stitched from its runs.
    // By the run's name (a riser's on every storey it passes; any other run's on its level).
    let bases: BTreeMap<String, bool> =
        ats.iter().filter_map(|a| mepedit::run_at(&v, *a)).map(|r| (super::super::mep::run_base(r), r.top_ft.is_some())).collect();
    for el in els.iter().filter(|e| e.system.eq_ignore_ascii_case(&mover.system)) {
        if moved.len() >= MAX_PIECES {
            break;
        }
        let base = if el.run.is_empty() {
            d.model
                .get(el.handle)
                .and_then(|e| super::super::bimlook::made_tag(e, "mepid"))
                .and_then(|id| id.rsplit_once('/'))
                .map(|(b, _)| b.to_string())
        } else {
            el.run.split_once('#').map(|(b, _)| b.to_string())
        };
        if base.and_then(|b| bases.get(&b).copied()).is_some_and(|riser| riser || el.level == mover.level) {
            moved.insert(el.handle);
        }
    }
    let in_sync = super::super::buildrec::spec_in_sync(d);
    let ats: Vec<mepedit::At> = ats.into_iter().collect();
    let mut sp = sp;
    let ids = mepedit::keep_moves(&mut sp, &v, &ats, Some([mv.x, mv.y]), Some(mv.z / upf), reason, by)
        .map_err(|why| bad(cmd, format!("the fix can't be kept in the building spec: {why}")))?;
    // The runs as the builds see them now: what their pieces are named by and carry.
    let mut after = sp.clone();
    let _ = mepedit::apply(&mut after);
    let runs: BTreeMap<String, (String, MepRun)> = ats
        .iter()
        .filter_map(|a| {
            let r = mepedit::run_at(&after, *a)?.clone();
            Some((super::super::mep::run_base(mepedit::run_at(&v, *a)?), (super::super::mep::run_base(&r), r)))
        })
        .collect();
    s.doc_mut()?.building_spec = Some(sp.to_json()).into();
    let moved: Vec<Handle> = moved.into_iter().collect();
    translate(s, &moved, mv, &runs)?;
    if in_sync {
        super::super::buildrec::spec_kept_in_model(s);
    }
    Ok(json!({"kept_by": "building spec", "edits": ids, "overrides": [], "pieces_moved": moved.len()}))
}

/// Move the pieces `hs` by `mv` as a build would lay them from the moved elements: their bodies,
/// a stick's ends, the run they name (`runs`: by the old name, its new name and the run as the
/// builds see it now) and on a run's segment the run's corrections.
fn translate(s: &mut Session, hs: &[Handle], mv: Vec3, runs: &BTreeMap<String, (String, MepRun)>) -> Result<()> {
    let m = Mat4 { m: [[1.0, 0.0, 0.0, mv.x], [0.0, 1.0, 0.0, mv.y], [0.0, 0.0, 1.0, mv.z], [0.0, 0.0, 0.0, 1.0]] };
    let renamed = |id: &str| {
        let cut = id.find(['/', '#']).unwrap_or(id.len());
        let (base, rest) = id.split_at(cut);
        runs.get(base).map(|(new, r)| (format!("{new}{rest}"), r))
    };
    for h in hs {
        s.doc_mut()?.modify_entity(*h, |e| {
            e.kind.transform3(&m);
            let mut segment_of: Option<&MepRun> = None;
            let mut x: Vec<RawTag> = e.common.xdata.to_vec();
            let part = x.iter().any(|t| t.code == 1000 && t.value == "part:segment");
            for t in x.iter_mut().filter(|t| t.code == 1000) {
                if let Some((new, r)) = t.value.strip_prefix("mepid:").and_then(renamed) {
                    t.value = format!("mepid:{new}");
                    if part {
                        segment_of = Some(r);
                    }
                }
            }
            if props::has_store(&x) {
                let mut ps = props::read(&x);
                let mut changed = false;
                if let Some((a, b)) = super::super::mepatoms::build::stick_ends(&ps) {
                    for (k, p) in [("start", a + mv), ("end", b + mv)] {
                        let src = ps.get(k).map_or_else(props::Source::computed, |q| q.source.clone());
                        changed |= ps.set(k, props::Prop::new(format!("{:.4},{:.4},{:.4}", p.x, p.y, p.z), src));
                    }
                }
                if let Some((new, _)) = ps.get("run").map(props::Prop::text).as_deref().and_then(renamed) {
                    let src = ps.get("run").map_or_else(props::Source::computed, |q| q.source.clone());
                    changed |= ps.set("run", props::Prop::new(new, src));
                }
                if changed {
                    x = props::write(&x, &ps);
                }
            }
            if let Some(r) = segment_of {
                x = super::super::provenance::with_run_corrections(x, r);
            }
            e.common.xdata = x.into();
        })?;
    }
    Ok(())
}
