//! What `model.clashes` measures a run's piece against in the built model (Q6386): the ceiling
//! over a plan point, the walls a piece is concealed in, and the built objects a clash names.
//!
//! `model.build` stacks each level's built objects onto the lowest level's plan (`stacked`
//! tag) while the runs keep their own level's plan points, so every outline here is taken back
//! onto its own level's plan before a run is measured against it.
//!
//! - **Ceilings**: the smallest built ceiling round a point (a room's inside a larger area's),
//!   its finished face; the same ceiling the conduit router keeps its raceway over.
//! - **Walls**: a piece of conduit, cable or pipe whose ends and middle are all inside built
//!   walls runs concealed in them (a raceway along the studs between two boxes, a pipe in a
//!   chase wall): it does not hang below a ceiling.
//! - **Built objects**: the model's own pieces of a run (its segments and fittings, or its
//!   conduit and cable atoms) of the same system on the same level beside the piece, nearest the
//!   clash first, so every clash can be found and shown.

use std::collections::HashMap;

use cadaddy_doc::{Drawing, Entity, EntityKind, Handle};
use cadaddy_geom::Vec2;

/// The most built objects read (a bound on a hostile drawing; the plans' own objects are
/// passed over, however many).
const MAX_OBJECTS: usize = 500_000;
/// The most vertices of one mesh read for its outline.
const MAX_VERTS: usize = 10_000;
/// The most built objects a clash names for one run.
pub(super) const MAX_NAMED: usize = 3;

/// The most faces of one mesh read for its plan shape.
const MAX_FACES: usize = 4_000;

/// A built outline on its own level's plan: its faces seen from above (an L-shaped room's
/// ceiling is its own shape, not its hull), its hull and plan bounds.
struct Outline {
    faces: Vec<Vec<Vec2>>,
    hull: Vec<Vec2>,
    lo: Vec2,
    hi: Vec2,
}

impl Outline {
    fn of(e: &Entity) -> Option<(Self, f64, f64)> {
        let EntityKind::Mesh(m) = &e.kind else { return None };
        let off = super::super::bspec::stacked_by(e);
        let pts: Vec<Vec2> = m.vertices.iter().take(MAX_VERTS).map(|v| v.xy() - off).filter(|p| p.is_finite()).collect();
        let (z0, z1) = m
            .vertices
            .iter()
            .take(MAX_VERTS)
            .map(|v| v.z)
            .filter(|z| z.is_finite())
            .fold((f64::INFINITY, f64::NEG_INFINITY), |(a, b), z| (a.min(z), b.max(z)));
        let hull = super::hull(&pts);
        if hull.len() < 3 || !(z0.is_finite() && z1.is_finite()) {
            return None;
        }
        let lo = hull.iter().fold(Vec2::new(f64::INFINITY, f64::INFINITY), |a, p| Vec2::new(a.x.min(p.x), a.y.min(p.y)));
        let hi = hull.iter().fold(Vec2::new(f64::NEG_INFINITY, f64::NEG_INFINITY), |a, p| Vec2::new(a.x.max(p.x), a.y.max(p.y)));
        // Its faces seen from above (the ones with an area there: its top and bottom).
        let faces: Vec<Vec<Vec2>> = m
            .faces
            .iter()
            .take(MAX_FACES)
            .filter_map(|f| f.v.iter().take(64).map(|i| m.vertices.get(*i as usize).map(|v| v.xy() - off)).collect::<Option<Vec<Vec2>>>())
            .filter(|f| f.len() >= 3 && cadaddy_geom::shoelace(f).abs() > 1e-9)
            .collect();
        Some((Outline { faces, hull, lo, hi }, z0, z1))
    }

    /// `p` inside the outline (its faces seen from above, else its hull) or within `slack` of it.
    fn holds(&self, p: Vec2, slack: f64) -> bool {
        if p.x < self.lo.x - slack || p.y < self.lo.y - slack || p.x > self.hi.x + slack || p.y > self.hi.y + slack {
            return false;
        }
        if self.faces.is_empty() {
            return cadaddy_geom::point_in_polygon(&self.hull, p) || dist_to_outline(&self.hull, p) <= slack;
        }
        self.faces.iter().any(|f| cadaddy_geom::point_in_polygon(f, p) || (slack > 0.0 && dist_to_outline(f, p) <= slack))
    }

    fn area(&self) -> f64 {
        cadaddy_geom::shoelace(&self.hull).abs()
    }
}

/// The shortest distance from `p` to a closed outline's edges.
fn dist_to_outline(poly: &[Vec2], p: Vec2) -> f64 {
    let n = poly.len();
    (0..n).filter_map(|i| Some((*poly.get(i)?, *poly.get((i + 1) % n)?))).map(|(a, b)| point_seg(p, a, b)).fold(f64::INFINITY, f64::min)
}

/// The distance from `p` to the segment `a`–`b`.
pub(super) fn point_seg(p: Vec2, a: Vec2, b: Vec2) -> f64 {
    let ab = b - a;
    let l2 = ab.len2();
    if l2 <= 1e-18 {
        return p.dist(a);
    }
    let t = ((p - a).dot(ab) / l2).clamp(0.0, 1.0);
    p.dist(a + ab * t)
}

/// A built piece of a run: its handle, plan bounds on its own level's plan, heights, and
/// whether it is a run's straight piece (named before its fittings).
struct Built {
    handle: Handle,
    lo: Vec2,
    hi: Vec2,
    z0: f64,
    z1: f64,
    straight: bool,
}

/// The built model as `model.clashes` measures runs against it.
pub(super) struct Scope {
    /// By level (lower case): each ceiling's outline, its finished face and its area.
    ceilings: HashMap<String, Vec<(Outline, f64, f64)>>,
    /// By level (lower case): each wall's outline.
    walls: HashMap<String, Vec<Outline>>,
    /// By (system, level), lower case: the run pieces built.
    built: HashMap<(String, String), Vec<Built>>,
}

/// The run piece kind a built MEP object is (`part:` tag): `Some(true)` a straight piece,
/// `Some(false)` a fitting; none for what a clash doesn't name (boxes, rings, wire, straps).
fn run_part(part: &str) -> Option<bool> {
    match part {
        "segment" | "atom:conduit" | "atom:cable" | "atom:offset" => Some(true),
        p if p.starts_with("fitting:") => Some(false),
        "atom:elbow" | "atom:bend" | "atom:coupling" | "atom:connector" => Some(false),
        _ => None,
    }
}

impl Scope {
    pub(super) fn of(d: &Drawing) -> Self {
        let mut s = Scope { ceilings: HashMap::new(), walls: HashMap::new(), built: HashMap::new() };
        let mut kept = 0usize;
        for e in d.model.iter() {
            if kept >= MAX_OBJECTS {
                break;
            }
            let Some(made) = super::super::wallgen::made_as(e) else { continue };
            kept += 1;
            let Some(level) = super::super::bspec::made_level(e) else { continue };
            let level = level.to_ascii_lowercase();
            match made.as_str() {
                "ceiling" => {
                    if let Some((o, z0, _)) = Outline::of(e) {
                        let area = o.area();
                        if area > 0.0 {
                            s.ceilings.entry(level).or_default().push((o, z0, area));
                        }
                    }
                }
                "wall" => {
                    if let Some((o, _, _)) = Outline::of(e) {
                        s.walls.entry(level).or_default().push(o);
                    }
                }
                "mep" | "atom" => {
                    let tag =
                        |key: &str| e.common.xdata.iter().filter(|t| t.code == 1000).find_map(|t| t.value.strip_prefix(key).map(str::to_string));
                    let (Some(system), Some(straight)) = (tag("system:"), tag("part:").as_deref().and_then(run_part)) else { continue };
                    let Some((o, z0, z1)) = Outline::of(e).or_else(|| thin(e)) else { continue };
                    s.built.entry((system.to_ascii_lowercase(), level)).or_default().push(Built {
                        handle: e.handle,
                        lo: o.lo,
                        hi: o.hi,
                        z0,
                        z1,
                        straight,
                    });
                }
                _ => {}
            }
        }
        s
    }

    /// The finished face of the ceiling over plan point `p` of `level` (sheet units): the
    /// smallest built ceiling round it; over the gap between rooms' ceilings (a wall's width,
    /// `gap`), the lower of the ceilings beside it; none where no ceiling is built near.
    pub(super) fn ceiling_at(&self, level: &str, p: Vec2, gap: f64) -> Option<f64> {
        let all = self.ceilings.get(&level.to_ascii_lowercase())?;
        all.iter()
            .filter(|(o, _, _)| o.holds(p, 0.0))
            .min_by(|a, b| a.2.total_cmp(&b.2))
            .map(|(_, z, _)| *z)
            .or_else(|| all.iter().filter(|(o, _, _)| o.holds(p, gap)).map(|(_, z, _)| *z).min_by(f64::total_cmp))
    }

    /// The piece `a`–`b` of `level` (half its width `half`) runs inside the level's built walls:
    /// both its ends and its middle.
    pub(super) fn in_walls(&self, level: &str, a: Vec2, b: Vec2, half: f64) -> bool {
        let Some(walls) = self.walls.get(&level.to_ascii_lowercase()) else { return false };
        let slack = half.max(0.0);
        [a, (a + b) * 0.5, b].iter().all(|p| walls.iter().any(|w| w.holds(*p, slack)))
    }

    /// The built pieces of `system` on `level` beside the run piece `a`–`b` (heights `z0`–`z1`,
    /// width `w`), nearest `near` first, straight pieces before fittings: at most [`MAX_NAMED`].
    #[allow(clippy::too_many_arguments)]
    pub(super) fn named(&self, system: &str, level: &str, a: Vec2, b: Vec2, z0: f64, z1: f64, w: f64, near: Vec2, upf: f64) -> Vec<Handle> {
        let Some(all) = self.built.get(&(system.to_ascii_lowercase(), level.to_ascii_lowercase())) else { return Vec::new() };
        let pad = w.max(0.0) / 2.0 + upf / 12.0;
        let (lo, hi) = (Vec2::new(a.x.min(b.x) - pad, a.y.min(b.y) - pad), Vec2::new(a.x.max(b.x) + pad, a.y.max(b.y) + pad));
        let mut hits: Vec<(bool, f64, Handle)> = all
            .iter()
            .filter(|p| p.lo.x <= hi.x && p.hi.x >= lo.x && p.lo.y <= hi.y && p.hi.y >= lo.y)
            .filter(|p| p.z0 <= z1 + pad && p.z1 >= z0 - pad)
            .filter(|p| {
                // Beside the piece's own line, not just inside its box (an L-shaped run's).
                let c = Vec2::new((p.lo.x + p.hi.x) / 2.0, (p.lo.y + p.hi.y) / 2.0);
                let r = Vec2::new(p.hi.x - p.lo.x, p.hi.y - p.lo.y).len() / 2.0;
                point_seg(c, a, b) <= r + pad
            })
            .map(|p| {
                let c = Vec2::new(near.x.clamp(p.lo.x, p.hi.x), near.y.clamp(p.lo.y, p.hi.y));
                (!p.straight, c.dist(near), p.handle)
            })
            .collect();
        hits.sort_by(|x, y| x.0.cmp(&y.0).then(x.1.total_cmp(&y.1)).then(x.2.cmp(&y.2)));
        hits.into_iter().take(MAX_NAMED).map(|(_, _, h)| h).collect()
    }
}

/// A mesh too thin for an outline (a straight piece seen end on, a vertical stick): its plan
/// bounds as a box.
fn thin(e: &Entity) -> Option<(Outline, f64, f64)> {
    let EntityKind::Mesh(m) = &e.kind else { return None };
    let off = super::super::bspec::stacked_by(e);
    let mut lo = Vec2::new(f64::INFINITY, f64::INFINITY);
    let mut hi = Vec2::new(f64::NEG_INFINITY, f64::NEG_INFINITY);
    let (mut z0, mut z1) = (f64::INFINITY, f64::NEG_INFINITY);
    for v in m.vertices.iter().take(MAX_VERTS) {
        let p = v.xy() - off;
        if !(p.is_finite() && v.z.is_finite()) {
            continue;
        }
        lo = Vec2::new(lo.x.min(p.x), lo.y.min(p.y));
        hi = Vec2::new(hi.x.max(p.x), hi.y.max(p.y));
        z0 = z0.min(v.z);
        z1 = z1.max(v.z);
    }
    (lo.is_finite() && hi.is_finite() && z0.is_finite())
        .then(|| (Outline { faces: Vec::new(), hull: vec![lo, Vec2::new(hi.x, lo.y), hi, Vec2::new(lo.x, hi.y)], lo, hi }, z0, z1))
}
