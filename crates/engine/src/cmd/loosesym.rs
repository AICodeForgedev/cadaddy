//! Symbols drawn as loose strokes (Q6167): a device with no mark beside it (a fire alarm plan's
//! strobe, horn/strobe) imports as its strokes, not as a symbol block, so it is no device until
//! something says what it is. Its legend says so: the row's drawn sample has the same shape as
//! each copy on the plans. The plans' short strokes are grouped where they touch, and each group
//! whose shape (turned a quarter at a time, or mirrored) is a legend row's is that row's symbol.

use std::collections::BTreeMap;

use cadaddy_doc::{Drawing, EntityKind, Handle};
use cadaddy_geom::{Mat3, Vec2};

use super::guided::{Shape, shape_of};

/// Strokes looked at on one sheet, symbols found on it, at most.
const MAX_STROKES: usize = 200_000;
const MAX_FOUND: usize = 5_000;
/// Strokes in one symbol, at most (a legend's sample is a few lines and circles).
const MAX_PARTS: usize = 64;

/// A legend row's symbol found on a plan.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Found {
    /// Its middle (drawing units).
    pub at: Vec2,
    /// The row's words and the sheet of the legend.
    pub row: String,
    pub legend_sheet: String,
    pub handles: Vec<Handle>,
}

/// The extent of a stroke.
fn extent(k: &EntityKind) -> Option<[f64; 4]> {
    let (lo, hi) = match k {
        EntityKind::Line(l) => ([l.a.x.min(l.b.x), l.a.y.min(l.b.y)], [l.a.x.max(l.b.x), l.a.y.max(l.b.y)]),
        EntityKind::Circle(c) => ([c.center.x - c.radius, c.center.y - c.radius], [c.center.x + c.radius, c.center.y + c.radius]),
        EntityKind::Arc(a) => ([a.center.x - a.radius, a.center.y - a.radius], [a.center.x + a.radius, a.center.y + a.radius]),
        EntityKind::LwPolyline(p) if !p.vertices.is_empty() => {
            p.vertices.iter().take(MAX_PARTS).fold(([f64::INFINITY; 2], [f64::NEG_INFINITY; 2]), |(lo, hi), v| {
                ([lo[0].min(v.p.x), lo[1].min(v.p.y)], [hi[0].max(v.p.x), hi[1].max(v.p.y)])
            })
        }
        _ => return None,
    };
    let b = [lo[0], lo[1], hi[0], hi[1]];
    b.iter().all(|v| v.is_finite()).then_some(b)
}

fn find_root(parent: &mut [usize], mut i: usize) -> usize {
    while let Some(&p) = parent.get(i) {
        if p == i {
            break;
        }
        let gp = parent.get(p).copied().unwrap_or(p);
        if let Some(x) = parent.get_mut(i) {
            *x = gp;
        }
        i = p;
    }
    i
}

/// The legend rows' symbols among the loose strokes of the objects with handles in `lo..hi` (one
/// sheet), each no bigger than `max_size` across, outside `exclude` (the legend tables
/// themselves), for the rows `wanted` takes.
pub(crate) fn find(
    d: &Drawing,
    lo: u64,
    hi: u64,
    legend: &BTreeMap<Shape, (String, String)>,
    wanted: &dyn Fn(&str) -> bool,
    max_size: f64,
    exclude: &[[f64; 4]],
) -> Vec<Found> {
    let mut out = Vec::new();
    if legend.is_empty() || !(max_size.is_finite() && max_size > 0.0) {
        return out;
    }
    // The sheet's short strokes.
    let strokes: Vec<(Handle, &EntityKind, [f64; 4])> = d
        .model
        .iter()
        .filter(|e| e.handle.0 >= lo && e.handle.0 < hi)
        .filter_map(|e| extent(&e.kind).map(|b| (e.handle, &e.kind, b)))
        .filter(|(_, _, b)| b[2] - b[0] <= max_size && b[3] - b[1] <= max_size)
        .take(MAX_STROKES)
        .collect();
    // Those that touch, grouped (a grid a symbol wide).
    let tol = 0.02 * max_size;
    let cell = |v: f64| (v / max_size).floor() as i64;
    let mut grid: BTreeMap<(i64, i64), Vec<usize>> = BTreeMap::new();
    for (i, (_, _, b)) in strokes.iter().enumerate() {
        grid.entry((cell(b[0]), cell(b[1]))).or_default().push(i);
    }
    let mut parent: Vec<usize> = (0..strokes.len()).collect();
    for (i, (_, _, b)) in strokes.iter().enumerate() {
        let (cx, cy) = (cell(b[0]), cell(b[1]));
        for gx in cx - 1..=cx + 1 {
            for gy in cy - 1..=cy + 1 {
                for &j in grid.get(&(gx, gy)).into_iter().flatten().filter(|&&j| j > i) {
                    let Some((_, _, c)) = strokes.get(j) else { continue };
                    let touch = b[0] <= c[2] + tol && c[0] <= b[2] + tol && b[1] <= c[3] + tol && c[1] <= b[3] + tol;
                    if touch {
                        let (ri, rj) = (find_root(&mut parent, i), find_root(&mut parent, j));
                        if ri != rj
                            && let Some(p) = parent.get_mut(ri)
                        {
                            *p = rj;
                        }
                    }
                }
            }
        }
    }
    let mut groups: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
    for i in 0..strokes.len() {
        let r = find_root(&mut parent, i);
        groups.entry(r).or_default().push(i);
    }
    for members in groups.values() {
        if out.len() >= MAX_FOUND {
            break;
        }
        if members.len() < 2 || members.len() > MAX_PARTS {
            continue;
        }
        let b = members
            .iter()
            .filter_map(|&i| strokes.get(i).map(|s| s.2))
            .fold([f64::INFINITY, f64::INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY], |a, b| {
                [a[0].min(b[0]), a[1].min(b[1]), a[2].max(b[2]), a[3].max(b[3])]
            });
        if b[2] - b[0] > max_size || b[3] - b[1] > max_size {
            continue;
        }
        let mid = Vec2::new((b[0] + b[2]) / 2.0, (b[1] + b[3]) / 2.0);
        if exclude.iter().any(|x| mid.x >= x[0] && mid.x <= x[2] && mid.y >= x[1] && mid.y <= x[3]) {
            continue;
        }
        let kinds: Vec<&EntityKind> = members.iter().filter_map(|&i| strokes.get(i).map(|s| s.1)).collect();
        // As drawn, a quarter turn at a time, and mirrored.
        let mut hit: Option<&(String, String)> = None;
        'turns: for mirrored in [false, true] {
            for q in 0..4 {
                let m = Mat3::rotate_about(mid, f64::from(q) * std::f64::consts::FRAC_PI_2);
                let m = if mirrored { Mat3::mirror(mid, mid + Vec2::new(0.0, 1.0)).then(m) } else { m };
                let turned: Vec<EntityKind> = kinds
                    .iter()
                    .map(|k| {
                        let mut k = (*k).clone();
                        k.transform(&m);
                        k
                    })
                    .collect();
                if let Some(row) = shape_of(turned.iter()).and_then(|s| legend.get(&s)).filter(|(w, _)| wanted(w)) {
                    hit = Some(row);
                    break 'turns;
                }
            }
        }
        if let Some((row, sheet)) = hit {
            out.push(Found {
                at: mid,
                row: row.clone(),
                legend_sheet: sheet.clone(),
                handles: members.iter().filter_map(|&i| strokes.get(i).map(|s| s.0)).collect(),
            });
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use cadaddy_doc::{Circle, Entity, Line};
    use cadaddy_geom::Vec3;

    fn push(d: &mut Drawing, k: EntityKind) -> Handle {
        let h = Handle(d.handseed.max(0x100));
        d.handseed = h.0 + 1;
        d.model.push(Entity::new(h, k));
        h
    }

    /// A made-up strobe: a circle with an X in it and a stem to its left, at `c`, size `s`, turned.
    fn strobe(d: &mut Drawing, c: Vec2, s: f64, turn: f64) {
        let p = |x: f64, y: f64| {
            let v = Vec2::new(x, y).rotate(turn) + c;
            Vec3::new(v.x, v.y, 0.0)
        };
        push(d, EntityKind::Circle(Circle { center: Vec3::new(c.x, c.y, 0.0), radius: 0.3 * s }));
        push(d, EntityKind::Line(Line { a: p(-0.2 * s, -0.2 * s), b: p(0.2 * s, 0.2 * s) }));
        push(d, EntityKind::Line(Line { a: p(-0.2 * s, 0.2 * s), b: p(0.2 * s, -0.2 * s) }));
        push(d, EntityKind::Line(Line { a: p(-0.5 * s, -0.25 * s), b: p(-0.5 * s, 0.25 * s) }));
        push(d, EntityKind::Line(Line { a: p(-0.5 * s, 0.0), b: p(-0.3 * s, 0.0) }));
    }

    #[test]
    fn a_legend_rows_drawn_sample_finds_its_loose_copies_turned_or_not() {
        let mut d = Drawing::new_imperial();
        // The legend's sample (its shape is the row's), then the plan's copies: as drawn, a
        // quarter turn (on a wall the other way), and one by a long wall line it touches.
        let lo = d.handseed.max(0x100);
        strobe(&mut d, Vec2::new(500.0, 500.0), 6.0, 0.0);
        let sample: Vec<EntityKind> = d.model.iter().map(|e| e.kind.clone()).collect();
        let mut legend = BTreeMap::new();
        legend.insert(shape_of(sample.iter()).unwrap(), ("WALL STROBE".to_string(), "E-313".to_string()));
        strobe(&mut d, Vec2::new(10.0, 10.0), 6.0, 0.0);
        strobe(&mut d, Vec2::new(60.0, 10.0), 6.0, std::f64::consts::FRAC_PI_2);
        strobe(&mut d, Vec2::new(110.0, 10.0), 6.0, 0.0);
        push(&mut d, EntityKind::Line(Line { a: Vec3::new(107.0, -40.0, 0.0), b: Vec3::new(107.0, 60.0, 0.0) }));
        // Something else of the same size: no row's.
        push(&mut d, EntityKind::Circle(Circle { center: Vec3::new(200.0, 10.0, 0.0), radius: 2.0 }));
        push(&mut d, EntityKind::Line(Line { a: Vec3::new(198.0, 10.0, 0.0), b: Vec3::new(202.0, 10.0, 0.0) }));
        let hi = d.handseed;
        let found = find(&d, lo, hi, &legend, &|w| w.contains("STROBE"), 12.0, &[[490.0, 490.0, 510.0, 510.0]]);
        let at: Vec<(i64, i64)> = found.iter().map(|f| (f.at.x.round() as i64, f.at.y.round() as i64)).collect();
        assert_eq!(at.len(), 3, "the three copies, not the sample in the legend: {found:?}");
        assert!(at.contains(&(9, 10)) && at.iter().any(|a| a.0 == 60) && at.iter().any(|a| a.0 == 109), "{at:?}");
        assert!(found.iter().all(|f| f.row == "WALL STROBE" && f.legend_sheet == "E-313" && f.handles.len() == 5), "{found:?}");
        // Rows not wanted: none.
        assert!(find(&d, lo, hi, &legend, &|w| w.contains("SMOKE"), 12.0, &[]).is_empty());
        // Hostile sizes and empty legends never panic.
        for s in [0.0, -1.0, f64::NAN, f64::INFINITY] {
            let _ = find(&d, lo, hi, &legend, &|_| true, s, &[]);
        }
        assert!(find(&d, lo, hi, &BTreeMap::new(), &|_| true, 12.0, &[]).is_empty());
    }
}
