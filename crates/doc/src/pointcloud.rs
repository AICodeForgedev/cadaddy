//! Point clouds: the POINTCLOUD object (a reference to a scan file, placed, clipped and coloured
//! in the drawing; the points stay in the file) and the points themselves once read, sorted into
//! an octree for level-of-detail display.
//!
//! The octree is a linear one: points sorted in Morton (Z-order) of their cell at the deepest
//! level, so every node at every level is one run of the array. A node's first points, taken
//! with a stride, stand for it at coarse levels; `lod` walks the tree breadth first until the
//! point budget is spent, so a whole cloud draws with a fixed number of points and zooming in
//! brings the detail of what is on screen.

use std::sync::Arc;

use cadaddy_geom::{Bounds2, Vec2, Vec3};
use serde::{Deserialize, Serialize};

/// How points are coloured.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ColorMode {
    /// The scan's colours (grey from intensity when it has none).
    #[default]
    Rgb,
    Intensity,
    /// Blue (low) to red (high).
    Elevation,
    /// The object's colour.
    Single,
}

/// A POINTCLOUD object.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PointCloud {
    /// The scan file (LAS, PTS, XYZ).
    pub path: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub name: String,
    /// Where the cloud's origin goes.
    pub insert: Vec3,
    #[serde(default = "one")]
    pub scale: f64,
    /// Radians about Z.
    #[serde(default)]
    pub rotation: f64,
    /// A clipping box in the cloud's own coordinates (min, max); `None`: all of it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub clip: Option<[Vec3; 2]>,
    #[serde(default)]
    pub color_mode: ColorMode,
    /// The bounds of the cloud's points (its own coordinates), kept so the object has extents
    /// without reading the file.
    #[serde(default)]
    pub bounds: [Vec3; 2],
    /// Points in the file (shown in Properties).
    #[serde(default)]
    pub count: u64,
}

fn one() -> f64 {
    1.0
}

impl PointCloud {
    /// A point of the cloud (its own coordinates) in the drawing.
    pub fn place(&self, p: Vec3) -> Vec3 {
        let (s, c) = self.rotation.sin_cos();
        let q = Vec3::new(p.x * self.scale, p.y * self.scale, p.z * self.scale);
        Vec3::new(self.insert.x + q.x * c - q.y * s, self.insert.y + q.x * s + q.y * c, self.insert.z + q.z)
    }

    /// A drawing point in the cloud's own coordinates.
    pub fn unplace(&self, p: Vec3) -> Vec3 {
        let (s, c) = (-self.rotation).sin_cos();
        let (dx, dy) = (p.x - self.insert.x, p.y - self.insert.y);
        let k = if self.scale.abs() > 1e-300 { 1.0 / self.scale } else { 1.0 };
        Vec3::new((dx * c - dy * s) * k, (dx * s + dy * c) * k, (p.z - self.insert.z) * k)
    }

    /// The box the object covers in the drawing (clipped).
    pub fn extents(&self) -> Bounds2 {
        let [lo, hi] = self.clip.unwrap_or(self.bounds);
        let corners = [Vec3::new(lo.x, lo.y, lo.z), Vec3::new(hi.x, lo.y, lo.z), Vec3::new(hi.x, hi.y, lo.z), Vec3::new(lo.x, hi.y, lo.z)];
        Bounds2::from_points(corners.into_iter().map(|c| self.place(c).xy()))
    }

    /// Whether a point (own coordinates) is inside the clipping box.
    pub fn shows(&self, p: Vec3) -> bool {
        match &self.clip {
            None => true,
            Some([lo, hi]) => p.x >= lo.x && p.x <= hi.x && p.y >= lo.y && p.y <= hi.y && p.z >= lo.z && p.z <= hi.z,
        }
    }
}

/// The points of a scan.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Cloud {
    pub points: Vec<[f32; 3]>,
    /// Per point, when the file has colours.
    pub colors: Vec<[u8; 3]>,
    /// Per point, 0..=65535, when the file has intensities.
    pub intensity: Vec<u16>,
    /// Octree runs: per level, (start, len, Morton prefix) of each non-empty node, in Morton
    /// order.
    levels: Vec<Vec<(u32, u32, u64)>>,
    pub min: [f32; 3],
    pub max: [f32; 3],
    /// The octree's cube side (the largest extent).
    side: f32,
}

/// The octree's deepest level (8^10 cells).
pub const MAX_DEPTH: u32 = 10;

/// The Morton code of a cell (x, y, z each 0..1024).
fn morton(x: u32, y: u32, z: u32) -> u64 {
    let mut code = 0u64;
    for b in 0..MAX_DEPTH {
        code |= (u64::from((x >> b) & 1)) << (3 * b);
        code |= (u64::from((y >> b) & 1)) << (3 * b + 1);
        code |= (u64::from((z >> b) & 1)) << (3 * b + 2);
    }
    code
}

impl Cloud {
    /// Sort the points into the octree. Colours and intensities follow their points.
    pub fn build(mut self) -> Cloud {
        let n = self.points.len();
        if n == 0 {
            return self;
        }
        let mut min = [f32::MAX; 3];
        let mut max = [f32::MIN; 3];
        for p in &self.points {
            for k in 0..3 {
                min[k] = min[k].min(p[k]);
                max[k] = max[k].max(p[k]);
            }
        }
        let side = (0..3).map(|k| max[k] - min[k]).fold(0.0f32, f32::max).max(1e-9);
        let cells = (1u32 << MAX_DEPTH) as f32;
        let cell = |v: f32, k: usize| (((v - min[k]) / side) * cells).clamp(0.0, cells - 1.0) as u32;
        let mut keyed: Vec<(u64, u32)> =
            self.points.iter().enumerate().map(|(i, p)| (morton(cell(p[0], 0), cell(p[1], 1), cell(p[2], 2)), i as u32)).collect();
        // A stable order inside a node, spread through it, so a prefix samples the node evenly.
        keyed.sort_by_key(|(k, i)| (*k, i.reverse_bits()));
        let order: Vec<usize> = keyed.iter().map(|(_, i)| *i as usize).collect();
        fn pick<T: Copy>(v: &[T], order: &[usize]) -> Vec<T> {
            order.iter().filter_map(|i| v.get(*i).copied()).collect()
        }
        self.points = pick(&self.points, &order);
        if self.colors.len() == n {
            self.colors = pick(&self.colors, &order);
        } else {
            self.colors.clear();
        }
        if self.intensity.len() == n {
            self.intensity = pick(&self.intensity, &order);
        } else {
            self.intensity.clear();
        }
        self.levels = (0..=MAX_DEPTH)
            .map(|level| {
                let shift = 3 * (MAX_DEPTH - level);
                let mut runs: Vec<(u32, u32, u64)> = Vec::new();
                let mut prev: Option<u64> = None;
                for (i, (k, _)) in keyed.iter().enumerate() {
                    let node = k >> shift;
                    if prev == Some(node) {
                        if let Some(r) = runs.last_mut() {
                            r.1 += 1;
                        }
                    } else {
                        runs.push((i as u32, 1, node));
                        prev = Some(node);
                    }
                }
                runs
            })
            .collect();
        self.min = min;
        self.max = max;
        self.side = side;
        self
    }

    /// The plan box of node `code` at `level`.
    fn node_box(&self, level: usize, code: u64) -> (Vec2, Vec2) {
        let (mut x, mut y) = (0u64, 0u64);
        for b in 0..level {
            x |= ((code >> (3 * b)) & 1) << b;
            y |= ((code >> (3 * b + 1)) & 1) << b;
        }
        let size = f64::from(self.side) / (1u64 << level.min(62)) as f64;
        let lo = Vec2::new(f64::from(self.min[0]) + x as f64 * size, f64::from(self.min[1]) + y as f64 * size);
        (lo, lo + Vec2::new(size, size))
    }

    fn node_meets(&self, level: usize, code: u64, (lo, hi): (Vec2, Vec2)) -> bool {
        let (a, b) = self.node_box(level, code);
        a.x <= hi.x && b.x >= lo.x && a.y <= hi.y && b.y >= lo.y
    }

    /// Up to `max` points inside the plan box (own coordinates): the nodes of the level whose
    /// cells are about the box's size that meet it, every point of them tested.
    pub fn points_in(&self, lo: Vec2, hi: Vec2, max: usize) -> Vec<usize> {
        let span = (hi.x - lo.x).max(hi.y - lo.y).max(1e-12);
        let level = ((f64::from(self.side) / span).log2().floor().max(0.0) as usize).min(self.levels.len().saturating_sub(1));
        let Some(runs) = self.levels.get(level) else { return Vec::new() };
        let mut out = Vec::new();
        for (s, l, code) in runs {
            if !self.node_meets(level, *code, (lo, hi)) {
                continue;
            }
            for i in *s as usize..(*s + *l) as usize {
                if self.in_view(i, (lo, hi)) {
                    out.push(i);
                    if out.len() >= max {
                        return out;
                    }
                }
            }
        }
        out
    }

    /// Nodes at each level (for tests and statistics).
    pub fn level_sizes(&self) -> Vec<usize> {
        self.levels.iter().map(Vec::len).collect()
    }

    /// Indices of about `budget` points standing for the cloud, with more detail where `view`
    /// (own coordinates, plan) looks; the coarsest level that fills the budget, each node sampled
    /// in proportion to its points.
    pub fn lod(&self, budget: usize, view: Option<(Vec2, Vec2)>) -> Vec<usize> {
        let n = self.points.len();
        if n <= budget || self.levels.is_empty() {
            return (0..n).filter(|i| view.is_none_or(|v| self.in_view(*i, v))).collect();
        }
        // Points per node, at the deepest level whose node count still fits the budget.
        let level = self.levels.iter().rposition(|l| l.len() <= budget.max(1)).unwrap_or(0);
        let runs = self.levels.get(level).map(Vec::as_slice).unwrap_or_default();
        let visible: Vec<&(u32, u32, u64)> = runs.iter().filter(|(_, _, code)| view.is_none_or(|v| self.node_meets(level, *code, v))).collect();
        let total: usize = visible.iter().map(|(_, l, _)| *l as usize).sum::<usize>().max(1);
        let mut out = Vec::with_capacity(budget);
        for (s, l, _) in visible {
            let take = ((*l as usize * budget).div_ceil(total)).clamp(1, *l as usize);
            out.extend((*s as usize..*s as usize + take).filter(|i| view.is_none_or(|v| self.in_view(*i, v))));
        }
        out
    }

    fn in_view(&self, i: usize, (lo, hi): (Vec2, Vec2)) -> bool {
        self.points.get(i).is_some_and(|p| f64::from(p[0]) >= lo.x && f64::from(p[0]) <= hi.x && f64::from(p[1]) >= lo.y && f64::from(p[1]) <= hi.y)
    }

    /// The colour of point `i` in `mode` (`single` for [`ColorMode::Single`]).
    pub fn color(&self, i: usize, mode: ColorMode, single: [u8; 3]) -> [u8; 3] {
        let grey = |v: u16| {
            let g = (v >> 8) as u8;
            [g, g, g]
        };
        match mode {
            ColorMode::Single => single,
            ColorMode::Rgb => self.colors.get(i).copied().or_else(|| self.intensity.get(i).map(|v| grey(*v))).unwrap_or(single),
            ColorMode::Intensity => self.intensity.get(i).map(|v| grey(*v)).unwrap_or(single),
            ColorMode::Elevation => {
                let z = self.points.get(i).map_or(0.0, |p| p[2]);
                let t = ((z - self.min[2]) / (self.max[2] - self.min[2]).max(1e-9)).clamp(0.0, 1.0);
                // Blue → green → red.
                let (r, g, b) = if t < 0.5 { (0.0, t * 2.0, 1.0 - t * 2.0) } else { ((t - 0.5) * 2.0, 1.0 - (t - 0.5) * 2.0, 0.0) };
                [(r * 255.0) as u8, (g * 255.0) as u8, (b * 255.0) as u8]
            }
        }
    }

    /// The bounds as the object keeps them.
    pub fn bounds(&self) -> [Vec3; 2] {
        let v = |a: [f32; 3]| Vec3::new(f64::from(a[0]), f64::from(a[1]), f64::from(a[2]));
        [v(self.min), v(self.max)]
    }
}

/// Reads a scan file into a [`Cloud`] (installed by the host: the file formats live in `io`).
pub type CloudLoader = fn(&str) -> Option<Arc<Cloud>>;

#[cfg(test)]
mod tests {
    use super::*;

    fn grid(n: usize) -> Cloud {
        let mut points = Vec::new();
        for i in 0..n {
            for j in 0..n {
                points.push([i as f32, j as f32, ((i + j) % 7) as f32]);
            }
        }
        Cloud { points, ..Cloud::default() }.build()
    }

    #[test]
    fn the_octree_counts_and_lod_budget() {
        let c = grid(100);
        let sizes = c.level_sizes();
        assert_eq!(sizes.first(), Some(&1), "one root");
        assert!(sizes.windows(2).all(|w| w[0] <= w[1]), "levels only grow: {sizes:?}");
        assert_eq!(*sizes.last().unwrap(), 10_000, "every point its own cell at the deepest level");
        // Level 1: the 8 octants that hold points (a flat-ish grid fills the lower and upper halves).
        assert!(sizes[1] <= 8);
        for budget in [100, 1000, 5000] {
            let picked = c.lod(budget, None);
            assert!(picked.len() >= budget / 2 && picked.len() <= budget * 2, "{budget}: {}", picked.len());
            let mut u = picked.clone();
            u.sort_unstable();
            u.dedup();
            assert_eq!(u.len(), picked.len(), "no point twice");
        }
        assert_eq!(c.lod(1_000_000, None).len(), 10_000, "a budget over the size: every point");
        // Points in a small box: exactly those.
        let near = c.points_in(Vec2::new(9.5, 19.5), Vec2::new(10.5, 20.5), 100);
        assert_eq!(near.len(), 1);
        assert_eq!(c.points[near[0]][..2], [10.0, 20.0]);
        // A view on a quarter keeps to it.
        let part = c.lod(400, Some((Vec2::new(0.0, 0.0), Vec2::new(49.0, 49.0))));
        assert!(part.iter().all(|i| c.points[*i][0] <= 49.0 && c.points[*i][1] <= 49.0));
        assert!(!part.is_empty());
    }

    #[test]
    fn placing_clipping_and_colours() {
        let pc = PointCloud {
            path: "x.las".into(),
            name: String::new(),
            insert: Vec3::new(10.0, 0.0, 0.0),
            scale: 2.0,
            rotation: std::f64::consts::FRAC_PI_2,
            clip: Some([Vec3::ZERO, Vec3::new(1.0, 1.0, 1.0)]),
            color_mode: ColorMode::Elevation,
            bounds: [Vec3::ZERO, Vec3::new(5.0, 5.0, 5.0)],
            count: 0,
        };
        let p = pc.place(Vec3::new(1.0, 0.0, 0.0));
        assert!((p.x - 10.0).abs() < 1e-9 && (p.y - 2.0).abs() < 1e-9);
        assert!(pc.shows(Vec3::new(0.5, 0.5, 0.5)) && !pc.shows(Vec3::new(2.0, 0.5, 0.5)));
        let c = grid(4);
        assert_eq!(c.color(0, ColorMode::Single, [1, 2, 3]), [1, 2, 3]);
        let lo = c.points.iter().position(|p| p[2] == 0.0).unwrap();
        assert_eq!(c.color(lo, ColorMode::Elevation, [0; 3])[2], 255, "low is blue");
    }
}
