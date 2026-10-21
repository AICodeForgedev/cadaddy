//! Clashes computed from the truth's geometry: every pair of modelled elements that overlap (hard
//! clashes; runs or equipment reaching into a beam or column), and runs or equipment hanging below
//! the ceiling of the room they are in. Every element here is axis-aligned, so its box is exact
//! for rectangular ducts and equipment and a close bound for round ducts and pipes.

use crate::design::{depth_in, width_in};
use crate::truth::{Clash, Truth};
use std::collections::BTreeSet;

/// Overlaps smaller than this (feet) are touching, not clashing.
const TOUCH: f64 = 1.0 / 96.0;

#[derive(Clone, Debug)]
struct Item {
    id: String,
    min: [f64; 3],
    max: [f64; 3],
    /// Structure (beams, columns) clashes are `into_structure`.
    structure: bool,
    /// Nodes it is connected through: items sharing one don't clash.
    nodes: Vec<String>,
}

fn items(t: &Truth) -> Vec<Item> {
    let mut out = Vec::new();
    for s in &t.segments {
        let (w, d) = (width_in(&s.shape, s.size_in) / 24.0, depth_in(&s.shape, s.size_in) / 24.0);
        let [a, b] = s.path_ft;
        let along_x = (a[0] - b[0]).abs() > (a[1] - b[1]).abs();
        let (hx, hy, hz) = if s.vertical {
            // A vertical rectangular duct's size is [along x, along y].
            if s.shape == "rect" { (s.size_in[0] / 24.0, s.size_in[1] / 24.0, 0.0) } else { (w, w, 0.0) }
        } else if along_x {
            (0.0, w, d)
        } else {
            (w, 0.0, d)
        };
        out.push(Item {
            id: s.id.clone(),
            min: [a[0].min(b[0]) - hx, a[1].min(b[1]) - hy, a[2].min(b[2]) - hz],
            max: [a[0].max(b[0]) + hx, a[1].max(b[1]) + hy, a[2].max(b[2]) + hz],
            structure: false,
            nodes: vec![s.from.clone(), s.to.clone()],
        });
    }
    for e in &t.equipment {
        out.push(Item { id: e.id.clone(), min: e.min_ft, max: e.max_ft, structure: false, nodes: e.nodes.clone() });
    }
    for b in &t.building.beams {
        let hf = b.flange_in / 24.0;
        let along_x = (b.from_ft[0] - b.to_ft[0]).abs() > (b.from_ft[1] - b.to_ft[1]).abs();
        let (hx, hy) = if along_x { (0.0, hf) } else { (hf, 0.0) };
        out.push(Item {
            id: b.id.clone(),
            min: [b.from_ft[0].min(b.to_ft[0]) - hx, b.from_ft[1].min(b.to_ft[1]) - hy, b.tos_ft - b.depth_in / 12.0],
            max: [b.from_ft[0].max(b.to_ft[0]) + hx, b.from_ft[1].max(b.to_ft[1]) + hy, b.tos_ft],
            structure: true,
            nodes: Vec::new(),
        });
    }
    for c in &t.building.columns {
        let (hx, hy) = (c.footprint_in[0] / 24.0, c.footprint_in[1] / 24.0);
        out.push(Item {
            id: c.id.clone(),
            min: [c.at_ft[0] - hx, c.at_ft[1] - hy, c.bottom_ft],
            max: [c.at_ft[0] + hx, c.at_ft[1] + hy, c.top_ft],
            structure: true,
            nodes: Vec::new(),
        });
    }
    out
}

/// The overlap of two boxes, if every side of it is more than touching.
fn overlap(a: &Item, b: &Item) -> Option<([f64; 3], [f64; 3])> {
    let mut lo = [0.0; 3];
    let mut hi = [0.0; 3];
    for k in 0..3 {
        lo[k] = a.min[k].max(b.min[k]);
        hi[k] = a.max[k].min(b.max[k]);
        if hi[k] - lo[k] <= TOUCH {
            return None;
        }
    }
    Some((lo, hi))
}

/// Every clash, `planted` set for those involving an element in `planted`.
pub fn clashes(t: &Truth, planted: &[String]) -> Vec<Clash> {
    let all = items(t);
    let planted: BTreeSet<&str> = planted.iter().map(String::as_str).collect();
    let mut out = Vec::new();
    let push = |out: &mut Vec<Clash>, kind: &str, a: &str, b: &str, lo: [f64; 3], hi: [f64; 3]| {
        let depth = (0..3).map(|k| hi[k] - lo[k]).fold(f64::INFINITY, f64::min);
        out.push(Clash {
            id: String::new(),
            kind: kind.into(),
            a: a.into(),
            b: b.into(),
            at_ft: [0, 1, 2].map(|k| ((lo[k] + hi[k]) / 2.0 * 1e4).round() / 1e4),
            depth_in: (depth * 12.0 * 100.0).round() / 100.0,
            planted: planted.contains(a) || planted.contains(b),
        });
    };
    for (i, a) in all.iter().enumerate() {
        for b in all.iter().skip(i + 1) {
            if a.structure && b.structure {
                continue;
            }
            if a.nodes.iter().any(|n| b.nodes.contains(n)) {
                continue;
            }
            if let Some((lo, hi)) = overlap(a, b) {
                let kind = if a.structure || b.structure { "into_structure" } else { "hard" };
                push(&mut out, kind, &a.id, &b.id, lo, hi);
            }
        }
    }
    // Below the ceiling: horizontal runs and hung equipment over a room with a ceiling.
    let elev = |code: &str| t.level(code).map_or(0.0, |l| l.elevation_ft);
    for room in &t.building.rooms {
        let Some(c) = &room.ceiling else { continue };
        let floor = elev(&room.level);
        let clg = floor + c.height_ft;
        let inside = |min: [f64; 3], max: [f64; 3]| {
            min[0] < room.max_ft[0] - TOUCH && max[0] > room.min_ft[0] + TOUCH && min[1] < room.max_ft[1] - TOUCH && max[1] > room.min_ft[1] + TOUCH
        };
        for s in t.segments.iter().filter(|s| !s.vertical && !s.below_floor && s.level == room.level) {
            let Some(it) = all.iter().find(|i| i.id == s.id) else { continue };
            if inside(it.min, it.max) && it.min[2] < clg - TOUCH && it.max[2] > floor {
                let lo = [it.min[0].max(room.min_ft[0]), it.min[1].max(room.min_ft[1]), it.min[2]];
                let hi = [it.max[0].min(room.max_ft[0]), it.max[1].min(room.max_ft[1]), clg];
                push(&mut out, "below_ceiling", &s.id, &room.id, lo, hi);
            }
        }
        for e in t.equipment.iter().filter(|e| e.level == room.level && e.min_ft[2] > floor + 0.5) {
            if inside(e.min_ft, e.max_ft) && e.min_ft[2] < clg - TOUCH {
                let lo = [e.min_ft[0].max(room.min_ft[0]), e.min_ft[1].max(room.min_ft[1]), e.min_ft[2]];
                let hi = [e.max_ft[0].min(room.max_ft[0]), e.max_ft[1].min(room.max_ft[1]), clg];
                push(&mut out, "below_ceiling", &e.id, &room.id, lo, hi);
            }
        }
    }
    for (k, c) in out.iter_mut().enumerate() {
        c.id = format!("X{:03}", k + 1);
    }
    out
}
