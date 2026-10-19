//! What the walker stands on: every upward-facing triangle of the scene (floors, stair treads,
//! ramps, the tops of low solids), indexed on a plan grid so finding the surface under a point is
//! quick.

use std::collections::HashMap;

use crate::derive::Scene;
use crate::math::V3;
use cadaddy_doc::geom::Vec2;

/// Grid cell size, feet.
const CELL: f64 = 4.0;
/// Faces this close to level count as walkable (normal z at least this).
const WALKABLE: f32 = 0.5;
/// Most cells one triangle is entered in (a huge floor slab spans many).
const MAX_CELLS_PER_TRI: i64 = 40_000;

#[derive(Clone, Debug, Default)]
pub struct Ground {
    tris: Vec<[V3; 3]>,
    cells: HashMap<(i64, i64), Vec<u32>>,
}

fn cell(v: f64) -> i64 {
    let c = (v / CELL).floor();
    if c.is_finite() { c.clamp(-1e12, 1e12) as i64 } else { 0 }
}

impl Ground {
    pub fn new(scene: &Scene) -> Ground {
        let mut g = Ground::default();
        for t in scene.tris.as_chunks::<3>().0 {
            if t.iter().any(|v| v.n[2] < WALKABLE) {
                continue;
            }
            let p: [V3; 3] = t.map(|v| V3 { x: f64::from(v.p[0]), y: f64::from(v.p[1]), z: f64::from(v.p[2]) });
            if !p.iter().all(|v| v.x.is_finite() && v.y.is_finite() && v.z.is_finite()) {
                continue;
            }
            let (x0, x1) = (cell(p[0].x.min(p[1].x).min(p[2].x)), cell(p[0].x.max(p[1].x).max(p[2].x)));
            let (y0, y1) = (cell(p[0].y.min(p[1].y).min(p[2].y)), cell(p[0].y.max(p[1].y).max(p[2].y)));
            if (x1 - x0 + 1).saturating_mul(y1 - y0 + 1) > MAX_CELLS_PER_TRI || g.tris.len() >= u32::MAX as usize {
                continue;
            }
            let id = g.tris.len() as u32;
            g.tris.push(p);
            for cx in x0..=x1 {
                for cy in y0..=y1 {
                    g.cells.entry((cx, cy)).or_default().push(id);
                }
            }
        }
        g
    }

    /// The highest surface under plan point `p` that is no higher than `limit`.
    pub fn below(&self, p: Vec2, limit: f64) -> Option<f64> {
        let ids = self.cells.get(&(cell(p.x), cell(p.y)))?;
        let mut best: Option<f64> = None;
        for id in ids {
            let Some(t) = self.tris.get(*id as usize) else { continue };
            let Some(z) = height_in(t, p) else { continue };
            if z <= limit + 1e-6 && best.is_none_or(|b| z > b) {
                best = Some(z);
            }
        }
        best
    }

    pub fn is_empty(&self) -> bool {
        self.tris.is_empty()
    }
}

/// The height of triangle `t` above plan point `p`, when `p` is inside it.
fn height_in(t: &[V3; 3], p: Vec2) -> Option<f64> {
    let [a, b, c] = *t;
    let d = (b.y - c.y) * (a.x - c.x) + (c.x - b.x) * (a.y - c.y);
    if d.abs() < 1e-12 {
        return None;
    }
    let l1 = ((b.y - c.y) * (p.x - c.x) + (c.x - b.x) * (p.y - c.y)) / d;
    let l2 = ((c.y - a.y) * (p.x - c.x) + (a.x - c.x) * (p.y - c.y)) / d;
    let l3 = 1.0 - l1 - l2;
    let eps = -1e-9;
    (l1 >= eps && l2 >= eps && l3 >= eps).then_some(l1 * a.z + l2 * b.z + l3 * c.z)
}
