//! Boxes never share space (Q6252 fix-box-collisions; rules `box_clear`).
//!
//! - **Overlap**: a wall box whose body would overlap another box (two devices drawn at one
//!   spot that don't gang, a device at a corner over the next wall's box) moves along its own
//!   wall face, just clear of it.
//! - **Back to back**: two boxes on opposite faces of one fire-rated wall closer than the code's
//!   horizontal separation (one stud cavity): one moves along its face until they are that far
//!   apart; where neither can (the wall is too short, the spot is taken), both take the listed
//!   alternative (putty pads).
//!
//! The field's move, nothing else: the smallest move that clears every box around it and keeps
//! the box on its wall's face; the design spot stays beside the corrected one (the caller writes
//! the correction). General: sizes and distances are rules data, walls and boxes the model's.

use std::collections::HashMap;

use cadaddy_doc::Handle;
use cadaddy_geom::{Vec2, Vec3};
use serde::Deserialize;

/// The rules data (`box_clear` in mepatoms.json).
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub(crate) struct ClearRule {
    /// Least clear space between two boxes, inches.
    pub gap_in: f64,
    /// Farthest a box moves along its wall, inches.
    pub max_move_in: f64,
    /// The step a move is tried in, inches.
    pub step_in: f64,
    pub source: String,
    pub back_to_back: BackToBack,
}

/// Boxes on opposite faces of one wall.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub(crate) struct BackToBack {
    /// Least horizontal distance between them, inches, edge to edge (0: no rule).
    pub min_apart_in: f64,
    /// Only in a wall whose type carries a fire rating.
    pub rated_only: bool,
    pub source: String,
    /// What protects both boxes where neither can move far enough.
    pub putty_item: String,
    pub putty_source: String,
}

/// A box as the clearance sees it (drawing units): its centre, its face's direction along and
/// out (toward the room), its width along the face, depth across it and height, its wall.
#[derive(Clone, Debug)]
pub(crate) struct ClearBox {
    pub level: String,
    pub centre: Vec3,
    pub along: Vec2,
    pub out: Vec2,
    pub w: f64,
    pub t: f64,
    pub h: f64,
    pub wall: Option<Handle>,
    /// A wall box: it may move along its face. A ceiling or floor box stays where it is.
    pub movable: bool,
    /// Its wall takes the back-to-back rule (a fire-rated wall, or any when the rule says so).
    pub rated: bool,
}

/// Why a box moved.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Why {
    Overlap,
    BackToBack,
}

/// A box moved: which, by how much (plan, drawing units), why, and the box it cleared.
#[derive(Clone, Debug)]
pub(crate) struct Moved {
    pub index: usize,
    pub shift: Vec2,
    pub why: Why,
    pub from: usize,
}

/// What [`clear`] did.
#[derive(Debug, Default)]
pub(crate) struct Cleared {
    /// Each move made, in order (a box may move more than once).
    pub moved: Vec<Moved>,
    /// Back-to-back pairs no move clears: both take the rules' putty pads.
    pub putty: Vec<(usize, usize)>,
    /// Overlapping pairs no move clears (neither is a wall box that can move): a review.
    pub stuck: Vec<(usize, usize)>,
}

/// Most boxes looked at (hostile models).
const MAX_BOXES: usize = 200_000;

/// Whether `a` at `ca` and `b` at `cb` overlap, or come closer than `gap`: one level, their
/// heights overlapping, and no plan axis of either separating them by `gap`.
fn overlaps(a: &ClearBox, ca: Vec3, b: &ClearBox, cb: Vec3, gap: f64) -> bool {
    if !a.level.eq_ignore_ascii_case(&b.level) || (ca.z - cb.z).abs() >= (a.h + b.h) / 2.0 + gap - 1e-9 {
        return false;
    }
    let d = Vec2::new(cb.x - ca.x, cb.y - ca.y);
    let reach = |x: &ClearBox, u: Vec2| x.along.dot(u).abs() * x.w / 2.0 + x.out.dot(u).abs() * x.t / 2.0;
    ![a.along, a.out, b.along, b.out].iter().any(|u| d.dot(*u).abs() >= reach(a, *u) + reach(b, *u) + gap - 1e-9)
}

/// How far apart along the wall `a` at `ca` and `b` at `cb` are, edge to edge, when they are
/// on opposite faces of one wall the back-to-back rule applies to.
fn apart_back_to_back(a: &ClearBox, ca: Vec3, b: &ClearBox, cb: Vec3) -> Option<f64> {
    if !(a.movable && b.movable && a.rated && a.wall.is_some() && a.wall == b.wall && a.out.dot(b.out) < -0.9) {
        return None;
    }
    if !a.level.eq_ignore_ascii_case(&b.level) {
        return None;
    }
    let d = Vec2::new(cb.x - ca.x, cb.y - ca.y);
    Some(d.dot(a.along).abs() - (a.w + b.w) / 2.0)
}

/// The two boxes' conflict, if any.
fn conflict(a: &ClearBox, ca: Vec3, b: &ClearBox, cb: Vec3, gap: f64, apart: f64) -> Option<Why> {
    if overlaps(a, ca, b, cb, gap) {
        return Some(Why::Overlap);
    }
    (apart > 0.0 && apart_back_to_back(a, ca, b, cb).is_some_and(|e| e < apart - 1e-9)).then_some(Why::BackToBack)
}

/// The boxes near a point, by level and grid cell.
struct Grid {
    cell: f64,
    levels: HashMap<String, usize>,
    cells: HashMap<(usize, i64, i64), Vec<usize>>,
}

impl Grid {
    fn key(&self, level: &str, p: Vec3) -> Option<(usize, i64, i64)> {
        if !(p.x.is_finite() && p.y.is_finite()) {
            return None;
        }
        let l = *self.levels.get(&level.to_ascii_lowercase())?;
        // Saturating float-to-int casts of finite numbers.
        Some((l, (p.x / self.cell).floor() as i64, (p.y / self.cell).floor() as i64))
    }
    fn put(&mut self, level: &str, p: Vec3, i: usize) {
        if let Some(k) = self.key(level, p) {
            self.cells.entry(k).or_default().push(i);
        }
    }
    fn take(&mut self, level: &str, p: Vec3, i: usize) {
        if let Some(k) = self.key(level, p)
            && let Some(v) = self.cells.get_mut(&k)
        {
            v.retain(|x| *x != i);
        }
    }
    fn near(&self, level: &str, p: Vec3) -> Vec<usize> {
        let Some((l, cx, cy)) = self.key(level, p) else { return Vec::new() };
        let mut out = Vec::new();
        for dx in -1..=1_i64 {
            for dy in -1..=1_i64 {
                if let Some(v) = self.cells.get(&(l, cx.saturating_add(dx), cy.saturating_add(dy))) {
                    out.extend_from_slice(v);
                }
            }
        }
        out
    }
}

/// Clear the boxes (`upi`: drawing units per inch): every overlap and every back-to-back pair
/// in a rated wall, in order, each by the smallest move of one of its two boxes (the later one
/// first) along its face, in steps, to a spot clear of every box near it where `fits` says the
/// box still stands on its wall's face. Back-to-back pairs no move clears take putty pads.
pub(crate) fn clear(boxes: &[ClearBox], rule: &ClearRule, upi: f64, fits: &dyn Fn(usize, Vec3) -> bool) -> Cleared {
    let mut out = Cleared::default();
    let n = boxes.len().min(MAX_BOXES);
    if n < 2 || !(upi.is_finite() && upi > 0.0) {
        return out;
    }
    let gap = rule.gap_in.max(0.0) * upi;
    let apart = rule.back_to_back.min_apart_in.max(0.0) * upi;
    let step = rule.step_in.max(0.125) * upi;
    let steps = ((rule.max_move_in.max(0.0) * upi / step).floor() as usize).min(1_000);
    let widest = boxes.iter().take(n).map(|b| b.w.max(b.t)).filter(|v| v.is_finite()).fold(0.0, f64::max).min(1e6 * upi);
    // A cell holds what a box can conflict with from anywhere in it.
    let cell = (apart + 2.0 * widest + gap).max(upi);
    let mut grid = Grid { cell, levels: HashMap::new(), cells: HashMap::new() };
    let mut at: Vec<Vec3> = boxes.iter().take(n).map(|b| b.centre).collect();
    let ok: Vec<bool> = boxes
        .iter()
        .take(n)
        .map(|b| [b.centre.x, b.centre.y, b.centre.z, b.w, b.t, b.h, b.along.x, b.along.y, b.out.x, b.out.y].iter().all(|v| v.is_finite()))
        .collect();
    for (i, b) in boxes.iter().take(n).enumerate() {
        if !ok.get(i).copied().unwrap_or(false) {
            continue;
        }
        let k = grid.levels.len();
        grid.levels.entry(b.level.to_ascii_lowercase()).or_insert(k);
        grid.put(&b.level, b.centre, i);
    }
    // Whether box `x` at `p` conflicts with no other box.
    let free = |grid: &Grid, at: &[Vec3], x: usize, p: Vec3| -> bool {
        let Some(bx) = boxes.get(x) else { return false };
        grid.near(&bx.level, p).into_iter().filter(|y| *y != x).all(|y| match (boxes.get(y), at.get(y)) {
            (Some(by), Some(cy)) => conflict(bx, p, by, *cy, gap, apart).is_none(),
            _ => true,
        })
    };
    for i in 0..n {
        let Some(a) = boxes.get(i).filter(|_| ok.get(i).copied().unwrap_or(false)) else { continue };
        let mut others = grid.near(&a.level, at.get(i).copied().unwrap_or(a.centre));
        others.retain(|j| *j > i);
        others.sort_unstable();
        for j in others {
            let (Some(b), Some(&ci), Some(&cj)) = (boxes.get(j), at.get(i), at.get(j)) else { continue };
            let Some(why) = conflict(a, ci, b, cj, gap, apart) else { continue };
            // The smallest move of either (the later one first, then forward), clear of all.
            let mut best: Option<(usize, usize, f64)> = None;
            'search: for k in 1..=steps {
                let s = step * k as f64;
                for x in [j, i] {
                    let (Some(bx), Some(&cx)) = (boxes.get(x), at.get(x)) else { continue };
                    if !bx.movable {
                        continue;
                    }
                    for dir in [1.0, -1.0] {
                        let p = cx + Vec3::new(bx.along.x, bx.along.y, 0.0) * (dir * s);
                        if free(&grid, &at, x, p) && fits(x, p) {
                            best = Some((x, k, dir));
                            break 'search;
                        }
                    }
                }
            }
            match best {
                Some((x, k, dir)) => {
                    let (Some(bx), Some(cx)) = (boxes.get(x), at.get(x).copied()) else { continue };
                    let shift = bx.along * (dir * step * k as f64);
                    let p = cx + Vec3::new(shift.x, shift.y, 0.0);
                    grid.take(&bx.level, cx, x);
                    grid.put(&bx.level, p, x);
                    if let Some(c) = at.get_mut(x) {
                        *c = p;
                    }
                    out.moved.push(Moved { index: x, shift, why, from: if x == j { i } else { j } });
                }
                None if why == Why::BackToBack => out.putty.push((i, j)),
                None => out.stuck.push((i, j)),
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rule() -> ClearRule {
        ClearRule {
            gap_in: 0.25,
            max_move_in: 48.0,
            step_in: 0.5,
            back_to_back: BackToBack { min_apart_in: 24.0, rated_only: true, ..BackToBack::default() },
            ..ClearRule::default()
        }
    }

    /// A 4 in square box 2-1/8 deep in a wall face (inches as drawing units).
    fn wall_box(x: f64, y: f64, z: f64, out: Vec2, wall: u64, rated: bool) -> ClearBox {
        ClearBox {
            level: "Level 1".into(),
            centre: Vec3::new(x, y, z),
            along: out.perp(),
            out,
            w: 4.0,
            t: 2.125,
            h: 4.0,
            wall: Some(Handle(wall)),
            movable: true,
            rated,
        }
    }

    fn moved_total(c: &Cleared, i: usize) -> f64 {
        c.moved.iter().filter(|m| m.index == i).fold(Vec2::new(0.0, 0.0), |s, m| s + m.shift).len()
    }

    #[test]
    fn coincident_boxes_part_by_the_smallest_move_along_the_wall() {
        let n = Vec2::new(0.0, 1.0);
        let boxes = vec![wall_box(10.0, 0.0, 44.0, n, 1, false), wall_box(10.0, 0.0, 44.0, n, 1, false)];
        let c = clear(&boxes, &rule(), 1.0, &|_, _| true);
        assert_eq!(c.moved.len(), 1);
        let m = &c.moved[0];
        // The later box moves: one width and the gap, along the face, never across it.
        assert_eq!((m.index, m.from, m.why), (1, 0, Why::Overlap));
        assert!((m.shift.len() - 4.5).abs() < 1e-9, "{:?}", m.shift);
        assert!(m.shift.y.abs() < 1e-9);
        assert!(c.putty.is_empty() && c.stuck.is_empty());
    }

    #[test]
    fn a_box_at_a_corner_moves_off_the_next_walls_box_and_stays_on_its_face() {
        // A switch on the north wall's face and a receptacle on the east wall's, at one height,
        // their corners 0.1 in into each other.
        let sw = wall_box(98.0, 100.0 - 1.0625, 46.0, Vec2::new(0.0, -1.0), 1, false);
        let rc = wall_box(100.0 - 1.0625, 101.9 - 4.0, 46.0, Vec2::new(-1.0, 0.0), 2, false);
        let boxes = vec![sw, rc];
        assert!(overlaps(&boxes[0], boxes[0].centre, &boxes[1], boxes[1].centre, 0.25));
        // The receptacle can't move toward the corner (off its wall's face past it).
        let fits = |i: usize, p: Vec3| i != 1 || p.y <= 98.0;
        let c = clear(&boxes, &rule(), 1.0, &fits);
        assert_eq!(c.moved.len(), 1);
        let m = &c.moved[0];
        assert_eq!(m.index, 1);
        assert!(m.shift.y < 0.0 && m.shift.x.abs() < 1e-9);
        let p = boxes[1].centre + Vec3::new(m.shift.x, m.shift.y, 0.0);
        assert!(!overlaps(&boxes[0], boxes[0].centre, &boxes[1], p, 0.25));
    }

    #[test]
    fn back_to_back_boxes_in_a_rated_wall_end_up_the_codes_distance_apart() {
        // Two units' receptacles on the two faces of one rated wall, 3 in apart along it.
        let a = wall_box(50.0, 2.0, 18.0, Vec2::new(0.0, 1.0), 7, true);
        let b = wall_box(53.0, -2.0, 18.0, Vec2::new(0.0, -1.0), 7, true);
        let c = clear(&[a.clone(), b.clone()], &rule(), 1.0, &|_, _| true);
        assert_eq!(c.moved.len(), 1, "{c:?}");
        let m = &c.moved[0];
        assert_eq!((m.index, m.why), (1, Why::BackToBack));
        let p = b.centre + Vec3::new(m.shift.x, m.shift.y, 0.0);
        let edge = apart_back_to_back(&a, a.centre, &b, p).unwrap_or(0.0);
        assert!((24.0 - 1e-9..24.5 + 1e-9).contains(&edge), "{edge}");
        // Away from the other: 3 in apart moving on is the shorter way.
        assert!(m.shift.x > 0.0);
        // Not rated: they stay. Two walls side by side (two handles): not one cavity.
        let (mut a2, mut b2) = (a.clone(), b.clone());
        a2.rated = false;
        b2.rated = false;
        assert!(clear(&[a2, b2], &rule(), 1.0, &|_, _| true).moved.is_empty());
        let mut b3 = b.clone();
        b3.wall = Some(Handle(8));
        assert!(clear(&[a.clone(), b3], &rule(), 1.0, &|_, _| true).moved.is_empty());
        // Both facing one way (side by side on one face, 3 in apart, not overlapping): no rule.
        let b4 = wall_box(60.0, 2.0, 18.0, Vec2::new(0.0, 1.0), 7, true);
        assert!(clear(&[a, b4], &rule(), 1.0, &|_, _| true).moved.is_empty());
    }

    #[test]
    fn a_back_to_back_pair_with_nowhere_to_go_takes_putty_pads_and_an_overlap_of_fixed_boxes_is_a_review() {
        let a = wall_box(50.0, 2.0, 18.0, Vec2::new(0.0, 1.0), 7, true);
        let b = wall_box(52.0, -2.0, 18.0, Vec2::new(0.0, -1.0), 7, true);
        // A wall 12 in long: no spot 24 in from the other.
        let fits = |_: usize, p: Vec3| (44.0..=56.0).contains(&p.x);
        let c = clear(&[a, b], &rule(), 1.0, &fits);
        assert!(c.moved.is_empty());
        assert_eq!(c.putty, vec![(0, 1)]);
        // Two ceiling boxes over each other: neither moves.
        let mut l1 = wall_box(0.0, 0.0, 100.0, Vec2::new(0.0, 1.0), 1, false);
        l1.movable = false;
        l1.wall = None;
        let mut l2 = l1.clone();
        l2.centre.x = 3.0;
        let c = clear(&[l1, l2], &rule(), 1.0, &|_, _| true);
        assert!(c.moved.is_empty());
        assert_eq!(c.stuck, vec![(0, 1)]);
    }

    #[test]
    fn a_move_never_lands_on_a_third_box_and_hostile_input_is_harmless() {
        // Three boxes in a row on one face: the coincident pair's mover skips the taken spot.
        let n = Vec2::new(0.0, 1.0);
        let boxes = vec![wall_box(10.0, 0.0, 44.0, n, 1, false), wall_box(10.0, 0.0, 44.0, n, 1, false), wall_box(5.5, 0.0, 44.0, n, 1, false)];
        // Moving box 1 along +x is perp of +y = (-1, 0): toward box 2. It must go past it or the other way.
        let c = clear(&boxes, &rule(), 1.0, &|_, _| true);
        let mut at: Vec<Vec3> = boxes.iter().map(|b| b.centre).collect();
        for m in &c.moved {
            at[m.index] = at[m.index] + Vec3::new(m.shift.x, m.shift.y, 0.0);
        }
        for i in 0..3 {
            for j in i + 1..3 {
                assert!(!overlaps(&boxes[i], at[i], &boxes[j], at[j], 0.25), "{i} {j} {at:?}");
            }
        }
        assert!(moved_total(&c, 1) > 0.0);
        let mut bad = wall_box(f64::NAN, 0.0, 0.0, n, 1, false);
        bad.w = f64::INFINITY;
        let c = clear(&[bad.clone(), bad], &rule(), 1.0, &|_, _| true);
        assert!(c.moved.is_empty());
        assert!(clear(&boxes, &rule(), f64::NAN, &|_, _| true).moved.is_empty());
    }
}
