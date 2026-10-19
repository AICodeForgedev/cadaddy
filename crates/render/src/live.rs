//! Live sections: a section plane with `live` on cuts solids, meshes and 3D lines in 3D views.
//! A solid is shown intersected with the kept half-space, so its cut faces are real faces (drawn
//! in the cut colour); meshes and lines are clipped.

use cadaddy_doc::{BoolOp, Drawing, EntityKind, SectionPlane, Solid3d, Space, combine, half_space};
use cadaddy_geom::Vec3;

/// The live section planes of a space.
pub fn live_sections(d: &Drawing, space: &Space) -> Vec<SectionPlane> {
    let Some(store) = d.space(space) else { return Vec::new() };
    store
        .iter()
        .filter(|e| d.is_visible(e))
        .filter_map(|e| match &e.kind {
            EntityKind::SectionPlane(sp) if sp.live && sp.is_valid() => Some(sp.clone()),
            _ => None,
        })
        .take(16)
        .collect()
}

/// The solid as the sections leave it, and the first leaf that is a cut (polygons from it or
/// later leaves are cut faces). `None`: untouched; `Some` with an empty solid: all cut away.
pub fn cut_solid(so: &Solid3d, planes: &[SectionPlane]) -> Option<(Solid3d, u32)> {
    if planes.is_empty() {
        return None;
    }
    let pr = so.props(so.mesh_tol());
    let corners = (0..8).map(|i| {
        Vec3::new(
            if i & 1 == 0 { pr.min.x } else { pr.max.x },
            if i & 2 == 0 { pr.min.y } else { pr.max.y },
            if i & 4 == 0 { pr.min.z } else { pr.max.z },
        )
    });
    let corners: Vec<Vec3> = corners.collect();
    let cutting: Vec<&SectionPlane> = planes.iter().filter(|sp| corners.iter().any(|c| sp.side(*c) < 0.0)).collect();
    if cutting.is_empty() {
        return None;
    }
    let size = (pr.max - pr.min).len() + cutting.iter().map(|sp| (sp.origin - pr.min).len()).fold(0.0, f64::max) + 1.0;
    let first = u32::try_from(so.leaves().len()).unwrap_or(u32::MAX);
    let mut s = so.clone();
    for sp in cutting {
        // `half_space(p, n)` keeps the side `n` points away from.
        s = combine(s, half_space(sp.origin, sp.normal * -1.0, size), BoolOp::Intersect);
    }
    Some((s, first))
}

/// A convex polygon clipped to the kept side of every plane.
pub fn clip_polygon(mut pts: Vec<Vec3>, planes: &[SectionPlane]) -> Vec<Vec3> {
    for sp in planes {
        if pts.is_empty() {
            break;
        }
        let n = pts.len();
        let mut out = Vec::with_capacity(n + 2);
        for i in 0..n {
            let (a, b) = (pts[i], pts[(i + 1) % n]);
            let (da, db) = (sp.side(a), sp.side(b));
            if da >= 0.0 {
                out.push(a);
            }
            if (da >= 0.0) != (db >= 0.0) {
                out.push(a + (b - a) * (da / (da - db)));
            }
        }
        pts = out;
    }
    pts
}

/// A segment clipped to the kept side of every plane.
pub fn clip_segment(mut a: Vec3, mut b: Vec3, planes: &[SectionPlane]) -> Option<(Vec3, Vec3)> {
    for sp in planes {
        let (da, db) = (sp.side(a), sp.side(b));
        match (da >= 0.0, db >= 0.0) {
            (true, true) => {}
            (false, false) => return None,
            (true, false) => b = a + (b - a) * (da / (da - db)),
            (false, true) => a = a + (b - a) * (da / (da - db)),
        }
    }
    Some((a, b))
}

#[cfg(test)]
mod tests {
    use super::*;
    use cadaddy_doc::SolidShape;

    #[test]
    fn a_live_section_halves_a_box_and_caps_it() {
        let b = Solid3d::new(SolidShape::Box { length: 4.0, width: 4.0, height: 4.0 });
        let sp = SectionPlane {
            origin: Vec3::new(1.0, 2.0, 2.0),
            normal: Vec3::new(1.0, 0.0, 0.0),
            width: 10.0,
            height: 10.0,
            live: true,
            name: String::new(),
        };
        let (cut, first) = cut_solid(&b, std::slice::from_ref(&sp)).unwrap();
        assert!((cut.props(0.01).volume - 48.0).abs() < 1e-9, "{}", cut.props(0.01).volume);
        let polys = cut.polys(0.01);
        // The cut face: the 4 × 4 square at x = 1.
        let cap: f64 =
            polys.iter().filter(|p| cadaddy_doc::tag_leaf(p.tag) >= first).map(|p| cadaddy_doc::bsp::mass(std::slice::from_ref(p)).1).sum();
        assert!((cap - 16.0).abs() < 1e-9, "{cap}");
        // A box wholly on the kept side is left alone.
        let far = SectionPlane { origin: Vec3::new(-5.0, 0.0, 0.0), ..sp.clone() };
        assert!(cut_solid(&b, &[far]).is_none());
        assert_eq!(clip_polygon(vec![Vec3::ZERO, Vec3::new(2.0, 0.0, 0.0), Vec3::new(2.0, 2.0, 0.0)], std::slice::from_ref(&sp)).len(), 4);
        assert!(clip_segment(Vec3::ZERO, Vec3::new(0.5, 0.0, 0.0), &[sp]).is_none());
    }
}
