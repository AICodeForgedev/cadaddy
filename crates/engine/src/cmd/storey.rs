//! A level view is its storey (Q6660).
//!
//! A level's 3D view (the BIM window's Floor and Room views, `model.render {level}`) shows what
//! stands between the level's floor and the floor above, from every level: a riser or a stack
//! built as one run from its bottom level to the roof is cut at the floor above (its stub through
//! that floor at most), and the piece of a run from another level that stands in this storey is
//! drawn, carried onto this level's plan. Nothing outside the storey is drawn, so a floor picture
//! reads as a floor whether the build cut its risers at each floor (Q6493) or kept them whole.
//!
//! The floor above is the lowest level over this one's floor (a mezzanine is a partial floor
//! inside the storey under it, no storey of its own: it is skipped); the highest level has no
//! floor above and is not cut at the top. The level's own objects under its floor (its slab,
//! what runs under it) stay: only the cut at the top applies to them. Objects of a level at the
//! same height as this one (an enlarged or partial plan of the same floor) and objects filed
//! under no level are never brought in.

use std::borrow::Cow;

use cadaddy_doc::{Drawing, Entity, EntityKind, Mesh, MeshFace};
use cadaddy_geom::{Mat3, Vec2, Vec3};

/// The most objects scanned for a level's stacking move.
const MAX_SCAN: usize = 2_000_000;
/// Heights closer than this (feet) are one floor.
const SAME_FT: f64 = 0.5;
/// How far (feet) an object of another level must reach into the storey to stand in it: a run
/// cut at a floor's face, or a wall whose top meets the floor, only touches it.
const REACH_FT: f64 = 1.0 / 12.0;
/// A floor, roof deck or ceiling of another level whose top is this near (feet) the floor above
/// is that floor's slab (the lid of the storey): left out, as a level view leaves out the slab
/// over its level to see in.
const LID_FT: f64 = 0.5;
/// The parts that close a storey from above.
const LIDS: [&str; 3] = ["floor", "roof", "ceiling"];

/// A level's storey: its floor and the floor above (drawing units), and how each other level's
/// objects carry onto its plan.
#[derive(Clone, Debug)]
pub(crate) struct Storey {
    /// The level (its name in the spec).
    pub level: String,
    /// Its floor's height.
    pub lo: f64,
    /// The floor above's height (none for the highest level).
    pub hi: Option<f64>,
    /// [`REACH_FT`] and [`LID_FT`] in drawing units.
    reach: f64,
    lid: f64,
    /// The levels at this one's height (upper case): the same floor's other plans.
    same: Vec<String>,
    /// Each level's stacking move ([`super::bimlook::stack_offsets`]), upper case.
    offsets: Vec<(String, Vec2)>,
    /// Where this level's own objects stand against the stacked building: its stacking move less
    /// what the build already moved them by.
    view_move: Vec2,
}

/// What a storey does to an object.
#[derive(Debug)]
pub(crate) enum Cut<'a> {
    /// Nothing of it stands in the storey.
    Gone,
    /// It, or the part of it in the storey.
    Kept(Cow<'a, Entity>),
}

impl Storey {
    /// The storey of `level` in `d`'s building spec (none without a spec or that level, or when
    /// the level has no finite height).
    pub fn of(d: &Drawing, level: &str) -> Option<Storey> {
        let sp = super::bspec::stored(d).ok().flatten()?;
        let lv = sp.level(level)?;
        if !lv.elevation_ft.is_finite() {
            return None;
        }
        let upf = if sp.units_per_foot.is_finite() && sp.units_per_foot > 0.0 { sp.units_per_foot } else { 12.0 };
        let elev = lv.elevation_ft;
        let hi = sp
            .levels
            .iter()
            .filter(|l| l.elevation_ft.is_finite() && l.elevation_ft > elev + SAME_FT && !cadaddy_walk3d::levels::is_mezzanine(&l.name))
            .map(|l| l.elevation_ft)
            .min_by(f64::total_cmp)
            .map(|z| z * upf);
        let same = sp
            .levels
            .iter()
            .filter(|l| !l.name.eq_ignore_ascii_case(&lv.name) && l.elevation_ft.is_finite() && (l.elevation_ft - elev).abs() <= SAME_FT)
            .map(|l| l.name.to_ascii_uppercase())
            .collect();
        let offsets: Vec<(String, Vec2)> = super::bimlook::stack_offsets(d).into_iter().map(|(n, o)| (n.to_ascii_uppercase(), o)).collect();
        let own_off = offsets.iter().find(|(n, _)| n.eq_ignore_ascii_case(&lv.name)).map_or(Vec2::ZERO, |o| o.1);
        // What the build moved this level's objects by (a stacked build moved them all alike).
        let built_by = d
            .model
            .iter()
            .take(MAX_SCAN)
            .find(|e| super::wallgen::made_as(e).is_some() && super::bspec::made_level(e).is_some_and(|l| l.eq_ignore_ascii_case(&lv.name)))
            .map_or(Vec2::ZERO, |e| super::bspec::stacked_by(e));
        Some(Storey {
            level: lv.name.clone(),
            lo: elev * upf,
            hi,
            reach: REACH_FT * upf,
            lid: LID_FT * upf,
            same,
            offsets,
            view_move: own_off - built_by,
        })
    }

    /// Whether `e`, filed under `level` (another level than this one), stands in the storey:
    /// it reaches more than [`REACH_FT`] above the floor and below the floor above. Never one of
    /// no level or of a level at this one's height, nor the slab or roof deck under the floor
    /// above ([`LID_FT`]).
    pub fn stands_in(&self, e: &Entity, level: &str) -> bool {
        let up = level.trim().to_ascii_uppercase();
        if up.is_empty() || up.eq_ignore_ascii_case(&self.level) || self.same.contains(&up) {
            return false;
        }
        let Some((z0, z1)) = z_range(e) else { return false };
        if !(z1 > self.lo + self.reach && self.hi.is_none_or(|hi| z0 < hi - self.reach)) {
            return false;
        }
        let lid = |hi: f64| z1 >= hi - self.lid && super::wallgen::made_as(e).is_some_and(|p| LIDS.iter().any(|l| p.eq_ignore_ascii_case(l)));
        !self.hi.is_some_and(lid)
    }

    /// The plan move that carries `e`, filed under `level`, onto this level's plan (each level
    /// built over its own plan's place); none when they share one.
    pub fn carry_move(&self, e: &Entity, level: &str) -> Option<Vec2> {
        let off = self.offsets.iter().find(|(n, _)| n.eq_ignore_ascii_case(level)).map_or(Vec2::ZERO, |o| o.1);
        let o = off - super::bspec::stacked_by(e) - self.view_move;
        (o.is_finite() && o.len() > 1e-6).then_some(o)
    }

    /// The part of `e` the storey shows: cut at the floor above, and, for an object of another
    /// level (`carried`), at this level's floor too.
    pub fn cut<'a>(&self, e: &'a Entity, carried: bool) -> Cut<'a> {
        let lo = carried.then_some(self.lo);
        let hi = self.hi;
        let Some((z0, z1)) = z_range(e) else { return Cut::Kept(Cow::Borrowed(e)) };
        if lo.is_some_and(|lo| z1 <= lo) || hi.is_some_and(|hi| z0 >= hi) {
            return Cut::Gone;
        }
        if lo.is_none_or(|lo| z0 >= lo) && hi.is_none_or(|hi| z1 <= hi) {
            return Cut::Kept(Cow::Borrowed(e));
        }
        match clip_kind(&e.kind, lo, hi) {
            Some(Some(kind)) => {
                let mut c = e.clone();
                c.kind = kind;
                Cut::Kept(Cow::Owned(c))
            }
            Some(None) => Cut::Gone,
            // A kind with no cut (its height is one point): whole while that point is in.
            None => Cut::Kept(Cow::Borrowed(e)),
        }
    }

    /// [`Self::cut`] for an object of another level filed under `from` (none: this level's own),
    /// then, when `carry` (the level drawn on its own plan, not stacked), [`Self::carry_move`].
    pub fn place<'a>(&self, e: &'a Entity, from: Option<&str>, carry: bool) -> Cut<'a> {
        match self.cut(e, from.is_some()) {
            Cut::Gone => Cut::Gone,
            Cut::Kept(c) => match from.filter(|_| carry).and_then(|l| self.carry_move(e, l)) {
                Some(o) => {
                    let mut c = c.into_owned();
                    c.kind.transform(&Mat3::translate(o));
                    Cut::Kept(Cow::Owned(c))
                }
                None => Cut::Kept(c),
            },
        }
    }
}

/// The lowest and highest height of `e` (drawing units), for the kinds a build makes: meshes,
/// lines and 3D polylines; a block, point or polyline at its one height. None for the others.
pub(crate) fn z_range(e: &Entity) -> Option<(f64, f64)> {
    let span = |zs: &mut dyn Iterator<Item = f64>| {
        let (lo, hi) = zs.filter(|z| z.is_finite()).fold((f64::INFINITY, f64::NEG_INFINITY), |(a, b), z| (a.min(z), b.max(z)));
        (lo <= hi).then_some((lo, hi))
    };
    match &e.kind {
        EntityKind::Mesh(m) => span(&mut m.vertices.iter().map(|v| v.z)),
        EntityKind::Line(l) => span(&mut [l.a.z, l.b.z].into_iter()),
        EntityKind::Polyline3d(p) => span(&mut p.points.iter().map(|v| v.z)),
        EntityKind::Point(p) => span(&mut std::iter::once(p.p.z)),
        EntityKind::Insert(i) => span(&mut std::iter::once(i.insert.z)),
        EntityKind::LwPolyline(p) => span(&mut std::iter::once(p.elevation)),
        _ => None,
    }
}

/// `kind` cut to `lo..=hi`: `Some(Some(_))` the part left, `Some(None)` nothing left, `None` a
/// kind not cut (its one height decides: see [`Storey::cut`]).
fn clip_kind(kind: &EntityKind, lo: Option<f64>, hi: Option<f64>) -> Option<Option<EntityKind>> {
    match kind {
        EntityKind::Mesh(m) => Some(clip_mesh(m, lo, hi).map(EntityKind::Mesh)),
        EntityKind::Line(l) => Some(clip_seg(l.a, l.b, lo, hi).map(|(a, b)| EntityKind::Line(cadaddy_doc::Line { a, b }))),
        EntityKind::Polyline3d(p) => {
            Some(clip_polyline(&p.points, p.closed, lo, hi).map(|points| EntityKind::Polyline3d(cadaddy_doc::Polyline3d { points, closed: false })))
        }
        _ => None,
    }
}

/// Signed distances of a height into the band's two planes (≥ 0 inside).
fn planes(lo: Option<f64>, hi: Option<f64>) -> Vec<(f64, f64)> {
    // (sign, c): sign * z - c >= 0 keeps.
    let mut v = Vec::with_capacity(2);
    if let Some(lo) = lo {
        v.push((1.0, lo));
    }
    if let Some(hi) = hi {
        v.push((-1.0, -hi));
    }
    v
}

fn clip_seg(mut a: Vec3, mut b: Vec3, lo: Option<f64>, hi: Option<f64>) -> Option<(Vec3, Vec3)> {
    for (s, c) in planes(lo, hi) {
        let (da, db) = (s * a.z - c, s * b.z - c);
        match (da >= 0.0, db >= 0.0) {
            (false, false) => return None,
            (true, false) => b = a + (b - a) * (da / (da - db)),
            (false, true) => a = a + (b - a) * (da / (da - db)),
            _ => {}
        }
    }
    Some((a, b))
}

/// The longest run of a 3D polyline inside the band (a run stands in a storey once).
fn clip_polyline(pts: &[Vec3], closed: bool, lo: Option<f64>, hi: Option<f64>) -> Option<Vec<Vec3>> {
    let n = pts.len();
    let segs = if closed && n > 2 { n } else { n.saturating_sub(1) };
    let mut runs: Vec<Vec<Vec3>> = Vec::new();
    for k in 0..segs {
        let (Some(a), Some(b)) = (pts.get(k), pts.get((k + 1) % n.max(1))) else { continue };
        let Some((p, q)) = clip_seg(*a, *b, lo, hi) else { continue };
        match runs.last_mut() {
            Some(r) if r.last().is_some_and(|l| (*l - p).len() < 1e-9) => r.push(q),
            _ => runs.push(vec![p, q]),
        }
    }
    let len = |r: &Vec<Vec3>| r.windows(2).map(|w| w.first().zip(w.get(1)).map_or(0.0, |(a, b)| (*a - *b).len())).sum::<f64>();
    runs.into_iter().max_by(|a, b| len(a).total_cmp(&len(b)))
}

/// A mesh's faces cut to the band; the cut left open (a section). None when nothing is left.
fn clip_mesh(m: &Mesh, lo: Option<f64>, hi: Option<f64>) -> Option<Mesh> {
    let planes = planes(lo, hi);
    let mut vertices: Vec<Vec3> = Vec::with_capacity(m.vertices.len());
    let mut kept: std::collections::HashMap<u32, u32> = std::collections::HashMap::new();
    let mut faces: Vec<MeshFace> = Vec::with_capacity(m.faces.len());
    for f in &m.faces {
        // (point, its index in the mesh when it is one of the mesh's, the edge leaving it hidden).
        let mut poly: Vec<(Vec3, Option<u32>, bool)> = Vec::with_capacity(f.v.len() + 2);
        for (k, &i) in f.v.iter().enumerate() {
            let Some(p) = m.vertices.get(i as usize) else { continue };
            let hidden = k < 64 && f.hidden & (1u64 << k) != 0;
            poly.push((*p, Some(i), hidden));
        }
        if poly.len() < 3 {
            if poly.iter().all(|(p, _, _)| planes.iter().all(|(s, c)| s * p.z - c >= 0.0)) {
                let mut v = Vec::with_capacity(poly.len());
                for (p, i, _) in &poly {
                    v.push(index_of(&mut vertices, &mut kept, *p, *i));
                }
                faces.push(MeshFace { v, hidden: f.hidden });
            }
            continue;
        }
        for (s, c) in &planes {
            if poly.is_empty() {
                break;
            }
            let input = std::mem::take(&mut poly);
            let n = input.len();
            for k in 0..n {
                let (Some(&(q, qi, qh)), Some(&(p, _, _))) = (input.get(k), input.get((k + 1) % n)) else { continue };
                let (dq, dp) = (s * q.z - c, s * p.z - c);
                let at = || q + (p - q) * (dq / (dq - dp));
                match (dq >= 0.0, dp >= 0.0) {
                    (true, true) => poly.push((q, qi, qh)),
                    (true, false) => {
                        poly.push((q, qi, qh));
                        // Along the cut to the next point.
                        poly.push((at(), None, false));
                    }
                    (false, true) => poly.push((at(), None, qh)),
                    (false, false) => {}
                }
            }
        }
        if poly.len() < 3 {
            continue;
        }
        let mut v = Vec::with_capacity(poly.len());
        let mut hidden = 0u64;
        for (k, (p, i, h)) in poly.iter().enumerate() {
            v.push(index_of(&mut vertices, &mut kept, *p, *i));
            if *h && k < 64 {
                hidden |= 1u64 << k;
            }
        }
        faces.push(MeshFace { v, hidden });
    }
    (!faces.is_empty()).then(|| Mesh { vertices, faces, kind: m.kind })
}

/// The new mesh's index of a point: a mesh vertex once, a cut point each time.
fn index_of(vertices: &mut Vec<Vec3>, kept: &mut std::collections::HashMap<u32, u32>, p: Vec3, i: Option<u32>) -> u32 {
    if let Some(i) = i
        && let Some(&j) = kept.get(&i)
    {
        return j;
    }
    let j = u32::try_from(vertices.len()).unwrap_or(u32::MAX);
    vertices.push(p);
    if let Some(i) = i {
        kept.insert(i, j);
    }
    j
}

#[cfg(test)]
mod tests {
    use super::*;

    fn box_mesh(z0: f64, z1: f64) -> Mesh {
        let mut vertices = Vec::new();
        for z in [z0, z1] {
            for (x, y) in [(0.0, 0.0), (1.0, 0.0), (1.0, 1.0), (0.0, 1.0)] {
                vertices.push(Vec3::new(x, y, z));
            }
        }
        let quad = |a: u32, b: u32, c: u32, d: u32| MeshFace { v: vec![a, b, c, d], hidden: 0 };
        let faces = vec![quad(0, 1, 2, 3), quad(4, 5, 6, 7), quad(0, 1, 5, 4), quad(1, 2, 6, 5), quad(2, 3, 7, 6), quad(3, 0, 4, 7)];
        Mesh { vertices, faces, kind: cadaddy_doc::MeshKind::default() }
    }

    fn zr(m: &Mesh) -> (f64, f64) {
        let used: Vec<f64> = m.faces.iter().flat_map(|f| f.v.iter()).filter_map(|i| m.vertices.get(*i as usize)).map(|v| v.z).collect();
        (used.iter().copied().fold(f64::INFINITY, f64::min), used.iter().copied().fold(f64::NEG_INFINITY, f64::max))
    }

    #[test]
    fn a_tall_box_is_cut_to_the_band() {
        let m = clip_mesh(&box_mesh(12.0, 384.0), Some(120.0), Some(240.0)).expect("a piece in the band");
        let (lo, hi) = zr(&m);
        assert!((lo - 120.0).abs() < 1e-9 && (hi - 240.0).abs() < 1e-9, "{lo}..{hi}");
        // The four sides stay (cut); the caps are outside.
        assert_eq!(m.faces.len(), 4);
    }

    #[test]
    fn a_box_above_the_band_leaves_nothing() {
        assert!(clip_mesh(&box_mesh(250.0, 300.0), Some(120.0), Some(240.0)).is_none());
        assert!(clip_mesh(&box_mesh(0.0, 100.0), Some(120.0), None).is_none());
    }

    #[test]
    fn a_box_cut_at_the_top_only_keeps_its_foot() {
        let m = clip_mesh(&box_mesh(12.0, 384.0), None, Some(240.0)).expect("its foot");
        let (lo, hi) = zr(&m);
        assert!((lo - 12.0).abs() < 1e-9 && (hi - 240.0).abs() < 1e-9, "{lo}..{hi}");
        assert_eq!(m.faces.len(), 5, "the bottom cap and the four sides");
    }

    #[test]
    fn hidden_edges_stay_hidden_and_the_cut_shows() {
        // A triangle with its first edge (0→1) hidden, straddling the top.
        let m = Mesh {
            vertices: vec![Vec3::new(0.0, 0.0, 0.0), Vec3::new(1.0, 0.0, 0.0), Vec3::new(0.0, 0.0, 10.0)],
            faces: vec![MeshFace { v: vec![0, 1, 2], hidden: 1 }],
            kind: cadaddy_doc::MeshKind::default(),
        };
        let c = clip_mesh(&m, None, Some(5.0)).expect("a piece");
        let f = c.faces.first().expect("a face");
        assert_eq!(f.v.len(), 4);
        assert_eq!(f.hidden, 1, "the hidden edge keeps its flag, the cut and the cut sides are drawn");
    }

    #[test]
    fn lines_and_polylines_cut() {
        let (a, b) = clip_seg(Vec3::new(0.0, 0.0, 0.0), Vec3::new(0.0, 0.0, 30.0), Some(10.0), Some(20.0)).expect("in");
        assert!((a.z - 10.0).abs() < 1e-9 && (b.z - 20.0).abs() < 1e-9);
        assert!(clip_seg(Vec3::new(0.0, 0.0, 0.0), Vec3::new(5.0, 0.0, 0.0), Some(10.0), None).is_none());
        let pts = [Vec3::new(0.0, 0.0, 5.0), Vec3::new(0.0, 0.0, 25.0), Vec3::new(10.0, 0.0, 25.0)];
        let p = clip_polyline(&pts, false, Some(10.0), Some(20.0)).expect("the riser's piece");
        assert_eq!(p.len(), 2);
    }
}
