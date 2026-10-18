//! TXTEXP: explode text into geometry: each TEXT, MTEXT or attribute definition becomes the
//! polylines it is drawn with (stroke-font strokes as open polylines, outline-font glyphs as
//! closed outlines), on its layer with its properties, at its elevation.

use cadaddy_doc::{Entity, EntityKind, Handle, LwPolyline};
use cadaddy_geom::{PolyVertex, Vec2};
use serde_json::{Value, json};

use super::super::machines::SelectRun;
use super::super::*;
use crate::{Result, Session};

/// Most polylines one call makes.
const MAX_PIECES: usize = 2_000_000;

/// The polylines `k` is drawn with, and its elevation; `None` for other objects.
fn pieces(d: &cadaddy_doc::Drawing, k: &EntityKind) -> Option<(Vec<Vec<Vec2>>, f64)> {
    match k {
        EntityKind::Text(t) => Some((cadaddy_render::place_text_entity(d, t, &t.value).0.outlines(), t.insert.z)),
        EntityKind::AttDef(a) => Some((cadaddy_render::place_text_entity(d, &a.text, &a.text.value).0.outlines(), a.text.insert.z)),
        EntityKind::MText(t) => Some((cadaddy_render::layout_mtext_entity(d, t).strokes, t.insert.z)),
        _ => None,
    }
}

fn polyline(pts: &[Vec2], elevation: f64) -> Option<LwPolyline> {
    let mut pts: Vec<Vec2> = pts.iter().copied().filter(|p| p.is_finite()).collect();
    pts.dedup_by(|a, b| a.near(*b, 1e-12));
    // A contour that ends where it started is a closed outline.
    let closed = pts.len() > 2 && pts.first().zip(pts.last()).is_some_and(|(a, b)| a.near(*b, 1e-9));
    if closed {
        pts.pop();
    }
    (pts.len() >= 2).then(|| LwPolyline {
        vertices: pts.into_iter().map(|p| PolyVertex { p, ..PolyVertex::default() }).collect(),
        closed,
        const_width: 0.0,
        elevation: if elevation.is_finite() { elevation } else { 0.0 },
        plinegen: false,
    })
}

pub(crate) fn run(s: &mut Session, p: &Value) -> Result<Value> {
    let hs = targets(s, p)?;
    let space = s.space();
    let mut work: Vec<(Handle, Entity, Vec<LwPolyline>)> = Vec::new();
    let mut total = 0usize;
    {
        let d = s.doc()?;
        for h in hs {
            if super::super::curves::is_locked(s, h) {
                continue;
            }
            let Some(e) = d.entity(h) else { continue };
            let Some((lines, z)) = pieces(d, &e.kind) else { continue };
            let polys: Vec<LwPolyline> = lines.iter().filter_map(|l| polyline(l, z)).collect();
            total = total.saturating_add(polys.len());
            if total > MAX_PIECES {
                return Err(other("The text is too large to explode."));
            }
            work.push((h, (**e).clone(), polys));
        }
    }
    if work.is_empty() {
        return Err(other("No text, mtext or attribute definitions selected."));
    }
    let d = s.doc_mut()?;
    let mut out = Vec::new();
    for (h, e, polys) in work {
        d.remove_entity(h);
        for pl in polys {
            let ne = Entity { handle: d.new_handle(), common: e.common.clone(), kind: EntityKind::LwPolyline(pl) };
            out.push(ne.handle.hex());
            if let Some(st) = d.space_mut(&space) {
                st.push(ne);
            }
        }
    }
    s.set_selection(Vec::new());
    Ok(json!({ "handles": out }))
}

pub(crate) fn interactive() -> SelectRun {
    SelectRun::new("txtexp", "TXTEXP")
}

fn other(m: impl Into<String>) -> crate::EngineError {
    crate::EngineError::Other(m.into())
}
