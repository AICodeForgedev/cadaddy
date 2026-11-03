//! A level's building outline from its walls: the floor plate inside the exterior walls' outside
//! faces (Q6023). An L or U plan's slab is its L or U, not the walls' extents; a court open to
//! the sky stays open. The walls are drawn into a grid, small gaps in them (a door the plan
//! breaks its wall for, a corner that does not quite meet) closed, and the grid flooded from
//! outside: what the flood cannot reach is the building.

use std::collections::VecDeque;

use cadaddy_geom::Vec2;
use cadaddy_walk3d::CirculationKind;

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
    building_with(walls, lo, hi, upf, false)
}

/// [`building`] with every gap up to the widest closed ([`CLOSE_FT`]): the whole building, a
/// corridor open at its ends to the outside included (what may stand in it, not its slab).
pub(super) fn building_whole(walls: &[(Vec2, Vec2, f64)], lo: Vec2, hi: Vec2, upf: f64) -> Option<Outline> {
    building_with(walls, lo, hi, upf, true)
}

fn building_with(walls: &[(Vec2, Vec2, f64)], lo: Vec2, hi: Vec2, upf: f64, whole: bool) -> Option<Outline> {
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
    let fill = if whole { fills.last()? } else { fills.iter().find(|g| total(g) as f64 >= 0.8 * widest as f64)? };
    let x_at = |i: usize| (origin.x + i as f64 * cell).clamp(lo.x, hi.x);
    let y_at = |j: usize| (origin.y + j as f64 * cell).clamp(lo.y, hi.y);
    let at = |i: usize, j: usize| i < nx && j < ny && fill.get(j * nx + i).copied().unwrap_or(false);
    let mut out = Outline { rects: rects_of(fill, nx, ny, x_at, y_at)?, edges: Vec::new() };
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

/// The cells of `fill` as rectangles: each row's runs, a run carried up while the rows above
/// repeat it. None past [`MAX_RECTS`].
fn rects_of(fill: &[bool], nx: usize, ny: usize, x_at: impl Fn(usize) -> f64, y_at: impl Fn(usize) -> f64) -> Option<Vec<[Vec2; 2]>> {
    let at = |i: usize, j: usize| i < nx && j < ny && fill.get(j * nx + i).copied().unwrap_or(false);
    let mut rects = Vec::new();
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
                rects.push([Vec2::new(x_at(i0), y_at(j0)), Vec2::new(x_at(i1), y_at(j))]);
            }
        }
        next.extend(runs.into_iter().map(|(i0, i1)| (i0, i1, j)));
        open = next;
        if rects.len() > MAX_RECTS {
            return None;
        }
    }
    rects.retain(|[a, b]| b.x > a.x && b.y > a.y);
    Some(rects)
}

/// Door gaps closed tracing a space, feet each side: a door up to twice as wide.
const DOOR_FT: f64 = 2.0;
/// How far from its label a space's floor is looked for, feet (a label set against its wall).
const LABEL_REACH_FT: f64 = 3.0;
/// The widest corridor, feet: a wider space a label calls a corridor is traced whole.
const CORRIDOR_FT: f64 = 14.0;
/// How far a corridor's walls may step out along it, feet (a door's alcove does not widen it).
const CORRIDOR_JOG_FT: f64 = 1.0;

/// The floor area a space of the kind can have, square feet: less is a symbol's or a tag's box,
/// more a trace that got out (a stair open to its lobby, a hoistway's door wider than a door).
fn space_sf(kind: CirculationKind) -> (f64, f64) {
    match kind {
        CirculationKind::Hoistway => (20.0, 300.0),
        CirculationKind::Stair => (30.0, 800.0),
        CirculationKind::Corridor => (30.0, 20_000.0),
    }
}

/// The thinnest wall round a hoistway, inches: the shaft's rated walls (masonry, shaft wall); the
/// car drawn inside it is thinner.
const HOISTWAY_WALL_IN: f64 = 4.0;

/// The circulation spaces the labels (kind, centre) name on a storey: each traced on the walls'
/// grid inside the shell (rectangles), its door gaps closed — the hoistways first (on their
/// shaft walls, [`HOISTWAY_WALL_IN`]), then the corridors, then the stairs, each in what those
/// before it leave. A corridor runs along its axis between its walls (through a lobby or a
/// stair's landing it opens onto, not across it), so a stair open to it stays in its own
/// enclosure. Why not, for a label on a wall or in a space already traced, and for a trace
/// bigger or smaller than its kind's ([`space_sf`]).
pub(super) fn spaces(
    walls: &[(Vec2, Vec2, f64)],
    shell: &[[Vec2; 2]],
    upf: f64,
    labels: &[(CirculationKind, Vec2)],
) -> Vec<Result<Vec<[Vec2; 2]>, String>> {
    let mut out = vec![Err("no shell to trace in".to_string()); labels.len()];
    if labels.is_empty() || shell.is_empty() || !(upf.is_finite() && upf > 0.0) {
        return out;
    }
    let lo = Vec2::new(shell.iter().map(|r| r[0].x).fold(f64::INFINITY, f64::min), shell.iter().map(|r| r[0].y).fold(f64::INFINITY, f64::min));
    let hi =
        Vec2::new(shell.iter().map(|r| r[1].x).fold(f64::NEG_INFINITY, f64::max), shell.iter().map(|r| r[1].y).fold(f64::NEG_INFINITY, f64::max));
    let (w, h) = (hi.x - lo.x, hi.y - lo.y);
    if !(w > 0.0 && h > 0.0 && w.is_finite() && h.is_finite()) {
        return out;
    }
    let cell = (CELL_FT * upf).max((w * h / MAX_CELLS).sqrt());
    let nx = (w / cell).ceil() as usize + 4;
    let ny = (h / cell).ceil() as usize + 4;
    if nx as f64 * ny as f64 > MAX_CELLS * 1.5 {
        return out;
    }
    let origin = Vec2::new(lo.x - 2.0 * cell, lo.y - 2.0 * cell);
    let Some(wall) = raster(walls, origin, cell, nx, ny) else { return out };
    let thick: Vec<(Vec2, Vec2, f64)> = walls.iter().copied().filter(|w| w.2 >= HOISTWAY_WALL_IN * upf / 12.0).collect();
    let Some(shaft_wall) = raster(&thick, origin, cell, nx, ny) else { return out };
    let centre = |i: usize, j: usize| Vec2::new(origin.x + (i as f64 + 0.5) * cell, origin.y + (j as f64 + 0.5) * cell);
    let outside: Vec<bool> = (0..nx * ny)
        .map(|k| {
            let p = centre(k % nx, k / nx);
            !shell.iter().any(|[a, b]| p.x >= a.x && p.x <= b.x && p.y >= a.y && p.y <= b.y)
        })
        .collect();
    // Blocked: a wall, outside the shell, or a space already traced; for a hoistway, its shaft's
    // walls only.
    let mut blocked: Vec<bool> = wall.iter().zip(&outside).map(|(w, o)| *w || *o).collect();
    let mut shaft_blocked: Vec<bool> = shaft_wall.iter().zip(&outside).map(|(w, o)| *w || *o).collect();
    let r = (DOOR_FT * upf / cell).ceil() as usize;
    let reach = (LABEL_REACH_FT * upf / cell).ceil() as usize;
    let jog = (CORRIDOR_JOG_FT * upf / cell).ceil() as usize;
    let sf = cell * cell / (upf * upf);
    let x_at = |i: usize| origin.x + i as f64 * cell;
    let y_at = |j: usize| origin.y + j as f64 * cell;
    let rank = |k: CirculationKind| match k {
        CirculationKind::Hoistway => 0,
        CirculationKind::Corridor => 1,
        CirculationKind::Stair => 2,
    };
    let mut order: Vec<usize> = (0..labels.len()).collect();
    order.sort_by_key(|i| labels.get(*i).map_or(3, |l| rank(l.0)));
    for n in order {
        let Some(&(kind, at)) = labels.get(n) else { continue };
        let Some(o) = out.get_mut(n) else { continue };
        let (i0, j0) = ((at.x - origin.x) / cell, (at.y - origin.y) / cell);
        if !(i0 >= 0.0 && j0 >= 0.0 && (i0 as usize) < nx && (j0 as usize) < ny) {
            *o = Err("outside the shell".into());
            continue;
        }
        let k0 = j0 as usize * nx + i0 as usize;
        let (here, walls_here) = if kind == CirculationKind::Hoistway { (&shaft_blocked, &shaft_wall) } else { (&blocked, &wall) };
        if here.get(k0).copied().unwrap_or(true) {
            *o = Err(if outside.get(k0).copied().unwrap_or(true) {
                "outside the shell"
            } else if walls_here.get(k0).copied().unwrap_or(true) {
                "on a wall"
            } else {
                "in a space already traced"
            }
            .into());
            continue;
        }
        let grown = grow(here, nx, ny, r);
        // The floor nearest the label, not across a wall from it.
        let Some(start) = nearest(here, &grown, nx, ny, k0, reach) else {
            *o = Err(format!("no floor within {LABEL_REACH_FT}' (walls closer than a door's width)"));
            continue;
        };
        let (si, sj) = (start % nx, start / nx);
        let free = |i: usize, j: usize| !grown.get(j * nx + i).copied().unwrap_or(true);
        let run = |di: isize, dj: isize| {
            let (mut i, mut j, mut n) = (si as isize, sj as isize, 0usize);
            while i + di >= 0 && j + dj >= 0 && ((i + di) as usize) < nx && ((j + dj) as usize) < ny && free((i + di) as usize, (j + dj) as usize) {
                i += di;
                j += dj;
                n += 1;
            }
            n
        };
        // A corridor's band: across its axis, its width at the label (its walls' faces).
        let band: Option<(bool, usize, usize)> = (kind == CirculationKind::Corridor)
            .then(|| {
                let (west, east, south, north) = (run(-1, 0), run(1, 0), run(0, -1), run(0, 1));
                let along_x = west + east >= south + north;
                let (a, b) = if along_x { (sj - south, sj + north) } else { (si - west, si + east) };
                ((b - a + 1 + 2 * r) as f64 * cell <= CORRIDOR_FT * upf).then_some((along_x, a.saturating_sub(jog), b + jog))
            })
            .flatten();
        let in_band = |i: usize, j: usize, widen: usize| {
            band.is_none_or(|(along_x, a, b)| {
                let c = if along_x { j } else { i };
                c + widen >= a && c <= b + widen
            })
        };
        let mut floor = vec![false; nx * ny];
        let mut queue = VecDeque::from([start]);
        floor[start] = true;
        while let Some(k) = queue.pop_front() {
            let (i, j) = (k % nx, k / nx);
            let mut step = |i: usize, j: usize| {
                let q = j * nx + i;
                if free(i, j) && in_band(i, j, 0) && !floor[q] {
                    floor[q] = true;
                    queue.push_back(q);
                }
            };
            if i > 0 {
                step(i - 1, j);
            }
            if i + 1 < nx {
                step(i + 1, j);
            }
            if j > 0 {
                step(i, j - 1);
            }
            if j + 1 < ny {
                step(i, j + 1);
            }
        }
        // Back out to the walls' faces: the gaps' closing undone.
        let floor: Vec<bool> = grow(&floor, nx, ny, r)
            .into_iter()
            .enumerate()
            .map(|(k, f)| f && !here.get(k).copied().unwrap_or(true) && in_band(k % nx, k / nx, r))
            .collect();
        let area = floor.iter().filter(|f| **f).count() as f64 * sf;
        let (min, max) = space_sf(kind);
        let Some(o) = out.get_mut(n) else { continue };
        if !(min..=max).contains(&area) {
            *o = Err(format!("traced {area:.0} sf, not {min:.0}–{max:.0} sf"));
            continue;
        }
        let Some(rects) = rects_of(&floor, nx, ny, x_at, y_at) else {
            *o = Err("too many pieces".into());
            continue;
        };
        for ((b, sb), f) in blocked.iter_mut().zip(shaft_blocked.iter_mut()).zip(&floor) {
            *b |= *f;
            *sb |= *f;
        }
        *o = Ok(rects);
    }
    out
}

/// The smallest and largest room traced on a storey's floor grid, square feet: less is a symbol's
/// or a tag's box, more a trace that got out through an opening wider than a door.
const ROOM_SF: (f64, f64) = (16.0, 3_000.0);

/// A storey's floor for tracing its rooms: its walls drawn in with their door gaps closed, the
/// shell's outside and its circulation's floor taken. Each room is traced once, from the first
/// label on it; another label on the same floor finds that room again.
pub(crate) struct Floors {
    nx: usize,
    ny: usize,
    cell: f64,
    origin: Vec2,
    upf: f64,
    /// A wall, outside the shell, or circulation.
    blocked: Vec<bool>,
    grown: Vec<bool>,
    /// Which traced room (1-based) a cell is on; 0 none.
    owner: Vec<u32>,
    rooms: Vec<Vec<[Vec2; 2]>>,
}

impl Floors {
    /// The floor of the storey the walls (centreline ends and thickness) stand on inside its shell
    /// (rectangles), less what is `taken` (its circulation's rectangles). None for a hostile or
    /// vast plan.
    pub(crate) fn new(walls: &[(Vec2, Vec2, f64)], shell: &[[Vec2; 2]], upf: f64, taken: &[[Vec2; 2]]) -> Option<Floors> {
        if shell.is_empty() || !(upf.is_finite() && upf > 0.0) {
            return None;
        }
        let lo = Vec2::new(shell.iter().map(|r| r[0].x).fold(f64::INFINITY, f64::min), shell.iter().map(|r| r[0].y).fold(f64::INFINITY, f64::min));
        let hi =
            Vec2::new(shell.iter().map(|r| r[1].x).fold(f64::NEG_INFINITY, f64::max), shell.iter().map(|r| r[1].y).fold(f64::NEG_INFINITY, f64::max));
        let (w, h) = (hi.x - lo.x, hi.y - lo.y);
        if !(w > 0.0 && h > 0.0 && w.is_finite() && h.is_finite()) {
            return None;
        }
        let cell = (CELL_FT * upf).max((w * h / MAX_CELLS).sqrt());
        let (nx, ny) = ((w / cell).ceil() as usize + 4, (h / cell).ceil() as usize + 4);
        if nx as f64 * ny as f64 > MAX_CELLS * 1.5 {
            return None;
        }
        let origin = Vec2::new(lo.x - 2.0 * cell, lo.y - 2.0 * cell);
        let wall = raster(walls, origin, cell, nx, ny)?;
        let inside = |p: Vec2, rects: &[[Vec2; 2]]| rects.iter().any(|[a, b]| p.x >= a.x && p.x <= b.x && p.y >= a.y && p.y <= b.y);
        let blocked: Vec<bool> = (0..nx * ny)
            .map(|k| {
                let p = Vec2::new(origin.x + ((k % nx) as f64 + 0.5) * cell, origin.y + ((k / nx) as f64 + 0.5) * cell);
                wall.get(k).copied().unwrap_or(true) || !inside(p, shell) || inside(p, taken)
            })
            .collect();
        let grown = grow(&blocked, nx, ny, (DOOR_FT * upf / cell).ceil() as usize);
        Some(Floors { nx, ny, cell, origin, upf, blocked, grown, owner: vec![0; nx * ny], rooms: Vec::new() })
    }

    /// The room whose floor holds `at` (a room label): which one (traced in order) and its
    /// rectangles, traced the first time a label is on it. Why not, for a label on a wall, outside
    /// the shell or on the circulation, and for a trace smaller or bigger than a room ([`ROOM_SF`]).
    pub(crate) fn room_at(&mut self, at: Vec2) -> Result<(usize, &[[Vec2; 2]]), String> {
        let (nx, ny, cell) = (self.nx, self.ny, self.cell);
        let (i0, j0) = ((at.x - self.origin.x) / cell, (at.y - self.origin.y) / cell);
        if !(i0 >= 0.0 && j0 >= 0.0 && (i0 as usize) < nx && (j0 as usize) < ny) {
            return Err("outside the shell".into());
        }
        let k0 = j0 as usize * nx + i0 as usize;
        if self.blocked.get(k0).copied().unwrap_or(true) {
            return Err("on a wall, the circulation or outside the shell".into());
        }
        let known = |owner: &[u32], k: usize| owner.get(k).copied().filter(|o| *o > 0).map(|o| o as usize - 1);
        if let Some(n) = known(&self.owner, k0) {
            return Ok((n, &self.rooms[n]));
        }
        let r = (DOOR_FT * self.upf / cell).ceil() as usize;
        let reach = (LABEL_REACH_FT * self.upf / cell).ceil() as usize;
        let Some(start) = nearest(&self.blocked, &self.grown, nx, ny, k0, reach) else {
            return Err(format!("no floor within {LABEL_REACH_FT}' (walls closer than a door's width)"));
        };
        if let Some(n) = known(&self.owner, start) {
            return Ok((n, &self.rooms[n]));
        }
        let mut floor = vec![false; nx * ny];
        let mut queue = VecDeque::from([start]);
        floor[start] = true;
        while let Some(k) = queue.pop_front() {
            let (i, j) = (k % nx, k / nx);
            for (a, b) in [(i.wrapping_sub(1), j), (i + 1, j), (i, j.wrapping_sub(1)), (i, j + 1)] {
                let q = b * nx + a;
                if a < nx && b < ny && !self.grown[q] && !floor[q] {
                    floor[q] = true;
                    queue.push_back(q);
                }
            }
        }
        // Back out to the walls' faces; a cell another room already has stays its.
        let floor: Vec<bool> = grow(&floor, nx, ny, r)
            .into_iter()
            .enumerate()
            .map(|(k, f)| f && !self.blocked.get(k).copied().unwrap_or(true) && self.owner.get(k) == Some(&0))
            .collect();
        let area = floor.iter().filter(|f| **f).count() as f64 * cell * cell / (self.upf * self.upf);
        if !(ROOM_SF.0..=ROOM_SF.1).contains(&area) {
            return Err(format!("traced {area:.0} sf, not {:.0}–{:.0} sf", ROOM_SF.0, ROOM_SF.1));
        }
        let (ox, oy) = (self.origin.x, self.origin.y);
        let rects = rects_of(&floor, nx, ny, |i| ox + i as f64 * cell, |j| oy + j as f64 * cell).ok_or("too many pieces")?;
        let n = self.rooms.len();
        let id = u32::try_from(n + 1).map_err(|_| "too many rooms".to_string())?;
        for (o, f) in self.owner.iter_mut().zip(&floor) {
            if *f {
                *o = id;
            }
        }
        self.rooms.push(rects);
        Ok((n, &self.rooms[n]))
    }

    /// The floor no label traced, once every label is: each pocket of it (open floor between the
    /// walls, its door gaps not closed) that touches one traced room only, holds none of
    /// `keep_out` (labels that traced nothing: a room of their own) and is no bigger than
    /// [`POCKET_SF`] joins that room — a guest room's bath behind its door, its entry the closing
    /// of a narrow passage cut off. Each changed room (which one, its rectangles now) (Q6160).
    pub(crate) fn join_pockets(&mut self, keep_out: &[Vec2]) -> Vec<(usize, &[[Vec2; 2]])> {
        let (nx, ny, cell) = (self.nx, self.ny, self.cell);
        let max_cells = (POCKET_SF * self.upf * self.upf / (cell * cell)).ceil() as usize;
        let cell_of = |p: Vec2| {
            let (i, j) = ((p.x - self.origin.x) / cell, (p.y - self.origin.y) / cell);
            (i >= 0.0 && j >= 0.0 && (i as usize) < nx && (j as usize) < ny).then(|| j as usize * nx + i as usize)
        };
        let kept: std::collections::HashSet<usize> = keep_out.iter().filter_map(|p| cell_of(*p)).collect();
        let open = |owner: &[u32], k: usize| !self.blocked.get(k).copied().unwrap_or(true) && owner.get(k) == Some(&0);
        // Pockets meet through openings, not through a sliver of floor a cell or two wide left
        // along a wall (between a wall and the corridor traced beside it): that joins every
        // unit's bath along the corridor into one pocket.
        let sliver = grow(&self.blocked, nx, ny, POCKET_SLIVER_CELLS);
        let wide = |owner: &[u32], k: usize| open(owner, k) && !sliver.get(k).copied().unwrap_or(true);
        let mut seen = vec![false; nx * ny];
        let mut changed: std::collections::BTreeSet<usize> = std::collections::BTreeSet::new();
        for k0 in 0..nx * ny {
            if seen.get(k0).copied().unwrap_or(true) || !wide(&self.owner, k0) {
                continue;
            }
            let mut pocket = vec![k0];
            let mut neighbours: std::collections::BTreeSet<u32> = std::collections::BTreeSet::new();
            let mut queue = VecDeque::from([k0]);
            if let Some(s) = seen.get_mut(k0) {
                *s = true;
            }
            while let Some(k) = queue.pop_front() {
                let (i, j) = (k % nx, k / nx);
                for (a, b) in [(i.wrapping_sub(1), j), (i + 1, j), (i, j.wrapping_sub(1)), (i, j + 1)] {
                    if a >= nx || b >= ny {
                        continue;
                    }
                    let q = b * nx + a;
                    match self.owner.get(q).copied() {
                        Some(o) if o > 0 => {
                            neighbours.insert(o);
                        }
                        _ if wide(&self.owner, q) && seen.get(q) == Some(&false) => {
                            if let Some(s) = seen.get_mut(q) {
                                *s = true;
                            }
                            pocket.push(q);
                            queue.push_back(q);
                        }
                        _ => {}
                    }
                }
            }
            let [only] = neighbours.into_iter().collect::<Vec<_>>()[..] else { continue };
            if pocket.len() > max_cells || pocket.iter().any(|k| kept.contains(k)) {
                continue;
            }
            // The pocket and the floor along its walls the sliver test left out.
            let mut mine = vec![false; nx * ny];
            for k in &pocket {
                if let Some(m) = mine.get_mut(*k) {
                    *m = true;
                }
            }
            let reach = grow(&mine, nx, ny, POCKET_SLIVER_CELLS);
            for (k, o) in self.owner.iter_mut().enumerate() {
                if *o == 0 && reach.get(k).copied().unwrap_or(false) && !self.blocked.get(k).copied().unwrap_or(true) {
                    *o = only;
                }
            }
            changed.insert(only as usize - 1);
        }
        let (ox, oy) = (self.origin.x, self.origin.y);
        let mut done = Vec::new();
        for n in changed {
            let id = n as u32 + 1;
            let fill: Vec<bool> = self.owner.iter().map(|o| *o == id).collect();
            if let (Some(rects), Some(slot)) = (rects_of(&fill, nx, ny, |i| ox + i as f64 * cell, |j| oy + j as f64 * cell), self.rooms.get_mut(n)) {
                *slot = rects;
                done.push(n);
            }
        }
        done.into_iter().filter_map(|n| self.rooms.get(n).map(|r| (n, r.as_slice()))).collect()
    }

    /// A traced room's rectangles now.
    pub(crate) fn rects(&self, n: usize) -> Option<&[[Vec2; 2]]> {
        self.rooms.get(n).map(Vec::as_slice)
    }

    /// The part of traced room `n` inside `outline` (a closed room the trace got into: its own
    /// walls drawn, its door gap wider than the trace closes) made a room of its own: which one.
    /// None when either part would be smaller than a room ([`ROOM_SF`]); nothing changes then.
    pub(crate) fn carve(&mut self, n: usize, outline: &[Vec2]) -> Option<usize> {
        let (nx, ny, cell) = (self.nx, self.ny, self.cell);
        let id = u32::try_from(n + 1).ok()?;
        let new = self.rooms.len();
        let new_id = u32::try_from(new + 1).ok()?;
        if self.rooms.get(n).is_none() || outline.len() < 3 {
            return None;
        }
        let centre = |k: usize| Vec2::new(self.origin.x + ((k % nx) as f64 + 0.5) * cell, self.origin.y + ((k / nx) as f64 + 0.5) * cell);
        let moved: Vec<usize> =
            (0..nx * ny).filter(|&k| self.owner.get(k) == Some(&id) && cadaddy_geom::point_in_polygon(outline, centre(k))).collect();
        let kept = self.owner.iter().filter(|o| **o == id).count().saturating_sub(moved.len());
        let sf = cell * cell / (self.upf * self.upf);
        if (moved.len() as f64 * sf) < ROOM_SF.0 || (kept as f64 * sf) < ROOM_SF.0 {
            return None;
        }
        let (ox, oy) = (self.origin.x, self.origin.y);
        let mut fill_new = vec![false; nx * ny];
        for &k in &moved {
            if let Some(f) = fill_new.get_mut(k) {
                *f = true;
            }
        }
        let fill_old: Vec<bool> = self.owner.iter().enumerate().map(|(k, o)| *o == id && !fill_new.get(k).copied().unwrap_or(false)).collect();
        let rects_new = rects_of(&fill_new, nx, ny, |i| ox + i as f64 * cell, |j| oy + j as f64 * cell)?;
        let rects_old = rects_of(&fill_old, nx, ny, |i| ox + i as f64 * cell, |j| oy + j as f64 * cell)?;
        for &k in &moved {
            if let Some(o) = self.owner.get_mut(k) {
                *o = new_id;
            }
        }
        if let Some(slot) = self.rooms.get_mut(n) {
            *slot = rects_old;
        }
        self.rooms.push(rects_new);
        Some(new)
    }
}

/// Cells a pocket's floor must keep from the walls to reach on ([`Floors::join_pockets`]).
const POCKET_SLIVER_CELLS: usize = 2;

/// The biggest pocket of untraced floor a room takes in ([`Floors::join_pockets`]), square feet:
/// a bath, an entry, a closet or a vestibule; more is a room the labels missed.
const POCKET_SF: f64 = 150.0;

/// The cell nearest `k0` (within `reach` cells, through open cells) that is open once the gaps
/// are closed (`grown` clear).
fn nearest(blocked: &[bool], grown: &[bool], nx: usize, ny: usize, k0: usize, reach: usize) -> Option<usize> {
    let (i0, j0) = (k0 % nx, k0 / nx);
    let mut seen = std::collections::HashSet::from([k0]);
    let mut queue = VecDeque::from([k0]);
    while let Some(k) = queue.pop_front() {
        if !grown.get(k).copied().unwrap_or(true) {
            return Some(k);
        }
        let (i, j) = (k % nx, k / nx);
        let near = [(i.wrapping_sub(1), j), (i + 1, j), (i, j.wrapping_sub(1)), (i, j + 1)];
        for (a, b) in near {
            if a < nx && b < ny && a.abs_diff(i0).max(b.abs_diff(j0)) <= reach {
                let q = b * nx + a;
                if !blocked.get(q).copied().unwrap_or(true) && seen.insert(q) {
                    queue.push_back(q);
                }
            }
        }
    }
    None
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
    fn the_whole_building_holds_its_corridor_open_at_both_ends() {
        // Two rows of rooms 200' × 30', a 6' corridor between them open at both ends.
        let mut walls = ring(&[(0.25, 0.25), (199.75, 0.25), (199.75, 29.75), (0.25, 29.75)]);
        walls.extend(ring(&[(0.25, 36.25), (199.75, 36.25), (199.75, 65.75), (0.25, 65.75)]));
        let (lo, hi) = (Vec2::new(0.0, 0.0), Vec2::new(2400.0, 792.0));
        let slab = building(&walls, lo, hi, 12.0).unwrap();
        let whole = building_whole(&walls, lo, hi, 12.0).unwrap();
        // The slab is the two rows; the whole building has the corridor between them too.
        assert!(sf(&slab) < 200.0 * 61.0, "{} sf", sf(&slab));
        let want = 200.0 * 66.0;
        assert!((sf(&whole) - want).abs() < 0.02 * want, "{} sf, want {want}", sf(&whole));
    }

    /// 6" walls on centrelines (feet, 12 units per foot).
    fn wall(a: (f64, f64), b: (f64, f64)) -> (Vec2, Vec2, f64) {
        (Vec2::new(a.0 * 12.0, a.1 * 12.0), Vec2::new(b.0 * 12.0, b.1 * 12.0), 6.0)
    }

    /// A storey 130' × 46': a stair (0–10') behind a 3' door at the west end of a 6' corridor
    /// (y 20–26') between two rows of rooms with 3' doors onto it, a second stair (10–20') beside
    /// the corridor with its landing open onto it, the corridor open at its east end into a lobby
    /// (100–130'), and an 8' × 8' hoistway in the lobby with a 4' door and its car drawn inside
    /// (2" lines).
    fn storey() -> (Vec<(Vec2, Vec2, f64)>, Vec<[Vec2; 2]>) {
        let mut w = ring(&[(0.0, 0.0), (130.0, 0.0), (130.0, 46.0), (0.0, 46.0)]);
        // The corridor's walls, broken for the rooms' doors.
        w.extend([wall((10.0, 20.0), (30.0, 20.0)), wall((33.0, 20.0), (60.0, 20.0)), wall((63.0, 20.0), (100.0, 20.0))]);
        // None between the corridor and the second stair.
        w.extend([wall((20.0, 26.0), (40.0, 26.0)), wall((43.0, 26.0), (100.0, 26.0)), wall((20.0, 26.0), (20.0, 46.0))]);
        // The car: 2" lines 1' in from the shaft's walls.
        let car = |a: (f64, f64), b: (f64, f64)| (Vec2::new(a.0 * 12.0, a.1 * 12.0), Vec2::new(b.0 * 12.0, b.1 * 12.0), 2.0);
        w.extend([
            car((111.0, 31.0), (117.0, 31.0)),
            car((117.0, 31.0), (117.0, 37.0)),
            car((117.0, 37.0), (111.0, 37.0)),
            car((111.0, 37.0), (111.0, 31.0)),
        ]);
        // The rooms' party walls.
        for x in [40.0, 70.0] {
            w.push(wall((x, 0.0), (x, 20.0)));
            w.push(wall((x, 26.0), (x, 46.0)));
        }
        // The stair's wall and its door onto the corridor; the lobby's walls stop at the corridor.
        w.extend([wall((10.0, 0.0), (10.0, 21.5)), wall((10.0, 24.5), (10.0, 46.0))]);
        w.extend([wall((100.0, 0.0), (100.0, 20.0)), wall((100.0, 26.0), (100.0, 46.0))]);
        // The hoistway: its door (4') on the north side.
        w.extend([wall((110.0, 30.0), (118.0, 30.0)), wall((110.0, 30.0), (110.0, 38.0)), wall((118.0, 30.0), (118.0, 38.0))]);
        w.extend([wall((110.0, 38.0), (112.0, 38.0)), wall((116.0, 38.0), (118.0, 38.0))]);
        (w, vec![[Vec2::new(-3.0, -3.0), Vec2::new(130.0 * 12.0 + 3.0, 46.0 * 12.0 + 3.0)]])
    }

    fn sf_of(rects: &[[Vec2; 2]]) -> f64 {
        rects.iter().map(|[a, b]| (b.x - a.x) * (b.y - a.y)).sum::<f64>() / 144.0
    }

    fn extents(rects: &[[Vec2; 2]]) -> [f64; 4] {
        rects.iter().fold([f64::INFINITY, f64::INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY], |m, [a, b]| {
            [m[0].min(a.x / 12.0), m[1].min(a.y / 12.0), m[2].max(b.x / 12.0), m[3].max(b.y / 12.0)]
        })
    }

    #[test]
    fn the_stair_and_hoistway_close_at_their_doors_and_the_corridor_runs_between_its_walls() {
        let (walls, shell) = storey();
        let p = |x: f64, y: f64| Vec2::new(x * 12.0, y * 12.0);
        let labels = [
            (CirculationKind::Corridor, p(50.0, 23.0)),
            (CirculationKind::Stair, p(5.0, 10.0)),
            (CirculationKind::Hoistway, p(114.0, 34.0)),
            (CirculationKind::Stair, p(15.0, 40.0)),
        ];
        let found = spaces(&walls, &shell, 12.0, &labels);
        let [Ok(corridor), Ok(stair), Ok(hoistway), Ok(open_stair)] = &found[..] else { panic!("{found:?}") };
        // The stair open onto the corridor: its own enclosure (9.5' × 19.75' less the corridor's
        // 1' of give), not the corridor.
        let want = 9.5 * 18.75;
        assert!((sf_of(open_stair) - want).abs() < 0.1 * want, "open stair {} sf, want {want}", sf_of(open_stair));
        assert!(extents(open_stair)[1] > 26.0, "{:?}", extents(open_stair));
        // The stair: inside its walls (9.5' × 45.5'), the door onto the corridor closed.
        let want = 9.5 * 45.5;
        assert!((sf_of(stair) - want).abs() < 0.05 * want, "stair {} sf, want {want}", sf_of(stair));
        // The hoistway: 7.5' × 7.5' inside its shaft's walls (not the car), its door closed (the
        // lobby is not in it).
        let want = 7.5 * 7.5;
        assert!((sf_of(hoistway) - want).abs() < 0.1 * want, "hoistway {} sf, want {want}", sf_of(hoistway));
        // The corridor: from the stair's door to the lobby's far wall along its band, the rooms'
        // doors closed, not spread across the lobby.
        let e = extents(corridor);
        assert!(e[0] > 9.0 && e[0] < 11.5 && e[2] > 128.0, "corridor {e:?}");
        assert!(e[1] > 18.5 && e[3] < 27.5, "corridor {e:?}");
        assert!(sf_of(corridor) < 120.0 * 7.5, "corridor {} sf", sf_of(corridor));
    }

    #[test]
    fn a_room_is_its_floor_inside_its_walls_and_two_labels_in_it_find_it_once() {
        let (walls, shell) = storey();
        let p = |x: f64, y: f64| Vec2::new(x * 12.0, y * 12.0);
        let corridor = spaces(&walls, &shell, 12.0, &[(CirculationKind::Corridor, p(50.0, 23.0))]);
        let taken = corridor[0].clone().unwrap();
        let mut f = Floors::new(&walls, &shell, 12.0, &taken).unwrap();
        // A guest room between the party walls at 40' and 70', its door onto the corridor closed.
        let (a, rects) = f.room_at(p(55.0, 10.0)).unwrap();
        let want = 29.5 * 19.5;
        assert!((sf_of(rects) - want).abs() < 0.05 * want, "room {} sf, want {want}", sf_of(rects));
        // Its number elsewhere in it finds it again; the next room is its own; the corridor is taken.
        assert_eq!(f.room_at(p(65.0, 5.0)).unwrap().0, a);
        let (b, _) = f.room_at(p(85.0, 10.0)).unwrap();
        assert_ne!(a, b);
        assert!(f.room_at(p(50.0, 23.0)).is_err());
    }

    #[test]
    fn a_label_on_a_wall_or_in_a_space_already_traced_traces_nothing() {
        let (walls, shell) = storey();
        let p = |x: f64, y: f64| Vec2::new(x * 12.0, y * 12.0);
        let labels = [
            // On the stair's wall: a tag, not the stair's label.
            (CirculationKind::Stair, p(10.0, 30.0)),
            (CirculationKind::Stair, p(5.0, 10.0)),
            // The stair labelled twice.
            (CirculationKind::Stair, p(5.0, 40.0)),
            // A "corridor" label in a guest room is a room 30' × 20': it traces the room whole,
            // not a band (wider than a corridor), and is a corridor's size.
            (CirculationKind::Hoistway, p(55.0, 10.0)),
        ];
        let found = spaces(&walls, &shell, 12.0, &labels);
        assert!(found[1].is_ok(), "{found:?}");
        assert_eq!(
            (found[0].as_ref().err().map(String::as_str), found[2].as_ref().err().map(String::as_str)),
            (Some("on a wall"), Some("in a space already traced"))
        );
        // A "hoistway" the size of a guest room is no hoistway.
        assert!(found[3].as_ref().is_err_and(|e| e.starts_with("traced 5")), "{:?}", found[3].as_ref().map(|r| sf_of(r)));
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

    /// A room 30' × 20' (6" walls, feet × 12 units) with a bath 8' × 8' in its corner behind
    /// a 3' door gap, and, when `passage`, a 4' passage between it and a second room.
    fn bath_plan(passage: bool) -> (Vec<(Vec2, Vec2, f64)>, Vec<[Vec2; 2]>) {
        let p = |x: f64, y: f64| Vec2::new(x * 12.0, y * 12.0);
        let mut walls = ring(&[(0.0, 0.0), (30.0, 0.0), (30.0, 20.0), (0.0, 20.0)]);
        // The bath: its wall at x 22 with the door gap from y 3 to y 6, its top at y 8.
        walls.push((p(22.0, 0.0), p(22.0, 3.0), 6.0));
        walls.push((p(22.0, 6.0), p(22.0, 8.0), 6.0));
        walls.push((p(22.0, 8.0), p(30.0, 8.0), 6.0));
        if passage {
            // Two rooms each side of a passage 4' wide (x 11–15), each with a door onto it.
            for x in [11.0, 15.0] {
                walls.push((p(x, 0.0), p(x, 9.0), 6.0));
                walls.push((p(x, 12.0), p(x, 20.0), 6.0));
            }
        }
        (walls, vec![[p(0.0, 0.0), p(30.0, 20.0)]])
    }

    #[test]
    fn a_room_takes_in_its_bath_behind_its_door() {
        let (walls, shell) = bath_plan(false);
        let p = |x: f64, y: f64| Vec2::new(x * 12.0, y * 12.0);
        let mut f = Floors::new(&walls, &shell, 12.0, &[]).unwrap();
        let (n, rects) = f.room_at(p(10.0, 12.0)).unwrap();
        let before = sf_of(rects);
        // The door gap closed: the room stops at the bath's walls (its 64 sf out).
        assert!(before < 30.0 * 20.0 - 50.0, "{before} sf");
        let joined = f.join_pockets(&[]);
        let [(m, rects)] = joined[..] else { panic!("one room changed: {joined:?}") };
        assert_eq!(m, n);
        let after = sf_of(rects);
        assert!(after > before + 50.0 && after < 30.0 * 20.0, "{before} → {after} sf");
        // The bath is the room's now: its label there finds the room.
        assert_eq!(f.room_at(p(26.0, 4.0)).unwrap().0, n);
    }

    #[test]
    fn a_closet_the_trace_got_into_is_carved_out_of_the_room() {
        let p = |x: f64, y: f64| Vec2::new(x * 12.0, y * 12.0);
        // A room 30' × 20' with a closet (x 22–30, y 0–8) whose opening (y 2–8, 6') is wider
        // than the trace closes: the room's trace takes the closet in.
        let mut walls = ring(&[(0.0, 0.0), (30.0, 0.0), (30.0, 20.0), (0.0, 20.0)]);
        walls.push((p(22.0, 0.0), p(22.0, 2.0), 6.0));
        walls.push((p(22.0, 8.0), p(30.0, 8.0), 6.0));
        let shell = vec![[p(0.0, 0.0), p(30.0, 20.0)]];
        let mut f = Floors::new(&walls, &shell, 12.0, &[]).unwrap();
        let (n, rects) = f.room_at(p(10.0, 12.0)).unwrap();
        let before = sf_of(rects);
        assert_eq!(f.room_at(p(26.0, 4.0)).unwrap().0, n, "the closet is in the room's trace");
        // Its drawn outline carved out: a room of its own, the room's floor less it.
        let closet = [p(22.0, 0.0), p(30.0, 0.0), p(30.0, 8.0), p(22.0, 8.0)];
        let c = f.carve(n, &closet).expect("carved");
        assert_ne!(c, n);
        let (room, cl) = (sf_of(f.rects(n).unwrap()), sf_of(f.rects(c).unwrap()));
        assert!((40.0..=64.0).contains(&cl), "closet {cl} sf");
        assert!((room + cl - before).abs() < 1.0 && room < before - 40.0, "{before} → {room} + {cl}");
        assert_eq!(f.room_at(p(26.0, 4.0)).unwrap().0, c, "its label finds the closet");
        assert_eq!(f.room_at(p(10.0, 12.0)).unwrap().0, n);
        // Carving all of a room, or a sliver smaller than a room, changes nothing.
        let all = [p(-1.0, -1.0), p(31.0, -1.0), p(31.0, 21.0), p(-1.0, 21.0)];
        assert_eq!(f.carve(n, &all), None);
        let sliver = [p(1.0, 10.0), p(3.0, 10.0), p(3.0, 12.0), p(1.0, 12.0)];
        assert_eq!(f.carve(n, &sliver), None);
        assert!((sf_of(f.rects(n).unwrap()) - room).abs() < 1e-9);
    }

    #[test]
    fn a_pocket_with_its_own_label_or_two_rooms_round_it_stays_out() {
        let p = |x: f64, y: f64| Vec2::new(x * 12.0, y * 12.0);
        // A label in the bath that traced nothing keeps it out of the room.
        let (walls, shell) = bath_plan(false);
        let mut f = Floors::new(&walls, &shell, 12.0, &[]).unwrap();
        f.room_at(p(10.0, 12.0)).unwrap();
        assert!(f.join_pockets(&[p(26.0, 4.0)]).is_empty());
        // A passage between two rooms is neither's.
        let (walls, shell) = bath_plan(true);
        let mut f = Floors::new(&walls, &shell, 12.0, &[]).unwrap();
        let (a, _) = f.room_at(p(5.0, 10.0)).unwrap();
        let (b, before) = f.room_at(p(18.0, 15.0)).unwrap();
        let before = sf_of(before);
        assert_ne!(a, b);
        let joined = f.join_pockets(&[]);
        // The right room takes its bath only; the passage stays out of both.
        assert!(joined.iter().all(|(m, _)| *m == b), "{joined:?}");
        let after = joined.first().map_or(before, |(_, r)| sf_of(r));
        assert!(after < before + 8.0 * 8.0 + 10.0, "{before} → {after}: the passage went in too");
        assert!(f.room_at(p(13.0, 4.0)).is_err() || f.room_at(p(13.0, 4.0)).is_ok_and(|(m, _)| m != a && m != b));
    }
}
