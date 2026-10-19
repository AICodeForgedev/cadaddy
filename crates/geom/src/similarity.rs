//! Similarity transforms (uniform scale, rotation, translation — no mirror) and a shape signature
//! that doesn't change under them, for finding copies of a group of objects (BLOCKIFY).

use crate::{Mat3, Vec2};

/// `p ↦ scale · R(angle) · p + offset`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Similarity {
    pub scale: f64,
    pub angle: f64,
    pub offset: Vec2,
}

impl Similarity {
    pub const IDENTITY: Similarity = Similarity { scale: 1.0, angle: 0.0, offset: Vec2::ZERO };

    /// The similarity taking `a` to `a2` and `b` to `b2`; `None` when `a` and `b` coincide (or
    /// `a2` and `b2` do, or the numbers aren't finite).
    pub fn from_pairs(a: Vec2, b: Vec2, a2: Vec2, b2: Vec2) -> Option<Similarity> {
        let (d, d2) = (b - a, b2 - a2);
        let (l, l2) = (d.len(), d2.len());
        if !(l > 1e-300 && l2 > 1e-300 && l.is_finite() && l2.is_finite()) {
            return None;
        }
        let scale = l2 / l;
        let angle = d2.angle() - d.angle();
        let offset = a2 - (a * scale).rotate(angle);
        let s = Similarity { scale, angle, offset };
        (scale.is_finite() && angle.is_finite() && offset.is_finite()).then_some(s)
    }

    pub fn apply(&self, p: Vec2) -> Vec2 {
        (p * self.scale).rotate(self.angle) + self.offset
    }

    /// As a matrix (for transforming objects).
    pub fn mat(&self) -> Mat3 {
        Mat3::scale(self.scale, self.scale).then(Mat3::rotate(self.angle)).then(Mat3::translate(self.offset))
    }
}

/// A signature of a point set that a similarity doesn't change: every pairwise distance over the
/// largest, sorted (for at most `MAX` points). Two sets that are similar copies have the same
/// signature (within rounding); sets of different shapes almost never do.
pub fn shape_signature(points: &[Vec2]) -> Vec<f64> {
    const MAX: usize = 64;
    let pts: Vec<Vec2> = points.iter().copied().filter(|p| p.is_finite()).take(MAX).collect();
    let mut d = Vec::with_capacity(pts.len() * pts.len().saturating_sub(1) / 2);
    for (i, a) in pts.iter().enumerate() {
        for b in pts.iter().skip(i + 1) {
            d.push(a.dist(*b));
        }
    }
    let max = d.iter().copied().fold(0.0, f64::max);
    if max > 1e-300 {
        d.iter_mut().for_each(|x| *x /= max);
    }
    d.sort_by(f64::total_cmp);
    d
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_similarity_from_two_pairs_maps_them() {
        let (a, b) = (Vec2::new(1.0, 2.0), Vec2::new(4.0, 6.0));
        let s = Similarity { scale: 2.5, angle: 0.7, offset: Vec2::new(-3.0, 8.0) };
        let t = Similarity::from_pairs(a, b, s.apply(a), s.apply(b)).unwrap();
        assert!((t.scale - 2.5).abs() < 1e-12 && (t.angle - 0.7).abs() < 1e-12 && t.offset.dist(s.offset) < 1e-9);
        let p = Vec2::new(-7.0, 0.5);
        assert!(t.apply(p).dist(t.mat().apply(p)) < 1e-9);
        assert!(Similarity::from_pairs(a, a, a, b).is_none());
        assert!(Similarity::from_pairs(a, b, a, Vec2::new(f64::NAN, 0.0)).is_none());
    }

    #[test]
    fn the_signature_ignores_moves_turns_and_scale() {
        let pts = [Vec2::new(0.0, 0.0), Vec2::new(3.0, 0.0), Vec2::new(3.0, 1.0), Vec2::new(0.5, 2.0)];
        let s = Similarity { scale: 0.3, angle: 2.0, offset: Vec2::new(100.0, -5.0) };
        let moved: Vec<Vec2> = pts.iter().map(|p| s.apply(*p)).collect();
        let (a, b) = (shape_signature(&pts), shape_signature(&moved));
        assert!(a.iter().zip(&b).all(|(x, y)| (x - y).abs() < 1e-12));
        let other = [Vec2::new(0.0, 0.0), Vec2::new(3.0, 0.0), Vec2::new(3.0, 1.2), Vec2::new(0.5, 2.0)];
        assert!(shape_signature(&other).iter().zip(&a).any(|(x, y)| (x - y).abs() > 1e-3));
        assert!(shape_signature(&[]).is_empty());
    }
}
