//! A clipped viewport and its clip boundary act as one object, as in AutoCAD: selecting either
//! for MOVE, COPY, ROTATE, SCALE, MIRROR, ARRAY or ERASE takes both, a copy gets its own pair,
//! and reshaping the boundary (grips, STRETCH, PEDIT) refits the viewport to it.
//!
//! The link is [`Viewport::clip`]: the viewport points at a paper-space entity in its layout.

use std::collections::HashMap;
use std::sync::Arc;

use cadaddy_doc::{Drawing, Entity, EntityKind, Handle, Viewport};
use cadaddy_geom::{Bounds2, Mat3};

use crate::{Result, Session};

/// The next free viewport id (ids are unique across layouts; 1 is a sheet's own view).
pub(crate) fn next_viewport_id(d: &Drawing) -> u32 {
    d.layouts
        .iter()
        .flat_map(|l| l.entities.iter())
        .filter_map(|e| if let EntityKind::Viewport(v) = &e.kind { Some(v.id) } else { None })
        .max()
        .unwrap_or(1)
        .max(1)
        .saturating_add(1)
}

/// Every (viewport, boundary) pair in the drawing's layouts. A boundary must sit in its
/// viewport's layout; a self-reference or a dangling handle is no pair.
fn clip_pairs(d: &Drawing) -> Vec<(Handle, Handle)> {
    let mut out = Vec::new();
    for l in &d.layouts {
        for e in l.entities.iter() {
            if let EntityKind::Viewport(v) = &e.kind
                && let Some(b) = v.clip.filter(|b| *b != e.handle)
                && l.entities.get(b).is_some()
            {
                out.push((e.handle, b));
            }
        }
    }
    out
}

/// `hs` plus the partner of each (the boundary of a clipped viewport, the viewport a boundary
/// clips), in order and without repeats.
pub(crate) fn with_partners(d: &Drawing, hs: &[Handle]) -> Vec<Handle> {
    let pairs = clip_pairs(d);
    if pairs.is_empty() {
        return hs.to_vec();
    }
    let partner: HashMap<Handle, Handle> = pairs.iter().flat_map(|&(v, b)| [(v, b), (b, v)]).collect();
    let mut out = hs.to_vec();
    let mut seen: std::collections::HashSet<Handle> = hs.iter().copied().collect();
    for h in hs {
        if let Some(p) = partner.get(h)
            && seen.insert(*p)
        {
            out.push(*p);
        }
    }
    out
}

/// After `transform_entities` applied `m` (`moved` holds each source and its result: itself, or
/// its copy): copied viewports get their own id and clip their copied boundary, and a clipped
/// viewport turned with its boundary turns its view with it (AutoCAD's VPROTATEASSOC default),
/// so model space stays put inside the boundary. The rectangle follows in [`after_command`].
pub(crate) fn after_transform(d: &mut Drawing, moved: &[(Handle, Handle)], m: &Mat3, copy: bool) -> Result<()> {
    let new_of: HashMap<Handle, Handle> = moved.iter().copied().collect();
    let turn = if m.is_mirroring() { 0.0 } else { m.rotation() };
    for &(old, new) in moved {
        let Some(EntityKind::Viewport(v)) = d.entity(new).map(|e| &e.kind) else { continue };
        let boundary = v.clip.filter(|b| *b != old).and_then(|b| new_of.get(&b).copied());
        let id = if copy { Some(next_viewport_id(d)) } else { None };
        if boundary.is_none() && id.is_none() {
            continue;
        }
        d.modify_entity(new, |e| {
            if let EntityKind::Viewport(v) = &mut e.kind {
                if let Some(id) = id {
                    v.id = id;
                }
                if let Some(b) = boundary {
                    v.clip = Some(b);
                    if turn.is_finite() {
                        v.twist = cadaddy_geom::norm_angle(v.twist + turn);
                    }
                }
            }
        })?;
    }
    Ok(())
}

/// The extents of a clip boundary's outline (`None`: not a closed shape usable as one).
pub(crate) fn boundary_extents(d: &Drawing, e: &Entity) -> Option<Bounds2> {
    let b = Bounds2::from_points(cadaddy_render::clip::boundary_polygon(&e.kind)?);
    // The exact extents (the tessellated outline runs just inside curves).
    let eb = cadaddy_doc::entity_bounds(d, e, 0);
    Some(if eb.is_empty() || !eb.min.is_finite() || !eb.max.is_finite() { b } else { b.union(&eb) })
}

/// `v` with its rectangle on `b` (the boundary's extents), model space staying where it is on
/// the sheet. `None` when nothing changes or the result would not be usable.
pub(crate) fn fitted(v: &Viewport, b: Bounds2) -> Option<Viewport> {
    let k = v.view_height / v.height;
    let c = b.center();
    let view_center = v.to_model(c)?;
    let view_height = b.height() * k;
    if !(view_height.is_finite() && view_height > 0.0 && b.width() > 0.0 && c.is_finite()) {
        return None;
    }
    let tol = 1e-9 * (1.0 + v.width.abs().max(v.height.abs()) + v.center.xy().len());
    if v.center.xy().near(c, tol) && (v.width - b.width()).abs() <= tol && (v.height - b.height()).abs() <= tol {
        return None;
    }
    let mut out = v.clone();
    out.center = c.to3(v.center.z);
    out.width = b.width();
    out.height = b.height();
    out.view_center = view_center;
    out.view_height = view_height;
    Some(out)
}

/// The post-command pass: a clipped viewport whose boundary (or itself) changed is refitted to
/// the boundary's extents, so a reshaped or moved boundary carries its viewport along.
pub(crate) fn after_command(s: &mut Session, before: Option<&Arc<Drawing>>) {
    let Some(before) = before else { return };
    let Ok(st) = s.state_mut() else { return };
    if Arc::ptr_eq(before, &st.doc) {
        return;
    }
    let d = &st.doc;
    let changed = |h: Handle| match (before.entity(h), d.entity(h)) {
        (Some(a), Some(b)) => !Arc::ptr_eq(a, b) && a.kind != b.kind,
        _ => true,
    };
    let fits: Vec<(Handle, Viewport)> = clip_pairs(d)
        .into_iter()
        .filter(|&(v, b)| changed(v) || changed(b))
        .filter_map(|(vh, bh)| {
            let EntityKind::Viewport(v) = &d.entity(vh)?.kind else { return None };
            let b = boundary_extents(d, d.entity(bh)?)?;
            Some((vh, fitted(v, b)?))
        })
        .collect();
    if fits.is_empty() {
        return;
    }
    let d = Arc::make_mut(&mut st.doc);
    for (h, v) in fits {
        let _ = d.modify_entity(h, |e| e.kind = EntityKind::Viewport(v));
    }
}
