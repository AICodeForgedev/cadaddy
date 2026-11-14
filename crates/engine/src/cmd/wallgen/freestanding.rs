//! Wall pieces standing free inside the building are equipment, not walls (Q6637).
//!
//! A kitchen's shelving, prep tables, cook line and walk-in equipment, a store's fixtures, a
//! column's box, drawn on the floor plan as small outlines a few inches to a foot and a half
//! deep, pair up like a wall's faces. A wall runs between other walls, the outside or a door: at
//! least one of its ends meets another wall (its corner or its tee), carries on in line across an
//! opening, or holds a door. A group of pieces none of whose ends does any of that, small, and
//! inside the building clear of its outside walls, walling in no room, is the equipment or casework drawn there:
//! left out of the walls (its linework stays for the equipment readers), counted in the notes.
//! The sizes are rule data (`wallcheck.json`).

use cadaddy_geom::Vec2;

use super::{PlanWall, WallGrid, root, seg_dist};

/// Pieces whose ends this many degrees or more apart in direction meet as a corner or a tee;
/// closer, they run the same way (in line, or side by side).
const CORNER_DEG: f64 = 20.0;
/// Pieces within this many degrees run in line across an opening.
const IN_LINE_DEG: f64 = 3.0;
/// Two pieces in line are one wall's across an opening when the thinner is at least this share
/// of the thicker (a wall's one type either side of a door; a shelf in line with a wall is not).
const IN_LINE_THICK_SHARE: f64 = 0.66;
/// How far past a wall's body an end may stop and still meet it, inches (an end read short of
/// the face it meets).
const MEET_SLACK_IN: f64 = 3.0;
/// How far from a door's hinge or the ends of its swing a wall's end holds it, inches, past
/// half the wall's thickness.
const DOOR_REACH_IN: f64 = 6.0;
/// Walls weighed at most (a level; hostile drawings): more and nothing is dropped.
const MAX_WALLS: usize = 20_000;
/// Pairs compared at most.
const MAX_COMPARED: usize = 20_000_000;

/// The sizes the rule uses, drawing units.
pub(super) struct Sizes {
    /// A group of free pieces at most this long on its longest side is equipment.
    pub max_side: f64,
    /// It stands at least this far inside the building's outside walls.
    pub shell_clear: f64,
    /// A group walling in a room at least this across both ways is the room's walls.
    pub room_min: f64,
    /// Two pieces in line across a gap of at most this are one wall across an opening.
    pub max_opening: f64,
    /// A corner post between two walls' ends (they meet across it).
    pub corner_post: f64,
    /// Drawing units an inch.
    pub k: f64,
}

/// Whether the end `p` of `w` meets `o`: on (or just short of) its body, turning a corner or
/// making a tee with it; or at its end across a corner post.
fn end_meets(p: Vec2, w: &PlanWall, o: &PlanWall, z: &Sizes) -> bool {
    let (dw, d_o) = (w.b - w.a, o.b - o.a);
    let (lw, lo) = (dw.len(), d_o.len());
    if lw <= 1e-9 || lo <= 1e-9 {
        return false;
    }
    let sin = (dw.cross(d_o) / (lw * lo)).abs();
    if sin < CORNER_DEG.to_radians().sin() {
        return false;
    }
    let reach = 0.5 * (w.thickness + o.thickness) + MEET_SLACK_IN * z.k;
    seg_dist(p, o.a, o.b) <= reach || o.a.dist(p).min(o.b.dist(p)) <= reach + z.corner_post
}

/// Whether two pieces' bodies cross (an X of walls, each running through the other).
fn crosses(w: &PlanWall, o: &PlanWall) -> bool {
    let side = |p: Vec2, q: Vec2, r: Vec2| (q - p).cross(r - p);
    side(o.a, o.b, w.a) * side(o.a, o.b, w.b) < 0.0 && side(w.a, w.b, o.a) * side(w.a, w.b, o.b) < 0.0
}

/// Whether `w` and `o` run in line, one wall either side of an opening: the same way, on one
/// line, of about one thickness, the gap between their nearest ends at most an opening. Some
/// with the opening's ends (`w`'s end and `o`'s facing it; none where they overlap).
fn in_line(w: &PlanWall, o: &PlanWall, z: &Sizes) -> Option<Option<(Vec2, Vec2)>> {
    let (dw, d_o) = (w.b - w.a, o.b - o.a);
    let (lw, lo) = (dw.len(), d_o.len());
    if lw <= 1e-9 || lo <= 1e-9 {
        return None;
    }
    let (thin, thick) = (w.thickness.min(o.thickness), w.thickness.max(o.thickness));
    if !(thick > 0.0 && thin >= IN_LINE_THICK_SHARE * thick) {
        return None;
    }
    let u = dw * (1.0 / lw);
    if (u.cross(d_o) / lo).abs() > IN_LINE_DEG.to_radians().sin() {
        return None;
    }
    let n = u.perp();
    let off = |p: Vec2| n.dot(p - w.a).abs();
    if off(o.a).max(off(o.b)) > 0.5 * thick + z.k {
        return None;
    }
    // Along `w`: `o` starts past its end or ends before its start, within an opening.
    let (sa, sb) = (u.dot(o.a - w.a), u.dot(o.b - w.a));
    let (near, s0, s1) = if sa <= sb { (o.a, sa, sb) } else { (o.b, sb, sa) };
    let far = if sa <= sb { o.b } else { o.a };
    if s0 >= lw {
        (s0 - lw <= z.max_opening).then_some(Some((w.b, near)))
    } else if s1 <= 0.0 {
        (-s1 <= z.max_opening).then_some(Some((far, w.a)))
    } else {
        Some(None)
    }
}

/// Whether the pieces `members` of `walls` wall in a room at least `room_min` across both ways
/// (a shaft, a closet, a walk-in: walls on three sides or more of the middle of their extents);
/// shelving in a row or an L walls in nothing.
fn walls_in_a_room(walls: &[PlanWall], members: &[usize], room_min: f64) -> bool {
    let ws: Vec<&PlanWall> = members.iter().filter_map(|&i| walls.get(i)).collect();
    if ws.len() < 3 {
        return false;
    }
    let Some(long) = ws.iter().max_by(|a, b| a.a.dist(a.b).total_cmp(&b.a.dist(b.b))) else { return false };
    let len = long.a.dist(long.b);
    if len <= 1e-9 {
        return false;
    }
    let u = (long.b - long.a) * (1.0 / len);
    let n = u.perp();
    // Its extents along its own two ways.
    let span = |d: Vec2| {
        let (lo, hi) =
            ws.iter().flat_map(|w| [d.dot(w.a), d.dot(w.b)]).fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), v| (lo.min(v), hi.max(v)));
        (lo, hi)
    };
    let ((u0, u1), (n0, n1)) = (span(u), span(n));
    if u1 - u0 < room_min || n1 - n0 < room_min {
        return false;
    }
    let c = u * (0.5 * (u0 + u1)) + n * (0.5 * (n0 + n1));
    // A ray from the middle each way: does it pass through one of the pieces?
    let hits = |d: Vec2| {
        ws.iter().any(|w| {
            let e = w.b - w.a;
            let den = d.cross(e);
            if den.abs() <= 1e-12 {
                return false;
            }
            let t = (w.a - c).cross(e) / den;
            let s = (w.a - c).cross(d) / den;
            t > 0.0 && (0.0..=1.0).contains(&s)
        })
    };
    [u, u * -1.0, n, n * -1.0].into_iter().filter(|d| hits(*d)).count() >= 3
}

/// Drops the groups of wall pieces among `walls[..droppable]` that stand free inside the
/// building (see the module). `doors`: each door or window's hinge (or insertion point) and its
/// swing's ends. The walls at `droppable` and after (single-line walls, jambs) are never dropped,
/// and a group joined to one stays. Returns how many pieces were dropped and their length.
pub(super) fn drop_free_standing(walls: &mut Vec<PlanWall>, droppable: usize, doors: &[Vec2], z: &Sizes) -> (usize, f64) {
    let n = walls.len();
    let droppable = droppable.min(n);
    let sizes_ok = [z.max_side, z.shell_clear, z.room_min, z.max_opening, z.corner_post, z.k].iter().all(|v| v.is_finite() && *v >= 0.0);
    if !(4..=MAX_WALLS).contains(&n) || droppable == 0 || !sizes_ok || z.max_side <= 0.0 || z.k <= 0.0 {
        return (0, 0.0);
    }
    let max_thick = walls.iter().map(|w| w.thickness).filter(|t| t.is_finite()).fold(0.0f64, f64::max);
    // Cells about a corner's reach wide: each end looks in its own cell, and along its line out
    // to an opening's width for a piece in line.
    let cell = (2.0 * max_thick + z.corner_post + MEET_SLACK_IN * z.k).max(12.0 * z.k);
    let grid = WallGrid::build(walls, cell);
    let mut parent: Vec<usize> = (0..n).collect();
    let mut compared = 0usize;
    // The openings between pieces in line: each its ends, thickness and a piece of its wall. A
    // wall ending on one (running up to a wide opening between two lined-up walls) meets them.
    let mut bridges: Vec<(Vec2, Vec2, f64, usize)> = Vec::new();
    let join = |parent: &mut Vec<usize>, i: usize, j: usize| {
        let (ri, rj) = (root(parent, i), root(parent, j));
        if ri != rj
            && let Some(x) = parent.get_mut(ri)
        {
            *x = rj;
        }
    };
    for i in 0..n {
        let Some(w) = walls.get(i) else { continue };
        let len = w.a.dist(w.b);
        if len <= 1e-9 {
            continue;
        }
        let u = (w.b - w.a) * (1.0 / len);
        // The cells round each end, and along the line past it to an opening's width.
        let steps = (z.max_opening / cell).ceil().clamp(0.0, 64.0) as usize;
        let mut near: Vec<usize> = Vec::new();
        for (p, out) in [(w.a, u * -1.0), (w.b, u)] {
            for s in 0..=steps {
                near.extend_from_slice(grid.at(p + out * (s as f64 * cell)));
            }
        }
        // And along its body (a tee meeting it, a wall crossing it).
        let body_steps = (len / cell).ceil().clamp(0.0, 4096.0) as usize;
        for s in 0..=body_steps {
            near.extend_from_slice(grid.at(w.a + u * (s as f64 * cell).min(len)));
        }
        near.sort_unstable();
        near.dedup();
        for j in near {
            if j <= i {
                continue;
            }
            compared += 1;
            if compared > MAX_COMPARED {
                return (0, 0.0);
            }
            let Some(o) = walls.get(j) else { continue };
            let lined = in_line(w, o, z);
            if let Some(Some((a, b))) = lined
                && bridges.len() < MAX_WALLS
            {
                bridges.push((a, b, w.thickness.max(o.thickness), i));
            }
            let meets = lined.is_some()
                || end_meets(w.a, w, o, z)
                || end_meets(w.b, w, o, z)
                || end_meets(o.a, o, w, z)
                || end_meets(o.b, o, w, z)
                || crosses(w, o);
            if meets {
                join(&mut parent, i, j);
            }
        }
    }
    if !bridges.is_empty() {
        let bgrid = WallGrid::of(bridges.iter().map(|b| (b.0, b.1)), cell);
        for i in 0..n {
            let Some(w) = walls.get(i) else { continue };
            for p in [w.a, w.b] {
                for &bi in bgrid.at(p) {
                    let Some(&(a, b, t, j)) = bridges.get(bi) else { continue };
                    let opening = PlanWall { a, b, thickness: t, ..w.clone() };
                    if end_meets(p, w, &opening, z) && seg_dist(p, a, b) <= 0.5 * (w.thickness + t) + MEET_SLACK_IN * z.k {
                        join(&mut parent, i, j);
                    }
                }
            }
        }
    }
    let roots: Vec<usize> = (0..n).map(|i| root(&mut parent, i)).collect();
    let door_grid = WallGrid::of(doors.iter().take(MAX_WALLS * 4).map(|d| (*d, *d)), cell);
    // Each group: its length, its extents, whether a door holds it, whether all of it may go.
    struct Group {
        length: f64,
        lo: Vec2,
        hi: Vec2,
        held: bool,
        droppable: bool,
        members: Vec<usize>,
    }
    let mut groups: std::collections::HashMap<usize, Group> = std::collections::HashMap::new();
    for (i, w) in walls.iter().enumerate() {
        let r = roots.get(i).copied().unwrap_or(i);
        let half = 0.5 * w.thickness;
        let g = groups.entry(r).or_insert(Group {
            length: 0.0,
            lo: Vec2::new(f64::INFINITY, f64::INFINITY),
            hi: Vec2::new(f64::NEG_INFINITY, f64::NEG_INFINITY),
            held: false,
            droppable: true,
            members: Vec::new(),
        });
        g.members.push(i);
        g.length += w.a.dist(w.b);
        g.lo = g.lo.min(w.a.min(w.b) - Vec2::new(half, half));
        g.hi = g.hi.max(w.a.max(w.b) + Vec2::new(half, half));
        g.droppable &= i < droppable && w.edit.is_none() && w.basis.is_none();
        let reach = half + DOOR_REACH_IN * z.k;
        g.held |= [w.a, w.b].iter().any(|p| door_grid.at(*p).iter().any(|&d| doors.get(d).is_some_and(|d| d.dist(*p) <= reach)));
    }
    // The building: the longest group. Free groups stand inside it, clear of its outside.
    let Some((&main, building)) = groups.iter().max_by(|a, b| a.1.length.total_cmp(&b.1.length)) else { return (0, 0.0) };
    let (blo, bhi) = (building.lo + Vec2::new(z.shell_clear, z.shell_clear), building.hi - Vec2::new(z.shell_clear, z.shell_clear));
    if !(blo.x < bhi.x && blo.y < bhi.y) {
        return (0, 0.0);
    }
    let free = |r: usize| {
        r != main
            && groups.get(&r).is_some_and(|g| {
                g.droppable
                    && !g.held
                    && !walls_in_a_room(walls, &g.members, z.room_min)
                    && (g.hi.x - g.lo.x).max(g.hi.y - g.lo.y) <= z.max_side
                    && g.lo.x >= blo.x
                    && g.lo.y >= blo.y
                    && g.hi.x <= bhi.x
                    && g.hi.y <= bhi.y
            })
    };
    let free: std::collections::HashSet<usize> = groups.keys().copied().filter(|&r| free(r)).collect();
    let mut dropped = (0usize, 0.0f64);
    let mut i = 0usize;
    walls.retain(|w| {
        let gone = roots.get(i).is_some_and(|r| free.contains(r));
        i += 1;
        if gone {
            dropped.0 += 1;
            dropped.1 += w.a.dist(w.b);
        }
        !gone
    });
    dropped
}

#[cfg(test)]
mod tests {
    use super::*;
    use cadaddy_doc::Handle;

    fn w(a: (f64, f64), b: (f64, f64), t: f64) -> PlanWall {
        PlanWall {
            a: Vec2::new(a.0, a.1),
            b: Vec2::new(b.0, b.1),
            thickness: t,
            top: 0.0,
            source: Handle(0),
            cuts: Vec::new(),
            extra: Vec::new(),
            wall_type: None,
            fixed_top: false,
            head: None,
            basis: None,
            edit: None,
        }
    }

    /// Inches, as a plan drawn at 1" a unit.
    fn sizes() -> Sizes {
        Sizes { max_side: 16.0 * 12.0, shell_clear: 24.0, room_min: 36.0, max_opening: 16.0 * 12.0, corner_post: 12.0, k: 1.0 }
    }

    /// A 40' x 30' shell (6" walls, centrelines), a partition across it with a door gap.
    fn shell() -> Vec<PlanWall> {
        vec![
            w((0.0, 0.0), (480.0, 0.0), 6.0),
            w((480.0, 0.0), (480.0, 360.0), 6.0),
            w((480.0, 360.0), (0.0, 360.0), 6.0),
            w((0.0, 360.0), (0.0, 0.0), 6.0),
            // The partition at x = 240, broken for a 3' door at y 150..186.
            w((240.0, 3.0), (240.0, 150.0), 4.5),
            w((240.0, 186.0), (240.0, 357.0), 4.5),
        ]
    }

    #[test]
    fn shelving_standing_free_in_a_room_is_no_wall() {
        let mut walls = shell();
        let real = walls.len();
        // Shelving: an 18" x 4' unit, an L of two 12" units meeting at a corner, three 2'
        // units in a row; a 6" x 30" filled box (a column's casing, a piece of equipment).
        walls.push(w((300.0, 100.0), (348.0, 100.0), 18.0));
        walls.push(w((300.0, 200.0), (360.0, 200.0), 12.0));
        walls.push(w((366.0, 194.0), (366.0, 150.0), 12.0));
        for x in [60.0, 90.0, 120.0] {
            walls.push(w((x, 260.0), (x + 24.0, 260.0), 12.0));
        }
        walls.push(w((150.0, 80.0), (150.0, 110.0), 6.0));
        let n = walls.len();
        let (dropped, length) = drop_free_standing(&mut walls, n, &[], &sizes());
        assert_eq!(dropped, n - real, "{walls:?}");
        assert!((length - (48.0 + 60.0 + 44.0 + 72.0 + 30.0)).abs() < 1e-9, "{length}");
        assert_eq!(walls.len(), real);
    }

    #[test]
    fn a_wall_meeting_another_or_holding_a_door_stays() {
        let mut walls = shell();
        // A stub off the partition (a tee), a closet's two walls off the shell's north wall,
        // a wall in line with the partition across a 4' opening, a stub holding a door.
        walls.push(w((242.25, 60.0), (300.0, 60.0), 4.5));
        walls.push(w((400.0, 357.0), (400.0, 300.0), 4.5));
        walls.push(w((400.0, 300.0), (440.0, 300.0), 4.5));
        walls.push(w((100.0, 3.0), (100.0, 80.0), 4.5));
        walls.push(w((100.0, 128.0), (100.0, 200.0), 4.5));
        walls.push(w((160.0, 200.0), (160.0, 260.0), 4.5));
        let n = walls.len();
        let door = [Vec2::new(162.0, 262.0)];
        assert_eq!(drop_free_standing(&mut walls, n, &door, &sizes()).0, 0, "{walls:?}");
        // A wall ending on the line of a wide opening between two lined-up walls runs up to it.
        walls.push(w((3.0, 300.0), (60.0, 300.0), 4.5));
        walls.push(w((180.0, 300.0), (237.75, 300.0), 4.5));
        walls.push(w((120.0, 300.0), (120.0, 230.0), 4.5));
        let n = walls.len();
        assert_eq!(drop_free_standing(&mut walls, n, &door, &sizes()).0, 0, "{walls:?}");
        // Without its door, the stub goes; the rest are joined to the building.
        assert_eq!(drop_free_standing(&mut walls, n, &[], &sizes()).0, 1);
        assert_eq!(walls.len(), n - 1);
    }

    #[test]
    fn free_pieces_by_the_shell_long_ones_and_kept_walls_stay() {
        let mut walls = shell();
        // A piece just inside the outside wall, along it (by the shell: part of the outside,
        // a furring wall, not a room's equipment).
        walls.push(w((10.0, 100.0), (10.0, 140.0), 6.0));
        // A free wall longer than equipment (a screen wall across a room).
        walls.push(w((300.0, 120.0), (300.0, 320.0), 6.0));
        // A free piece the caller keeps (a single-line wall, a jamb).
        walls.push(w((60.0, 100.0), (100.0, 100.0), 6.0));
        let n = walls.len();
        assert_eq!(drop_free_standing(&mut walls, n - 1, &[], &sizes()).0, 0, "{walls:?}");
    }

    #[test]
    fn a_shaft_or_closet_standing_free_walls_in_a_room_and_stays() {
        let mut walls = shell();
        // A 7' x 6' shaft of 8" walls, its east side open; a 2' x 2' box of 3" walls (equipment).
        walls.push(w((300.0, 100.0), (384.0, 100.0), 8.0));
        walls.push(w((300.0, 172.0), (384.0, 172.0), 8.0));
        walls.push(w((300.0, 100.0), (300.0, 172.0), 8.0));
        walls.push(w((100.0, 100.0), (124.0, 100.0), 3.0));
        walls.push(w((100.0, 124.0), (124.0, 124.0), 3.0));
        walls.push(w((100.0, 100.0), (100.0, 124.0), 3.0));
        walls.push(w((124.0, 100.0), (124.0, 124.0), 3.0));
        let n = walls.len();
        assert_eq!(drop_free_standing(&mut walls, n, &[], &sizes()).0, 4, "{walls:?}");
        assert_eq!(walls.len(), n - 4);
        assert!(walls.iter().any(|x| x.a == Vec2::new(300.0, 100.0)));
    }

    #[test]
    fn hostile_sizes_drop_nothing() {
        let mut walls = shell();
        walls.push(w((300.0, 100.0), (348.0, 100.0), 18.0));
        let n = walls.len();
        let mut z = sizes();
        z.k = f64::NAN;
        assert_eq!(drop_free_standing(&mut walls, n, &[], &z).0, 0);
        assert_eq!(drop_free_standing(&mut walls[..3].to_vec(), 3, &[], &sizes()).0, 0);
    }
}
