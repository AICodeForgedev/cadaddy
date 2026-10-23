//! A coordination section box (drawing units, on the sheet) applied to a Walk scene (feet, each
//! level's plan placed at the building origin): only what stands inside the box is kept.

use crate::derive::{LevelInfo, Scene, Vertex};

impl Scene {
    /// This scene with only the triangles and edges whose centre is inside the section box
    /// `lo`..`hi` (drawing units of the sheets: a scene point `p` on level `i` is at
    /// `levels[i].base + p * units_per_foot` on its sheet, its height `p.z * units_per_foot`).
    /// Owners and parts stay with their triangles; the rest of the scene is kept as it is.
    pub fn inside_box(&self, lo: [f64; 3], hi: [f64; 3]) -> Scene {
        self.inside_box_of(&self.levels, self.units_per_foot, lo, hi)
    }

    /// [`Scene::inside_box`] for a part of a scene (Walk's shown triangles) placed on the sheets
    /// by the whole building's `levels` and `units_per_foot`.
    pub fn inside_box_of(&self, levels: &[LevelInfo], units_per_foot: f64, lo: [f64; 3], hi: [f64; 3]) -> Scene {
        let upf = if units_per_foot.is_finite() && units_per_foot > 0.0 { units_per_foot } else { 1.0 };
        let inside = |vs: &[Vertex]| {
            let n = vs.len().max(1) as f64;
            let c = |k: usize| vs.iter().map(|v| f64::from(v.p.get(k).copied().unwrap_or(0.0))).sum::<f64>() / n;
            let (x, y, z) = (c(0), c(1), c(2));
            let (bx, by) = level_at(levels, z).map_or((0.0, 0.0), |l| (l.base.x, l.base.y));
            let p = [bx + x * upf, by + y * upf, z * upf];
            (0..3).all(|k| match (p.get(k), lo.get(k), hi.get(k)) {
                (Some(v), Some(a), Some(b)) => *v >= *a && *v <= *b,
                _ => false,
            })
        };
        let mut out = self.clone();
        out.tris.clear();
        out.owners.clear();
        out.parts.clear();
        for (i, tri) in self.tris.as_chunks::<3>().0.iter().enumerate() {
            if inside(tri) {
                out.tris.extend_from_slice(tri);
                if let Some(o) = self.owners.get(i) {
                    out.owners.push(*o);
                }
                if let Some(p) = self.parts.get(i) {
                    out.parts.push(*p);
                }
            }
        }
        out.edges = self.edges.as_chunks::<2>().0.iter().filter(|e| inside(e.as_slice())).flatten().copied().collect();
        out
    }
}

/// The level a height (feet) stands on: the highest whose floor is at or below it (just under a
/// floor counts as that floor), else the lowest.
fn level_at(levels: &[LevelInfo], z: f64) -> Option<&LevelInfo> {
    levels.iter().filter(|l| l.elevation_ft - 0.05 <= z).max_by(|a, b| a.elevation_ft.total_cmp(&b.elevation_ft)).or_else(|| levels.first())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::derive::{Owner, Part};
    use cadaddy_doc::Handle;
    use cadaddy_geom::Vec2;

    fn v(x: f32, y: f32, z: f32) -> Vertex {
        Vertex { p: [x, y, z], n: [0.0, 0.0, 1.0], c: [200, 200, 200, 255] }
    }

    fn level(name: &str, elevation_ft: f64, base: Vec2) -> LevelInfo {
        LevelInfo { name: name.into(), elevation_ft, height_ft: 10.0, ceiling_ft: 9.0, min: Vec2::ZERO, max: Vec2::new(100.0, 100.0), base }
    }

    #[test]
    fn walk_scene_keeps_what_is_inside_the_box() {
        let mut s = Scene { units_per_foot: 12.0, ..Scene::default() };
        // Level 2's plan sits 2000 units to the right on the sheet.
        s.levels = vec![level("Level 1", 0.0, Vec2::ZERO), level("Level 2", 10.0, Vec2::new(2000.0, 0.0))];
        let mut tri = |a: [f32; 3], h: u64| {
            for k in 0..3 {
                let dx = if k == 1 { 1.0 } else { 0.0 };
                let dy = if k == 2 { 1.0 } else { 0.0 };
                s.tris.push(v(a[0] + dx, a[1] + dy, a[2]));
            }
            s.owners.push(Owner { src: 0, handle: Handle(h) });
            s.parts.push(Part::Wall);
        };
        tri([5.0, 5.0, 4.0], 1); // level 1, near the origin
        tri([50.0, 5.0, 4.0], 2); // level 1, far east
        tri([5.0, 5.0, 14.0], 3); // level 2, near its base
        s.edges = vec![v(5.0, 5.0, 4.0), v(6.0, 5.0, 4.0), v(50.0, 5.0, 4.0), v(51.0, 5.0, 4.0)];
        // A box round the first ten feet of level 1 (sheet units), floor to ceiling.
        let only = s.inside_box([0.0, 0.0, 0.0], [120.0, 120.0, 108.0]);
        assert_eq!(only.tris.len(), 3);
        assert_eq!(only.owners, vec![Owner { src: 0, handle: Handle(1) }]);
        assert_eq!(only.parts.len(), 1);
        assert_eq!(only.edges.len(), 2);
        // The same corner of level 2, on its own part of the sheet and at its height.
        let up = s.inside_box([2000.0, 0.0, 120.0], [2120.0, 120.0, 240.0]);
        assert_eq!(up.owners, vec![Owner { src: 0, handle: Handle(3) }]);
        // A part of the scene, placed by the whole building's levels.
        let part = Scene { tris: s.tris.clone(), owners: s.owners.clone(), ..Scene::default() };
        let cut = part.inside_box_of(&s.levels, s.units_per_foot, [2000.0, 0.0, 120.0], [2120.0, 120.0, 240.0]);
        assert_eq!(cut.owners, vec![Owner { src: 0, handle: Handle(3) }]);
        // A box round everything keeps everything.
        let all = s.inside_box([-1e6; 3], [1e6; 3]);
        assert_eq!(all.tris.len(), s.tris.len());
        assert_eq!(all.edges.len(), s.edges.len());
    }
}
