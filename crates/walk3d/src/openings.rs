//! Doors, windows and cased openings: the gaps between two pieces of one wall.
//!
//! A plan draws an opening as a break in both faces of a wall. Walk mode finds those breaks between
//! the solid wall pieces ([`Piece`]) and fills them the way they are built: a door gets a header
//! above its head and its leaf standing open on the hinge its swing arc shows; a gap in an outside
//! wall without a swing is a window (wall below the sill and above the head, glass between, and it
//! stops the walker); any other gap is a cased opening with a header.

use crate::derive::{Owner, Piece, Scene, Seg, WALL, WALL_TOP, boxed, edge, quad};
use crate::math::v3;
use crate::rules::Openings;
use cadaddy_color::Rgb;
use cadaddy_doc::geom::Vec2;

const GLASS: Rgb = Rgb(150, 192, 218);
const DOOR: Rgb = Rgb(150, 112, 78);
/// Door leaf thickness, feet (1¾").
const LEAF_FT: f64 = 0.15;
/// Gaps wider than this (feet) without a door or window are left open (corridor junctions).
const MAX_CASED_FT: f64 = 8.0;
/// The wall either side of an opening may read this much thicker or thinner (feet), as frames
/// and jamb returns make it.
const THICKNESS_SLACK_FT: f64 = 0.35;
/// Gaps found from both faces of one wall within this (feet) of each other are one opening.
const SAME_GAP_FT: f64 = 1.0;
/// Pieces looked at per level for openings (hostile input; quadratic).
const MAX_PIECES: usize = 8_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    Door,
    Window,
    Cased,
}

/// One gap: the face-line ends `e0`..`e1`, the wall's thickness `off` along `n`.
#[derive(Clone, Copy, Debug)]
struct Gap {
    lv: usize,
    e0: Vec2,
    e1: Vec2,
    n: Vec2,
    off: f64,
    z0: f64,
    z1: f64,
    owner: Owner,
}

impl Gap {
    fn width(&self) -> f64 {
        (self.e1 - self.e0).len()
    }
    fn centre(&self) -> Vec2 {
        (self.e0 + self.e1) * 0.5 + self.n * (self.off / 2.0)
    }
}

/// A swing arc: its centre (the hinge), radius and end points.
#[derive(Clone, Copy, Debug)]
struct Swing {
    c: Vec2,
    r: f64,
    ends: [Vec2; 2],
    owner: Owner,
}

/// An opening found and what it is; [`build`] fills it.
#[derive(Clone, Debug)]
pub(crate) struct Planned {
    gap: Gap,
    kind: Kind,
    /// Door leaves: hinge and swing.
    hinged: Vec<(Vec2, Swing)>,
}

/// Find the openings between `pieces` of the walls `all` (the wall faces); `curves` are the curved
/// runs the wall layers dropped (door swings among them).
pub(crate) fn plan(all: &[Seg], pieces: &[Piece], curves: &[(Vec<Vec2>, usize, Owner)], o: &Openings) -> Vec<Planned> {
    if !o.enabled {
        return Vec::new();
    }
    let swings: Vec<(usize, Swing)> = curves.iter().filter_map(|(pts, lv, owner)| swing(pts, *owner).map(|sw| (*lv, sw))).collect();
    // One wall read from either face is the same wall: measure every piece from the face whose
    // normal points east (or north), so the pieces either side of an opening line up.
    let pieces: Vec<Piece> = pieces
        .iter()
        .map(|p| {
            if p.n.x < -1e-6 || (p.n.x.abs() <= 1e-6 && p.n.y < 0.0) {
                Piece { p0: p.p0 + p.n * p.off, p1: p.p1 + p.n * p.off, n: p.n * -1.0, ..*p }
            } else {
                *p
            }
        })
        .collect();
    let min = o.min_ft.max(0.5);
    let max = o.max_ft.clamp(min, 60.0);
    let mut gaps: Vec<Gap> = Vec::new();
    let mut levels: Vec<usize> = pieces.iter().map(|p| p.lv).collect();
    levels.sort_unstable();
    levels.dedup();
    for lv in levels {
        let on: Vec<&Piece> = pieces.iter().filter(|p| p.lv == lv).collect();
        let walls: Vec<&Seg> = all.iter().filter(|w| w.level == lv).collect();
        if on.len() > MAX_PIECES {
            continue;
        }
        for a in &on {
            let la = (a.p1 - a.p0).len();
            if la < 1e-9 {
                continue;
            }
            let t = (a.p1 - a.p0) * (1.0 / la);
            // The nearest piece of the same wall beyond this one's end.
            let mut best: Option<(f64, Vec2)> = None;
            for b in &on {
                if (b.n.x * a.n.x + b.n.y * a.n.y) < 0.99 || (b.off - a.off).abs() > THICKNESS_SLACK_FT {
                    continue;
                }
                let side = |p: Vec2| (p - a.p0).x * a.n.x + (p - a.p0).y * a.n.y;
                if side(b.p0).abs() > 0.05 || side(b.p1).abs() > 0.05 {
                    continue;
                }
                let along = |p: Vec2| (p - a.p0).x * t.x + (p - a.p0).y * t.y;
                let lo = along(b.p0).min(along(b.p1));
                let gap = lo - la;
                if (min..=max).contains(&gap) && best.is_none_or(|(g, _)| gap < g) {
                    best = Some((gap, a.p0 + t * lo));
                }
            }
            // The first wall crossing the opening's middle beyond this end: an opening stops there.
            let blocked = crate::measure::ray(&walls, a.p1 + a.n * (a.off / 2.0), t);
            let corners = [a.p1, a.p1 + a.n * a.off];
            let hinged = swings.iter().any(|(l, sw)| *l == lv && corners.iter().any(|c| (*c - sw.c).len() < 0.6));
            let e1 = match (best, blocked) {
                (Some((g, e1)), b) if b.is_none_or(|b| b >= g - 0.05) => Some(e1),
                // A door at a corner: it runs to the wall it meets.
                (_, Some(b)) if hinged && (min..=max).contains(&b) => Some(a.p1 + t * b),
                _ => None,
            };
            if let Some(e1) = e1 {
                gaps.push(Gap { lv, e0: a.p1, e1, n: a.n, off: a.off, z0: a.z0, z1: a.z1, owner: a.owner });
            }
        }
    }
    // Both faces of a wall can each yield the same gap.
    let mut unique: Vec<Gap> = Vec::new();
    for g in gaps {
        if !unique.iter().any(|u| u.lv == g.lv && (u.centre() - g.centre()).len() < SAME_GAP_FT) {
            unique.push(g);
        }
    }
    // Every gap closed off, for the outside test: looking out of one opening must not see out
    // through another (an entry door lines up with its room's window).
    let closed: Vec<(usize, Seg)> = unique
        .iter()
        .enumerate()
        .flat_map(|(i, g)| [(i, Seg { a: g.e0, b: g.e1, level: g.lv }), (i, Seg { a: g.e0 + g.n * g.off, b: g.e1 + g.n * g.off, level: g.lv })])
        .collect();
    let gap_list = unique.clone();
    let mut out = Vec::new();
    for (gi, g) in gap_list.iter().copied().enumerate() {
        let width = g.width();
        let corners = [g.e0, g.e1, g.e0 + g.n * g.off, g.e1 + g.n * g.off];
        let hinged: Vec<(Vec2, Swing)> = swings
            .iter()
            .filter(|(lv, _)| *lv == g.lv)
            .filter_map(|(_, sw)| {
                let (hinge, d) = corners.iter().map(|c| (*c, (*c - sw.c).len())).min_by(|x, y| x.1.total_cmp(&y.1))?;
                (d < 0.6 && sw.r >= 0.4 * width && sw.r <= 1.3 * width).then_some((hinge, *sw))
            })
            .collect();
        let kind = if !hinged.is_empty() {
            Kind::Door
        } else if outside(
            all,
            &g,
            closed
                .iter()
                .filter(|(i, _)| *i != gi && gap_list.get(*i).is_none_or(|o| (o.centre() - g.centre()).len() > SAME_GAP_FT * 1.5))
                .map(|(_, seg)| seg),
        ) {
            Kind::Window
        } else {
            Kind::Cased
        };
        out.push(Planned { gap: g, kind, hinged });
    }
    out
}

/// A wall face lying inside a window opening is part of the window unit (its frame), not wall.
pub(crate) fn in_window(plans: &[Planned], lv: usize, a: Vec2, b: Vec2) -> bool {
    plans.iter().filter(|p| p.kind == Kind::Window && p.gap.lv == lv).any(|p| {
        let g = &p.gap;
        let w = g.width();
        if w < 1e-9 {
            return false;
        }
        let t = (g.e1 - g.e0) * (1.0 / w);
        let inside = |q: Vec2| {
            let (u, v) = ((q - g.e0).x * t.x + (q - g.e0).y * t.y, (q - g.e0).x * g.n.x + (q - g.e0).y * g.n.y);
            (-0.05..=w + 0.05).contains(&u) && (-0.05..=g.off + 0.05).contains(&v)
        };
        inside(a) && inside(b)
    })
}

/// Fill the planned openings.
pub(crate) fn build(s: &mut Scene, plans: &[Planned], o: &Openings) {
    for p in plans {
        let (g, hinged) = (p.gap, &p.hinged);
        let width = g.width();
        let (z0, z1) = (g.z0, g.z1);
        match p.kind {
            Kind::Door => {
                header(s, &g, z0 + o.door_head_ft, z1);
                let wall_mid = |p: Vec2| ((p - g.e0).x * g.n.x + (p - g.e0).y * g.n.y - g.off / 2.0).abs();
                for (hinge, sw) in hinged.iter().take(2) {
                    // The swing's end away from the wall is the leaf standing open.
                    let tip = if wall_mid(sw.ends[0]) > wall_mid(sw.ends[1]) { sw.ends[0] } else { sw.ends[1] };
                    let u = tip - *hinge;
                    if u.len() < 1e-6 {
                        continue;
                    }
                    let u = u * (1.0 / u.len());
                    let h = o.door_head_ft.min(z1 - z0).max(0.1);
                    let c = *hinge + u * (sw.r / 2.0);
                    let first_vertex = s.tris.len();
                    s.part = crate::Part::Door;
                    boxed(s, v3(c.x, c.y, z0 + h / 2.0), u, sw.r, LEAF_FT, h, DOOR, sw.owner);
                    s.part = crate::Part::Wall;
                    // Shut, the leaf lies across the opening in the middle of the wall.
                    let along = g.e1 - g.e0;
                    let along = if along.len() > 1e-9 { along * (1.0 / along.len()) } else { Vec2 { x: 1.0, y: 0.0 } };
                    let to_mid = g.centre() - *hinge;
                    let shut_dir = if to_mid.x * along.x + to_mid.y * along.y < 0.0 { along * -1.0 } else { along };
                    let depth = (*hinge - g.e0).x * g.n.x + (*hinge - g.e0).y * g.n.y;
                    s.door_leaves.push(crate::DoorLeaf {
                        level: g.lv,
                        hinge: *hinge,
                        open_dir: u,
                        shut_dir,
                        width: sw.r,
                        thick: LEAF_FT,
                        height: h,
                        z0,
                        shut_offset: g.n * (g.off / 2.0 - depth),
                        first_vertex,
                        color: DOOR,
                        owner: sw.owner,
                        open: true,
                    });
                }
                s.doors += 1;
            }
            Kind::Window => {
                let (sill, head) = (z0 + o.window_sill_ft.max(0.0), (z0 + o.window_head_ft).min(z1));
                slab(s, &g, z0, sill, true);
                header(s, &g, head, z1);
                let (m0, m1) = (g.e0 + g.n * (g.off / 2.0), g.e1 + g.n * (g.off / 2.0));
                if head > sill {
                    s.part = crate::Part::Glass;
                    quad(s, [v3(m0.x, m0.y, sill), v3(m1.x, m1.y, sill), v3(m1.x, m1.y, head), v3(m0.x, m0.y, head)], GLASS, g.owner);
                    s.part = crate::Part::Wall;
                }
                // Windows stop the walker.
                s.walls.push(Seg { a: g.e0, b: g.e1, level: g.lv });
                s.walls.push(Seg { a: g.e0 + g.n * g.off, b: g.e1 + g.n * g.off, level: g.lv });
                s.windows += 1;
            }
            Kind::Cased => {
                if width <= MAX_CASED_FT {
                    header(s, &g, z0 + o.door_head_ft, z1);
                    s.cased += 1;
                }
            }
        }
    }
}

/// The wall above an opening, from `zb` up to `zt`.
fn header(s: &mut Scene, g: &Gap, zb: f64, zt: f64) {
    slab(s, g, zb, zt, false);
}

/// A block of wall filling the gap between heights `zb` and `zt`: both faces, its top (a dark
/// cap when it is a sill or the wall's top) and its underside.
fn slab(s: &mut Scene, g: &Gap, zb: f64, zt: f64, sill: bool) {
    if zt - zb < 0.01 {
        return;
    }
    let (a, b) = (g.e0, g.e1);
    let (c, d) = (g.e1 + g.n * g.off, g.e0 + g.n * g.off);
    let p = |q: Vec2, z: f64| v3(q.x, q.y, z);
    quad(s, [p(a, zb), p(b, zb), p(b, zt), p(a, zt)], WALL, g.owner);
    quad(s, [p(c, zb), p(d, zb), p(d, zt), p(c, zt)], WALL, g.owner);
    quad(s, [p(a, zt), p(b, zt), p(c, zt), p(d, zt)], if sill { WALL } else { WALL_TOP }, g.owner);
    quad(s, [p(a, zb), p(d, zb), p(c, zb), p(b, zb)], WALL, g.owner);
    // Outline the opening's edge of the block: the soffit of a header, the top of a sill.
    let z_edge = if sill { zt } else { zb };
    edge(s, p(a, z_edge), p(b, z_edge));
    edge(s, p(d, z_edge), p(c, z_edge));
}

/// The gap is in an outside wall: looking out of either face, no wall of its level (nor another
/// opening, `others`) is in reach.
fn outside<'a>(all: &'a [Seg], g: &Gap, others: impl Iterator<Item = &'a Seg>) -> bool {
    let walls: Vec<&Seg> = all.iter().chain(others).filter(|w| w.level == g.lv).collect();
    // From just beyond each face: the window unit itself (frame lines inside the wall) is no wall.
    let c = (g.e0 + g.e1) * 0.5;
    let beyond = c + g.n * (g.off + 0.05);
    let before = c - g.n * 0.05;
    crate::measure::ray(&walls, beyond, g.n).is_none() || crate::measure::ray(&walls, before, g.n * -1.0).is_none()
}

/// A run that is a quarter-ish circle (60°–120°), as a door swing is drawn.
fn swing(pts: &[Vec2], owner: Owner) -> Option<Swing> {
    let (first, last) = (*pts.first()?, *pts.last()?);
    let mid = *pts.get(pts.len() / 2)?;
    let (c, r) = circle(first, mid, last)?;
    if !(0.8..=8.0).contains(&r) || pts.iter().any(|p| ((*p - c).len() - r).abs() > 0.1 * r) {
        return None;
    }
    let (u, v) = (first - c, last - c);
    let ang = ((u.x * v.x + u.y * v.y) / (u.len() * v.len()).max(1e-12)).clamp(-1.0, 1.0).acos().to_degrees();
    (60.0..=120.0).contains(&ang).then_some(Swing { c, r, ends: [first, last], owner })
}

/// The circle through three points.
fn circle(a: Vec2, b: Vec2, c: Vec2) -> Option<(Vec2, f64)> {
    let d = 2.0 * (a.x * (b.y - c.y) + b.x * (c.y - a.y) + c.x * (a.y - b.y));
    if d.abs() < 1e-9 {
        return None;
    }
    let (a2, b2, c2) = (a.x * a.x + a.y * a.y, b.x * b.x + b.y * b.y, c.x * c.x + c.y * c.y);
    let ux = (a2 * (b.y - c.y) + b2 * (c.y - a.y) + c2 * (a.y - b.y)) / d;
    let uy = (a2 * (c.x - b.x) + b2 * (a.x - c.x) + c2 * (b.x - a.x)) / d;
    let centre = Vec2 { x: ux, y: uy };
    let r = (a - centre).len();
    (r.is_finite() && centre.x.is_finite() && centre.y.is_finite()).then_some((centre, r))
}

#[cfg(test)]
mod tests {
    use crate::derive::{Scene, derive};
    use crate::rules::Rules;
    use cadaddy_doc::{Drawing, Entity, EntityKind, Handle, Line, LwPolyline};
    use cadaddy_geom::Vec3;

    fn add(d: &mut Drawing, kind: EntityKind) {
        let h = Handle(d.handseed.max(1));
        d.handseed = h.0 + 1;
        let mut e = Entity::new(h, kind);
        e.common.layer = "A-WALL".into();
        d.model.push(e);
    }

    /// Feet → a line in inches.
    fn line(d: &mut Drawing, a: (f64, f64), b: (f64, f64)) {
        let p = |(x, y): (f64, f64)| Vec3 { x: x * 12.0, y: y * 12.0, z: 0.0 };
        add(d, EntityKind::Line(Line { a: p(a), b: p(b) }));
    }

    /// Two rooms in a 40' × 10' box of 6" walls: a 4' window gap in the south wall (x 8–12), and
    /// a 3' doorway in the middle wall (y 3–6) with its swing into the west room, hinged at (20, 3).
    fn two_rooms() -> Scene {
        let mut d = Drawing::default();
        for (a, b) in [((0., 0.), (8., 0.)), ((12., 0.), (40., 0.)), ((40., 0.), (40., 10.)), ((40., 10.), (0., 10.)), ((0., 10.), (0., 0.))] {
            line(&mut d, a, b);
        }
        for (a, b) in
            [((0.5, 0.5), (8., 0.5)), ((12., 0.5), (39.5, 0.5)), ((39.5, 0.5), (39.5, 9.5)), ((39.5, 9.5), (0.5, 9.5)), ((0.5, 9.5), (0.5, 0.5))]
        {
            line(&mut d, a, b);
        }
        for x in [20.0, 20.5] {
            line(&mut d, (x, 0.5), (x, 3.0));
            line(&mut d, (x, 6.0), (x, 9.5));
        }
        let swing: Vec<_> = (0..=9)
            .map(|i| f64::from(i) * std::f64::consts::FRAC_PI_2 / 9.0)
            .map(|a| serde_json::json!({"p": {"x": (20.0 - 3.0 * a.sin()) * 12.0, "y": (3.0 + 3.0 * a.cos()) * 12.0}, "bulge": 0.0, "start_width": 0.0, "end_width": 0.0}))
            .collect();
        let pl: LwPolyline = serde_json::from_value(serde_json::json!({"vertices": swing, "closed": false})).unwrap_or_else(|e| panic!("{e}"));
        add(&mut d, EntityKind::LwPolyline(pl));
        let rules = Rules::from_json(
            r#"{"levels":[{"name":"L1","elevation_ft":0,"height_ft":9.5,"ceiling_ft":8,"base":[0,0]}],
                "layers":[{"layer":"A-WALL","kind":"wall","skip_curves":true}]}"#,
        )
        .unwrap_or_else(|e| panic!("{e}"));
        derive(&d, &rules)
    }

    #[test]
    fn doors_open_and_shut_and_a_shut_door_stops_the_walker() {
        let mut s = two_rooms();
        assert_eq!(s.door_leaves.len(), 1);
        let walk_east = |s: &Scene| {
            let mut w = crate::Walker::spawn(s, 0);
            w.eye = crate::math::v3(15.0, 4.5, 5.5);
            w.yaw = 0.0;
            w.step(s, 10.0, 0.0, 0.0);
            w.eye.x
        };
        assert!(walk_east(&s) > 22.0, "through the open door");
        // The leaf's triangles belong to the door; shutting it moves them across the opening.
        let tri = s.door_leaves[0].first_vertex / 3 + 1;
        assert_eq!(s.door_of_triangle(tri), Some(0));
        assert_eq!(s.toggle_door(0), Some(false));
        let leaf = &s.tris[s.door_leaves[0].first_vertex..s.door_leaves[0].first_vertex + crate::doors::LEAF_VERTS];
        assert!(leaf.iter().all(|v| v.p[0] > 19.8 && v.p[0] < 20.7), "shut in the wall: {:?}", leaf.iter().map(|v| v.p).collect::<Vec<_>>());
        assert!(walk_east(&s) < 20.0, "a shut door stops the walker");
        assert_eq!(s.toggle_door(0), Some(true));
        assert!(walk_east(&s) > 22.0);
        assert_eq!(s.toggle_door(5), None);
        assert_eq!(s.door_of_triangle(0), None);
    }

    #[test]
    fn a_gap_with_a_swing_is_a_door_and_an_outside_gap_a_window() {
        let s = two_rooms();
        assert_eq!((s.doors, s.windows, s.cased), (1, 1, 0));
        let has = |rgb: [u8; 3], pred: &dyn Fn(&[f32; 3]) -> bool| s.tris.iter().any(|v| v.c[..3] == rgb && pred(&v.p));
        // Glass between the 2' sill and 7' head, in the south wall.
        assert!(has([150, 192, 218], &|p| p[1] > 0.0 && p[1] < 0.5 && (p[2] - 2.0).abs() < 1e-4));
        assert!(has([150, 192, 218], &|p| (p[2] - 7.0).abs() < 1e-4));
        // The leaf stands open into the west room from the hinge at (20, 3): its far end is near x 17.
        assert!(has([150, 112, 78], &|p| p[0] < 17.2 && p[0] > 16.8));
        // The window stops the walker: segments across the gap on both faces.
        assert!(s.walls.iter().any(|w| (w.a.y - 0.0).abs() < 1e-9 && (w.b.y - 0.0).abs() < 1e-9 && (w.a.x - 8.0).abs() < 1e-9));
    }

    #[test]
    fn openings_can_be_turned_off() {
        let mut d = Drawing::default();
        line(&mut d, (0., 0.), (8., 0.));
        line(&mut d, (12., 0.), (20., 0.));
        line(&mut d, (0., 0.5), (8., 0.5));
        line(&mut d, (12., 0.5), (20., 0.5));
        let rules = Rules::from_json(
            r#"{"levels":[{"name":"L1","elevation_ft":0,"height_ft":9.5}],"layers":[{"layer":"A-WALL","kind":"wall"}],
                "openings":{"enabled":false}}"#,
        )
        .unwrap_or_else(|e| panic!("{e}"));
        let s = derive(&d, &rules);
        assert_eq!((s.doors, s.windows, s.cased), (0, 0, 0));
    }
}
