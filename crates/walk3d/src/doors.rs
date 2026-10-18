//! Door leaves that open and close: each door the openings pass builds is recorded with its
//! hinge and directions, so clicking it swings the leaf shut (it then stops the walker) or open.

use crate::derive::{Owner, Scene, Seg, boxed};
use crate::math::v3;
use cadaddy_color::Rgb;
use cadaddy_doc::geom::Vec2;

/// Vertices one leaf (a box: 6 faces, 12 triangles) takes in [`Scene::tris`].
pub(crate) const LEAF_VERTS: usize = 36;

/// One door leaf.
#[derive(Clone, Debug, PartialEq)]
pub struct DoorLeaf {
    pub level: usize,
    pub hinge: Vec2,
    /// Unit directions from the hinge: standing open, and shut across the opening.
    pub open_dir: Vec2,
    pub shut_dir: Vec2,
    /// Leaf width, thickness and height (feet), and its bottom.
    pub width: f64,
    pub thick: f64,
    pub height: f64,
    pub z0: f64,
    /// Shifts the shut leaf into the middle of the wall's thickness.
    pub shut_offset: Vec2,
    /// Where its vertices start in [`Scene::tris`].
    pub first_vertex: usize,
    pub color: Rgb,
    pub owner: Owner,
    pub open: bool,
}

impl DoorLeaf {
    /// The leaf's footprint line in plan where it stands now.
    pub fn line(&self) -> (Vec2, Vec2) {
        if self.open {
            (self.hinge, self.hinge + self.open_dir * self.width)
        } else {
            let a = self.hinge + self.shut_offset;
            (a, a + self.shut_dir * self.width)
        }
    }
}

impl Scene {
    /// The door whose leaf triangle `tri` (index into the triangle list) belongs to.
    pub fn door_of_triangle(&self, tri: usize) -> Option<usize> {
        let v = tri.checked_mul(3)?;
        self.door_leaves.iter().position(|d| v >= d.first_vertex && v < d.first_vertex + LEAF_VERTS)
    }

    /// Swing door `i` shut, or open again. Returns whether it is open now.
    pub fn toggle_door(&mut self, i: usize) -> Option<bool> {
        let d = self.door_leaves.get(i)?.clone();
        let open = !d.open;
        let (a, dir) = if open { (d.hinge, d.open_dir) } else { (d.hinge + d.shut_offset, d.shut_dir) };
        let c = a + dir * (d.width / 2.0);
        let mut leaf = Scene::default();
        boxed(&mut leaf, v3(c.x, c.y, d.z0 + d.height / 2.0), dir, d.width, d.thick, d.height, d.color, d.owner);
        let dst = self.tris.get_mut(d.first_vertex..d.first_vertex + LEAF_VERTS)?;
        if leaf.tris.len() != LEAF_VERTS {
            return None;
        }
        dst.copy_from_slice(&leaf.tris);
        if let Some(x) = self.door_leaves.get_mut(i) {
            x.open = open;
        }
        Some(open)
    }

    /// Shut doors as walls the walker can't pass.
    pub fn shut_doors(&self) -> impl Iterator<Item = Seg> + '_ {
        self.door_leaves.iter().filter(|d| !d.open).map(|d| {
            let (a, b) = d.line();
            Seg { a, b, level: d.level }
        })
    }
}
