//! Stairs and ramps between levels: the geometry of a flight of steps, its landings, a ramp,
//! their handrails, and the sloped surfaces Walk mode climbs.
//!
//! A stair rises from one floor to the next in equal risers: a run of `n` treads has `n + 1`
//! risers (the last one lands on the landing or the floor above), so a stair of runs
//! `n₁ … nₖ` divides its rise into `Σ (nᵢ + 1)` risers. Code limits checked against are those of
//! the International Code Council's *International Building Code* (2021) §1011.5.2 (risers 4″ to
//! 7″, treads at least 11″), *International Residential Code* (2021) §R311.7.5 (risers at most
//! 7¾″, treads at least 10″), IBC §1012.2 (ramps at most 1:12) and §1014.2 (handrails 34″ to
//! 38″ above the nosings). The walker climbs a flight as the slope through its nosings, the
//! "step offset" a character controller takes stairs with (after Fauerby, "Improved Collision
//! detection and Response", 2003).

use cadaddy_doc::geom::Vec2;
use serde::{Deserialize, Serialize};

/// Handrail height above the nosings (or a ramp's surface), feet: 36″, inside 34″–38″.
pub const HANDRAIL_FT: f64 = 3.0;
/// Code limits, inches.
pub const RISER_MIN_IN: f64 = 4.0;
pub const RISER_MAX_IN: f64 = 7.75;
pub const RISER_MAX_COMMERCIAL_IN: f64 = 7.0;
pub const TREAD_MIN_IN: f64 = 10.0;
/// Steepest ramp: 1 in 12.
pub const RAMP_MAX_SLOPE: f64 = 1.0 / 12.0;

/// One straight run of a stair (or a ramp): its centreline on the sheet from the bottom step to
/// the top, and its treads (none for a ramp).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct StairRun {
    pub from: [f64; 2],
    pub to: [f64; 2],
    #[serde(default)]
    pub treads: u32,
}

/// A stair as Walk mode and the model build need it: its runs on the sheet and the heights it
/// climbs between (feet).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct StairRule {
    pub name: String,
    /// The level it starts on (its plan places it).
    pub level: String,
    pub runs: Vec<StairRun>,
    /// Width, drawing units.
    pub width: f64,
    pub z_bottom_ft: f64,
    pub z_top_ft: f64,
    #[serde(default)]
    pub ramp: bool,
}

/// A walkable surface, feet: the rectangle `half_width` either side of `a`–`b`, at height `za`
/// at `a` rising evenly to `zb` at `b`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Slope {
    pub a: Vec2,
    pub b: Vec2,
    pub half_width: f64,
    pub za: f64,
    pub zb: f64,
}

impl Slope {
    /// The surface's height at `p`, if `p` is over it.
    pub fn height_at(&self, p: Vec2) -> Option<f64> {
        let d = self.b - self.a;
        let len2 = d.dot(d);
        if len2.is_nan() || len2 <= 1e-12 {
            return None;
        }
        let t = (p - self.a).dot(d) / len2;
        let off = (p - self.a).cross(d).abs() / len2.sqrt();
        ((-1e-6..=1.0 + 1e-6).contains(&t) && off <= self.half_width).then(|| self.za + (self.zb - self.za) * t.clamp(0.0, 1.0))
    }
}

/// A solid block, feet: on the centreline `a`–`b`, `width` wide, from `z0` up to `z1` (a step
/// or a landing).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Block {
    pub a: Vec2,
    pub b: Vec2,
    pub width: f64,
    pub z0: f64,
    pub z1: f64,
}

/// A ramp's solid, feet: on `a`–`b`, `width` wide, its top from `za` at `a` to `zb` at `b`, its
/// bottom at `z0`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Wedge {
    pub a: Vec2,
    pub b: Vec2,
    pub width: f64,
    pub z0: f64,
    pub za: f64,
    pub zb: f64,
}

/// What a stair is made of, feet.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct StairGeometry {
    pub blocks: Vec<Block>,
    pub wedges: Vec<Wedge>,
    /// Handrails: (start, end) as (x, y, z).
    pub rails: Vec<([f64; 3], [f64; 3])>,
    pub slopes: Vec<Slope>,
    /// Risers and their height (feet); 0 for a ramp.
    pub risers: u32,
    pub riser_ft: f64,
}

/// Most steps one stair makes (hostile input).
const MAX_STEPS: u32 = 1_000;

impl StairRule {
    /// The total risers (Σ treads + 1 per run).
    pub fn risers(&self) -> u32 {
        if self.ramp {
            return 0;
        }
        self.runs.iter().map(|r| r.treads.min(MAX_STEPS).saturating_add(1)).fold(0u32, u32::saturating_add)
    }

    /// The stair in world feet: the sheet point `base` is the building origin, `upf` drawing
    /// units per foot.
    pub fn geometry(&self, base: Vec2, upf: f64) -> StairGeometry {
        let mut g = StairGeometry::default();
        let upf = if upf.is_finite() && upf > 0.0 { upf } else { 12.0 };
        let w = (self.width / upf).abs();
        let (zb, zt) = (self.z_bottom_ft, self.z_top_ft);
        if !(w > 1e-6 && w.is_finite() && zb.is_finite() && zt.is_finite()) || self.runs.is_empty() {
            return g;
        }
        let world = |p: [f64; 2]| Vec2 { x: (p[0] - base.x) / upf, y: (p[1] - base.y) / upf };
        let rise = zt - zb;
        let risers = self.risers();
        let h = if risers > 0 { rise / f64::from(risers) } else { 0.0 };
        g.risers = risers;
        g.riser_ft = h;
        // Ramps: the rise shared along their runs' lengths.
        let total_len: f64 = self.runs.iter().map(|r| world(r.from).dist(world(r.to))).sum();
        let mut z = zb;
        let n = self.runs.len().min(100);
        for (i, r) in self.runs.iter().take(n).enumerate() {
            let (a, b) = (world(r.from), world(r.to));
            let len = a.dist(b);
            if len.is_nan() || len <= 1e-6 {
                continue;
            }
            let u = (b - a) * (1.0 / len);
            let side = Vec2 { x: -u.y, y: u.x };
            let z_end = if self.ramp { z + rise * len / total_len.max(1e-9) } else { z + h * f64::from(r.treads.min(MAX_STEPS) + 1) };
            if self.ramp {
                g.wedges.push(Wedge { a, b, width: w, z0: zb, za: z, zb: z_end });
            } else {
                let t = len / f64::from(r.treads.max(1));
                for k in 0..r.treads.min(MAX_STEPS) {
                    let (s0, s1) = (f64::from(k) * t, f64::from(k + 1) * t);
                    g.blocks.push(Block { a: a + u * s0, b: a + u * s1, width: w, z0: zb, z1: z + h * f64::from(k + 1) });
                }
            }
            // The walking surface: through the nosings (or along the ramp).
            let first = if self.ramp { z } else { z + h };
            g.slopes.push(Slope { a, b, half_width: w / 2.0, za: z, zb: z_end });
            for sgn in [-1.0, 1.0] {
                let off = side * (sgn * (w / 2.0 - 0.15).max(0.0));
                let (p, q) = (a + off, b + off);
                g.rails.push(([p.x, p.y, first + HANDRAIL_FT], [q.x, q.y, z_end + HANDRAIL_FT]));
            }
            // A landing to the next run: across both ends, as deep as the stair is wide (or up to
            // the next run when it carries straight on).
            if let Some(next) = self.runs.get(i + 1).filter(|_| i + 1 < n) {
                let (na, nb) = (world(next.from), world(next.to));
                let rel = |p: Vec2| ((p - b).dot(u), (p - b).dot(side));
                let ends = [b + side * (w / 2.0), b - side * (w / 2.0)];
                let nlen = na.dist(nb).max(1e-9);
                let nside = Vec2 { x: -(nb - na).y, y: (nb - na).x } * (1.0 / nlen);
                let starts = [na + nside * (w / 2.0), na - nside * (w / 2.0)];
                let pts: Vec<(f64, f64)> = ends.iter().chain(starts.iter()).map(|p| rel(*p)).collect();
                let (lo, hi) = pts.iter().fold((f64::INFINITY, f64::NEG_INFINITY), |(l, h), (_, s)| (l.min(*s), h.max(*s)));
                let ahead = (nb - na).dot(u) / nlen;
                let gap = rel(na).0;
                let depth = if ahead > 0.99 && gap > 0.05 { gap } else { w };
                let mid = (lo + hi) / 2.0;
                let (la, lb) = (b + side * mid, b + side * mid + u * depth);
                g.blocks.push(Block { a: la, b: lb, width: hi - lo, z0: zb, z1: z_end });
                g.slopes.push(Slope { a: la, b: lb, half_width: (hi - lo) / 2.0, za: z_end, zb: z_end });
            }
            z = z_end;
        }
        g
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rule(runs: Vec<StairRun>, ramp: bool) -> StairRule {
        StairRule { name: "S1".into(), level: "L1".into(), runs, width: 48.0, z_bottom_ft: 0.0, z_top_ft: 10.0, ramp }
    }

    #[test]
    fn a_straight_flight_divides_the_rise() {
        // 16 treads of 11" over 14'-8": 17 risers of 7.06".
        let r = rule(vec![StairRun { from: [0.0, 0.0], to: [176.0, 0.0], treads: 16 }], false);
        let g = r.geometry(Vec2 { x: 0.0, y: 0.0 }, 12.0);
        assert_eq!(g.risers, 17);
        assert!((g.riser_ft * 12.0 - 120.0 / 17.0).abs() < 1e-9);
        assert_eq!(g.blocks.len(), 16);
        let top = g.blocks.last().map_or(0.0, |b| b.z1);
        assert!((top - 16.0 * 10.0 / 17.0).abs() < 1e-9);
        let s = g.slopes[0];
        assert_eq!(s.height_at(Vec2 { x: 0.0, y: 0.0 }), Some(0.0));
        assert!((s.height_at(Vec2 { x: 14.0 + 2.0 / 3.0, y: 1.0 }).unwrap_or(0.0) - 10.0).abs() < 1e-9);
        assert_eq!(s.height_at(Vec2 { x: 5.0, y: 3.0 }), None);
        assert_eq!(g.rails.len(), 2);
        assert!((g.rails[0].0[2] - (10.0 / 17.0 + 3.0)).abs() < 1e-9);
    }

    #[test]
    fn a_switchback_lands_halfway() {
        // Two runs of 8 treads side by side, turning on a landing.
        let r =
            rule(vec![StairRun { from: [0.0, 0.0], to: [88.0, 0.0], treads: 8 }, StairRun { from: [88.0, 48.0], to: [0.0, 48.0], treads: 8 }], false);
        let g = r.geometry(Vec2 { x: 0.0, y: 0.0 }, 12.0);
        assert_eq!(g.risers, 18);
        let landing = g.blocks.iter().find(|b| (b.width - 8.0).abs() < 1e-9).copied().unwrap_or(Block {
            a: Vec2 { x: 0.0, y: 0.0 },
            b: Vec2 { x: 0.0, y: 0.0 },
            width: 0.0,
            z0: 0.0,
            z1: 0.0,
        });
        assert!((landing.z1 - 5.0).abs() < 1e-9, "{landing:?}");
        assert!((landing.a.x - 88.0 / 12.0).abs() < 1e-9 && (landing.b.x - 88.0 / 12.0 - 4.0).abs() < 1e-9, "{landing:?}");
        assert!((g.slopes[2].zb - 10.0).abs() < 1e-9);
    }

    #[test]
    fn a_ramp_is_one_slope() {
        let r = StairRule { z_top_ft: 2.0, ..rule(vec![StairRun { from: [0.0, 0.0], to: [288.0, 0.0], treads: 0 }], true) };
        let g = r.geometry(Vec2 { x: 0.0, y: 0.0 }, 12.0);
        assert_eq!((g.risers, g.blocks.len(), g.wedges.len()), (0, 0, 1));
        assert_eq!(g.slopes[0].height_at(Vec2 { x: 12.0, y: 0.0 }), Some(1.0));
    }
}
