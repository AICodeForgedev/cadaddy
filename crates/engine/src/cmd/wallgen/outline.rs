//! A level's building outline from its walls: the floor plate inside the exterior walls' outside
//! faces (Q6023). An L or U plan's slab is its L or U, not the walls' extents; a court open to
//! the sky stays open. The walls are drawn into a grid, small gaps in them (a door the plan
//! breaks its wall for, a corner that does not quite meet) closed, and the grid flooded from
//! outside: what the flood cannot reach is the building.

use std::collections::VecDeque;

use cadaddy_geom::Vec2;

/// A grid cell, feet (when it fits [`MAX_CELLS`]).
const CELL_FT: f64 = 0.25;
/// Most grid cells (a hostile or vast plan gets bigger cells).
const MAX_CELLS: f64 = 4_000_000.0;
/// Gaps in the walls closed, feet (each closes a gap twice as wide); the smallest that encloses
/// the building is used, so a notch wider than its gap stays out.
const CLOSE_FT: [f64; 4] = [1.5, 3.0, 6.0, 10.0];
/// Most cells looked at drawing the walls into the grid.
const MAX_TESTS: usize = 200_000_000;
/// Most rectangles an outline is made of.
const MAX_RECTS: usize = 20_000;

/// A floor plate: rectangles (corners, sheet units) and its boundary edges, each run with the
/// plate on its left (counter-clockwise round the plate).
#[derive(Clone, Debug, Default)]
pub(super) struct Outline {
    pub rects: Vec<[Vec2; 2]>,
    pub edges: Vec<(Vec2, Vec2)>,
}

#[cfg(test)]
impl Outline {
    /// The plate's area, sheet units squared.
    pub fn area(&self) -> f64 {
        self.rects.iter().map(|[a, b]| (b.x - a.x) * (b.y - a.y)).sum()
    }
}

/// The outline of the building the walls (centreline ends and thickness, sheet units, `upf` per
/// foot) stand round, inside `lo`–`hi` (their faces' extents); none when they do not enclose it.
pub(super) fn building(walls: &[(Vec2, Vec2, f64)], lo: Vec2, hi: Vec2, upf: f64) -> Option<Outline> {
    let (w, h) = (hi.x - lo.x, hi.y - lo.y);
    if !(w > 0.0 && h > 0.0 && w.is_finite() && h.is_finite() && upf.is_finite() && upf > 0.0) {
        return None;
    }
    let max_close = CLOSE_FT.last().copied().unwrap_or(0.0) * upf;
    let cell = (CELL_FT * upf).max(((w + 2.0 * max_close) * (h + 2.0 * max_close) / MAX_CELLS).sqrt());
    let margin = (max_close / cell).ceil() as usize + 2;
    let nx = (w / cell).ceil() as usize + 2 * margin;
    let ny = (h / cell).ceil() as usize + 2 * margin;
    if nx as f64 * ny as f64 > MAX_CELLS * 1.5 {
        return None;
    }
    let origin = Vec2::new(lo.x - margin as f64 * cell, lo.y - margin as f64 * cell);
    let wall = raster(walls, origin, cell, nx, ny)?;
    let total = |g: &[bool]| g.iter().filter(|v| **v).count();
    let fills: Vec<Vec<bool>> = CLOSE_FT.iter().map(|r| enclosed(&wall, nx, ny, (r * upf / cell).ceil() as usize)).collect();
    let widest = fills.last().map_or(0, |g| total(g));
    // Nothing enclosed even with the widest gaps closed (walls that stand alone, a plan that is
    // all gap): no outline.
    let walls_only = total(&wall);
    if widest == 0 || (widest as f64) < 0.25 * (w * h) / (cell * cell) || widest <= walls_only + walls_only / 2 {
        return None;
    }
    let fill = fills.iter().find(|g| total(g) as f64 >= 0.8 * widest as f64)?;
    let x_at = |i: usize| (origin.x + i as f64 * cell).clamp(lo.x, hi.x);
    let y_at = |j: usize| (origin.y + j as f64 * cell).clamp(lo.y, hi.y);
    let at = |i: usize, j: usize| i < nx && j < ny && fill.get(j * nx + i).copied().unwrap_or(false);
    let mut out = Outline::default();
    // Rectangles: each row's runs, a run carried up while the rows above repeat it.
    let mut open: Vec<(usize, usize, usize)> = Vec::new(); // (i0, i1, j0) — still growing
    for j in 0..=ny {
        let mut runs: Vec<(usize, usize)> = Vec::new();
        let mut i = 0;
        while j < ny && i < nx {
            if at(i, j) {
                let i0 = i;
                while at(i, j) {
                    i += 1;
                }
                runs.push((i0, i));
            } else {
                i += 1;
            }
        }
        let mut next = Vec::new();
        for (i0, i1, j0) in open.drain(..) {
            if let Some(k) = runs.iter().position(|r| *r == (i0, i1)) {
                runs.remove(k);
                next.push((i0, i1, j0));
            } else {
                out.rects.push([Vec2::new(x_at(i0), y_at(j0)), Vec2::new(x_at(i1), y_at(j))]);
            }
        }
        next.extend(runs.into_iter().map(|(i0, i1)| (i0, i1, j)));
        open = next;
        if out.rects.len() > MAX_RECTS {
            return None;
        }
    }
    out.rects.retain(|[a, b]| b.x > a.x && b.y > a.y);
    // Edges: a cell side with no plate across it, joined along its row or column.
    for j in 0..=ny {
        // Below the cells of row j (plate above, none below): runs west to east.
        let mut i = 0;
        while i < nx {
            let edge = |i: usize| at(i, j) != (j > 0 && at(i, j - 1));
            if edge(i) {
                let up = at(i, j);
                let i0 = i;
                while i < nx && edge(i) && at(i, j) == up {
                    i += 1;
                }
                let (a, b) = (Vec2::new(x_at(i0), y_at(j)), Vec2::new(x_at(i), y_at(j)));
                out.edges.push(if up { (a, b) } else { (b, a) });
            } else {
                i += 1;
            }
        }
    }
    for i in 0..=nx {
        let mut j = 0;
        while j < ny {
            let edge = |j: usize| at(i, j) != (i > 0 && at(i - 1, j));
            if edge(j) {
                let right = at(i, j);
                let j0 = j;
                while j < ny && edge(j) && at(i, j) == right {
                    j += 1;
                }
                let (a, b) = (Vec2::new(x_at(i), y_at(j0)), Vec2::new(x_at(i), y_at(j)));
                // Plate to the east: run north to south keeps it on the left.
                out.edges.push(if right { (b, a) } else { (a, b) });
            } else {
                j += 1;
            }
        }
    }
    out.edges.retain(|(a, b)| a.dist(*b) > 0.0);
    (!out.rects.is_empty()).then_some(out)
}

/// The cells whose centre lies within a wall (or the most of a cell from its line, so a thin
/// wall still shows); none past [`MAX_TESTS`] cells looked at.
fn raster(walls: &[(Vec2, Vec2, f64)], origin: Vec2, cell: f64, nx: usize, ny: usize) -> Option<Vec<bool>> {
    let mut g = vec![false; nx * ny];
    let mut tests = 0usize;
    for &(a, b, t) in walls {
        let len = a.dist(b);
        if !(len > 1e-9 && t.is_finite() && t >= 0.0) {
            continue;
        }
        let r = (t / 2.0).max(0.75 * cell);
        let idx = |v: f64, o: f64, n: usize| ((v - o) / cell).floor().clamp(0.0, n.saturating_sub(1) as f64) as usize;
        let (i0, i1) = (idx(a.x.min(b.x) - r, origin.x, nx), idx(a.x.max(b.x) + r, origin.x, nx));
        let (j0, j1) = (idx(a.y.min(b.y) - r, origin.y, ny), idx(a.y.max(b.y) + r, origin.y, ny));
        let u = (b - a) * (1.0 / len);
        tests = tests.saturating_add((i1 - i0 + 1).saturating_mul(j1 - j0 + 1));
        if tests > MAX_TESTS {
            return None;
        }
        for j in j0..=j1 {
            for i in i0..=i1 {
                let p = Vec2::new(origin.x + (i as f64 + 0.5) * cell, origin.y + (j as f64 + 0.5) * cell);
                let s = (p - a).dot(u).clamp(0.0, len);
                if p.dist(a + u * s) <= r
                    && let Some(c) = g.get_mut(j * nx + i)
                {
                    *c = true;
                }
            }
        }
    }
    Some(g)
}

/// The cells inside the walls once their gaps up to `r` cells each side are closed: the walls
/// grown by `r`, the outside flooded from the border, the outside grown back by `r`.
fn enclosed(wall: &[bool], nx: usize, ny: usize, r: usize) -> Vec<bool> {
    let grown = grow(wall, nx, ny, r);
    let mut outside = vec![false; nx * ny];
    let mut queue: VecDeque<usize> = VecDeque::new();
    let seed = |k: usize, outside: &mut Vec<bool>, queue: &mut VecDeque<usize>| {
        if !grown.get(k).copied().unwrap_or(true)
            && let Some(o) = outside.get_mut(k)
            && !*o
        {
            *o = true;
            queue.push_back(k);
        }
    };
    for i in 0..nx {
        seed(i, &mut outside, &mut queue);
        seed((ny.saturating_sub(1)) * nx + i, &mut outside, &mut queue);
    }
    for j in 0..ny {
        seed(j * nx, &mut outside, &mut queue);
        seed(j * nx + nx.saturating_sub(1), &mut outside, &mut queue);
    }
    while let Some(k) = queue.pop_front() {
        let (i, j) = (k % nx, k / nx);
        if i > 0 {
            seed(k - 1, &mut outside, &mut queue);
        }
        if i + 1 < nx {
            seed(k + 1, &mut outside, &mut queue);
        }
        if j > 0 {
            seed(k - nx, &mut outside, &mut queue);
        }
        if j + 1 < ny {
            seed(k + nx, &mut outside, &mut queue);
        }
    }
    grow(&outside, nx, ny, r).into_iter().map(|o| !o).collect()
}

/// `g` grown by `r` cells in every direction (a square), by a summed-area table.
fn grow(g: &[bool], nx: usize, ny: usize, r: usize) -> Vec<bool> {
    let w = nx + 1;
    let mut sum = vec![0u32; w * (ny + 1)];
    for j in 0..ny {
        let mut row = 0u32;
        for i in 0..nx {
            row += u32::from(g.get(j * nx + i).copied().unwrap_or(false));
            let above = sum.get(j * w + i + 1).copied().unwrap_or(0);
            if let Some(s) = sum.get_mut((j + 1) * w + i + 1) {
                *s = above + row;
            }
        }
    }
    let at = |i: usize, j: usize| sum.get(j * w + i).copied().unwrap_or(0);
    let mut out = vec![false; nx * ny];
    for j in 0..ny {
        let (j0, j1) = (j.saturating_sub(r), (j + r + 1).min(ny));
        for i in 0..nx {
            let (i0, i1) = (i.saturating_sub(r), (i + r + 1).min(nx));
            let n = (at(i1, j1) + at(i0, j0)).saturating_sub(at(i0, j1) + at(i1, j0));
            if n > 0
                && let Some(o) = out.get_mut(j * nx + i)
            {
                *o = true;
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Walls 6" thick round a closed loop of corners (feet, 12 units per foot), on their
    /// centrelines 3" in from the outside faces given.
    fn ring(pts: &[(f64, f64)]) -> Vec<(Vec2, Vec2, f64)> {
        let n = pts.len();
        (0..n).map(|k| (pts[k], pts[(k + 1) % n])).map(|(a, b)| (Vec2::new(a.0 * 12.0, a.1 * 12.0), Vec2::new(b.0 * 12.0, b.1 * 12.0), 6.0)).collect()
    }

    fn sf(o: &Outline) -> f64 {
        o.area() / 144.0
    }

    #[test]
    fn an_l_plan_is_its_l_not_its_extents() {
        // Centrelines of an L: 100' × 40' with a 40' × 60' leg; outside faces 3" out.
        let walls = ring(&[(0.25, 0.25), (99.75, 0.25), (99.75, 39.75), (39.75, 39.75), (39.75, 99.75), (0.25, 99.75)]);
        let o = building(&walls, Vec2::new(0.0, 0.0), Vec2::new(1200.0, 1200.0), 12.0).unwrap();
        let want = 100.0 * 40.0 + 40.0 * 60.0;
        assert!((sf(&o) - want).abs() < 0.01 * want, "{} sf, want {want} (extents 10000)", sf(&o));
        // The boundary runs round the L: its length is the L's perimeter.
        let perim: f64 = o.edges.iter().map(|(a, b)| a.dist(*b)).sum::<f64>() / 12.0;
        assert!((perim - 400.0).abs() < 4.0, "{perim}");
    }

    #[test]
    fn a_door_gap_in_the_wall_is_closed_but_a_court_stays_open() {
        // A U: 120' × 80' round a 40' wide court open to the north, with a 3' door gap in the
        // south wall.
        let mut walls =
            ring(&[(0.25, 0.25), (119.75, 0.25), (119.75, 79.75), (80.25, 79.75), (80.25, 30.25), (39.75, 30.25), (39.75, 79.75), (0.25, 79.75)]);
        let (a, b, t) = walls[0];
        walls[0] = (a, Vec2::new(58.0 * 12.0, b.y), t);
        walls.push((Vec2::new(61.0 * 12.0, a.y), b, t));
        let o = building(&walls, Vec2::new(0.0, 0.0), Vec2::new(1440.0, 960.0), 12.0).unwrap();
        let want = 120.0 * 80.0 - 40.0 * 49.5;
        assert!((sf(&o) - want).abs() < 0.01 * want, "{} sf, want {want}", sf(&o));
    }

    #[test]
    fn walls_that_enclose_nothing_give_no_outline() {
        let walls = vec![(Vec2::new(0.0, 0.0), Vec2::new(1200.0, 0.0), 6.0), (Vec2::new(0.0, 600.0), Vec2::new(1200.0, 600.0), 6.0)];
        assert!(building(&walls, Vec2::new(0.0, -3.0), Vec2::new(1200.0, 603.0), 12.0).is_none());
    }

    #[test]
    fn hostile_numbers_do_not_panic() {
        let walls = vec![(Vec2::new(f64::NAN, 0.0), Vec2::new(1e300, 1.0), f64::INFINITY), (Vec2::new(0.0, 0.0), Vec2::new(0.0, 0.0), -1.0)];
        assert!(building(&walls, Vec2::new(0.0, 0.0), Vec2::new(1e300, 1e300), 12.0).is_none());
        assert!(building(&walls, Vec2::new(0.0, 0.0), Vec2::new(1.0, 1.0), f64::NAN).is_none());
        // A vast plan gets coarser cells, not a vast grid.
        let big = ring(&[(0.0, 0.0), (1e6, 0.0), (1e6, 1e6), (0.0, 1e6)]);
        assert!(building(&big, Vec2::new(-3.0, -3.0), Vec2::new(1.2e7 + 3.0, 1.2e7 + 3.0), 12.0).is_some());
    }
}
