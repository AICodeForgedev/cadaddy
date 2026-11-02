//! Named wall faces (Q6174): each built wall's faces as the rooms they face and the compass side,
//! `<room>/<N|S|E|W>` (a demising wall is `121/S` on one face and `123/N` on the other).
//!
//! A room's outline is walked counter-clockwise; each edge the built walls stand along (their
//! pieces parallel to it, within their thickness of it) is a face: the wall's face toward the
//! room. Its side is the compass side its outward direction points to, from the plan's north (+Y
//! unless the caller turns it). A second or skewed face on the same side gets a number (`121/S2`):
//! the square faces first, the longest first. Each face knows its walls and their type, its
//! length and height, its openings (gaps in the full-height wall: a door under a head, a window
//! over a sill, an open bay) and the faces at its two ends, in corner order round the room.

use cadaddy_doc::{Drawing, EntityKind, Handle};
use cadaddy_geom::Vec2;

/// Pieces and faces looked at, at most.
const MAX_PIECES: usize = 500_000;
const MAX_FACES: usize = 200_000;
/// Corners of one room outline looked at, at most.
const MAX_CORNERS: usize = 4_000;
/// A wall piece is along a room's edge when it is this parallel (sine of the angle).
const PARALLEL_SIN: f64 = 0.05;
/// How far past a wall's thickness its room's edge may be traced (inches).
const TRACE_TOL_IN: f64 = 4.0;
/// An edge is a face when walls cover this share of it and at least this much (inches).
const COVER_SHARE: f64 = 0.3;
const COVER_MIN_IN: f64 = 6.0;
/// A gap in the full-height wall this wide or more is an opening (inches).
const OPENING_MIN_IN: f64 = 12.0;
/// Heights this close are the same (inches): a piece on the floor, a piece to the top.
const LEVEL_TOL_IN: f64 = 2.0;
/// A sill this high or more is a window's (inches above the floor).
const SILL_MIN_IN: f64 = 6.0;
/// A face this far off its compass side is skewed (degrees).
const SKEW_DEG: f64 = 5.0;
/// Outline edges shorter than this are jogs, not faces (inches).
const JOG_IN: f64 = 6.0;
/// A face is at least this long: a shorter one is a wall's end (inches).
const FACE_MIN_IN: f64 = 8.0;
/// A wall inside a room: its side looks into the room this far past its face (inches).
const FACE_PROBE_IN: f64 = 6.0;
/// A wall's sides are looked along this often for the rooms they face (inches), at most this
/// many times.
const SAMPLE_IN: f64 = 6.0;
const MAX_SAMPLES: usize = 4_000;

/// One wall piece: a centreline, half its thickness and its bottom and top (model coordinates).
#[derive(Clone, Debug)]
pub(crate) struct Piece {
    pub wall: Handle,
    pub level: Option<String>,
    pub a: Vec2,
    pub b: Vec2,
    pub half: f64,
    pub z0: f64,
    pub z1: f64,
    pub wall_type: Option<String>,
}

/// The built walls' pieces, as centrelines in model coordinates (where the meshes are).
pub(crate) fn pieces(d: &Drawing, upf: f64) -> Vec<Piece> {
    let mut out = Vec::new();
    for e in d.model.iter().filter(|e| super::super::wallgen::made_as(e).as_deref() == Some("wall")) {
        let EntityKind::Mesh(m) = &e.kind else { continue };
        let level = super::super::bspec::made_level(e);
        let wall_type = super::super::bimlook::made_tag(e, "type").map(str::to_string).filter(|t| !t.trim().is_empty());
        for p in super::super::validate::wall_pieces(m) {
            let ft: Vec<[f64; 2]> = p.outline.iter().map(|q| [q.x / upf, q.y / upf]).collect();
            let Ok(lines) = super::super::validate::footprint_centrelines(&ft) else { continue };
            for (a, b, thick) in lines {
                out.push(Piece {
                    wall: e.handle,
                    level: level.clone(),
                    a: Vec2::new(a[0] * upf, a[1] * upf),
                    b: Vec2::new(b[0] * upf, b[1] * upf),
                    half: thick * upf / 2.0,
                    z0: p.z0,
                    z1: p.z1,
                    wall_type: wall_type.clone(),
                });
            }
        }
        if out.len() >= MAX_PIECES {
            break;
        }
    }
    out
}

/// A room as the faces see it: its label (number, else name), level and outline.
#[derive(Clone, Debug)]
pub(crate) struct RoomRef {
    pub label: String,
    pub level: Option<String>,
    pub outline: Vec<Vec2>,
}

/// An opening in a face: where along it (from its start, drawing units) and its bottom and top
/// (model z).
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Opening {
    pub kind: &'static str,
    pub from: f64,
    pub to: f64,
    pub bottom: f64,
    pub top: f64,
}

/// A named wall face.
#[derive(Clone, Debug)]
pub(crate) struct Face {
    pub name: String,
    pub room: String,
    pub side: &'static str,
    pub level: Option<String>,
    /// The wall whose face it mostly is, and every wall along it.
    pub wall: Handle,
    pub walls: Vec<Handle>,
    pub wall_type: Option<String>,
    /// The face's line, in corner order round the room (model coordinates), its direction and
    /// the direction into the room.
    pub a: Vec2,
    pub b: Vec2,
    pub u: Vec2,
    pub inward: Vec2,
    pub len: f64,
    /// The walls' bottom and top along it (model z) and half the main wall's thickness.
    pub floor: f64,
    pub top: f64,
    pub half: f64,
    pub openings: Vec<Opening>,
    /// The faces at its start and its end (`None`: open there, or no wall).
    pub prev: Option<String>,
    pub next: Option<String>,
    pub skew_deg: f64,
    /// Its place round the room (0 first).
    pub order: usize,
}

impl Face {
    /// How far along the face `p` is from its start.
    pub fn along(&self, p: Vec2) -> f64 {
        (p - self.a).x * self.u.x + (p - self.a).y * self.u.y
    }
    /// How far `p` is in front of the face (into the room; negative: inside the wall).
    pub fn depth(&self, p: Vec2) -> f64 {
        (p - self.a).x * self.inward.x + (p - self.a).y * self.inward.y
    }
    pub fn at(&self, t: f64) -> Vec2 {
        self.a + self.u * t
    }
}

/// The compass side a direction points to, from plan north (`north_deg` counter-clockwise from
/// +Y), and how far off that side it is (degrees).
pub(crate) fn compass(dir: Vec2, north_deg: f64) -> (&'static str, f64) {
    let th = north_deg.to_radians();
    let n = Vec2::new(-th.sin(), th.cos());
    let e = Vec2::new(th.cos(), th.sin());
    let phi = (dir.x * e.x + dir.y * e.y).atan2(dir.x * n.x + dir.y * n.y).to_degrees();
    let q = (phi / 90.0).round();
    let side = match (q as i64).rem_euclid(4) {
        0 => "N",
        1 => "E",
        2 => "S",
        _ => "W",
    };
    (side, (phi - q * 90.0).abs())
}

fn cross(a: Vec2, b: Vec2) -> f64 {
    a.x * b.y - a.y * b.x
}

fn dot(a: Vec2, b: Vec2) -> f64 {
    a.x * b.x + a.y * b.y
}

/// A room outline as faces walk it: finite corners, no repeats, straight runs merged,
/// counter-clockwise.
fn simplify(outline: &[Vec2], eps: f64) -> Vec<Vec2> {
    let mut pts: Vec<Vec2> = Vec::new();
    for p in outline.iter().take(MAX_CORNERS).copied().filter(|p| p.is_finite()) {
        if pts.last().is_none_or(|q| q.dist(p) > eps) {
            pts.push(p);
        }
    }
    while pts.len() > 1 && pts.first().zip(pts.last()).is_some_and(|(a, b)| a.dist(*b) <= eps) {
        pts.pop();
    }
    // Merge corners on a straight run (bounded: each pass drops a corner or stops).
    for _ in 0..MAX_CORNERS {
        let n = pts.len();
        if n < 4 {
            break;
        }
        let drop = (0..n).find(|&i| {
            let (Some(p), Some(c), Some(q)) = (pts.get((i + n - 1) % n), pts.get(i), pts.get((i + 1) % n)) else { return false };
            let (u, v) = (*c - *p, *q - *c);
            let (lu, lv) = (u.len(), v.len());
            lu > 0.0 && lv > 0.0 && (cross(u, v) / (lu * lv)).abs() < 0.03 && dot(u, v) > 0.0
        });
        match drop {
            Some(i) => {
                pts.remove(i);
            }
            None => break,
        }
    }
    let n = pts.len();
    let area: f64 = (0..n).filter_map(|i| Some(cross(*pts.get(i)?, *pts.get((i + 1) % n)?))).sum();
    if area < 0.0 {
        pts.reverse();
    }
    pts
}

/// Merge intervals and give their total length.
fn union(mut iv: Vec<(f64, f64)>) -> Vec<(f64, f64)> {
    iv.sort_by(|a, b| a.0.total_cmp(&b.0));
    let mut out: Vec<(f64, f64)> = Vec::new();
    for (a, b) in iv {
        match out.last_mut() {
            Some(l) if a <= l.1 => l.1 = l.1.max(b),
            _ => out.push((a, b)),
        }
    }
    out
}

/// The face of the walls standing along the edge `p0`→`p1` (the room on its left), when they
/// cover enough of it: their pieces parallel to it, their centreline within their thickness of
/// it (outside it, or traced at the centreline), overlapping it. `only`: just that wall's pieces.
#[allow(clippy::too_many_arguments)]
fn edge_face(
    p0: Vec2,
    p1: Vec2,
    list: &[(&Piece, [f64; 4])],
    inch: f64,
    tol: f64,
    lvl_tol: f64,
    north_deg: f64,
    only: Option<Handle>,
) -> Option<Face> {
    let len = p0.dist(p1);
    if !(len.is_finite() && len > 0.0) {
        return None;
    }
    let u = (p1 - p0) * (1.0 / len);
    let n_out = Vec2::new(u.y, -u.x);
    let (lo, hi) = (Vec2::new(p0.x.min(p1.x), p0.y.min(p1.y)), Vec2::new(p0.x.max(p1.x), p0.y.max(p1.y)));
    let mut along: Vec<(&Piece, f64, f64, f64)> = Vec::new();
    for (pc, bx) in list {
        if only.is_some_and(|w| w != pc.wall) || bx[2] < lo.x || bx[0] > hi.x || bx[3] < lo.y || bx[1] > hi.y {
            continue;
        }
        let pl = pc.a.dist(pc.b);
        if !(pl.is_finite() && pl > 0.0) {
            continue;
        }
        let pu = (pc.b - pc.a) * (1.0 / pl);
        if cross(u, pu).abs() > PARALLEL_SIN {
            continue;
        }
        let off = dot((pc.a + pc.b) * 0.5 - p0, n_out);
        if off < -tol || off > pc.half + tol {
            continue;
        }
        let (s0, s1) = (dot(pc.a - p0, u), dot(pc.b - p0, u));
        let (s0, s1) = (s0.min(s1).max(0.0), s0.max(s1).min(len));
        if s1 - s0 > inch {
            along.push((pc, off, s0, s1));
        }
    }
    let covered: f64 = union(along.iter().map(|x| (x.2, x.3)).collect()).iter().map(|(a, b)| b - a).sum();
    if along.is_empty() || covered < COVER_SHARE * len || covered < COVER_MIN_IN * inch {
        return None;
    }
    let floor = along.iter().map(|x| x.0.z0).fold(f64::INFINITY, f64::min);
    let top = along.iter().map(|x| x.0.z1).fold(f64::NEG_INFINITY, f64::max);
    let full = |x: &&(&Piece, f64, f64, f64)| x.0.z0 <= floor + lvl_tol && x.0.z1 >= top - lvl_tol;
    // The main wall: the full-height piece covering most of it, else the longest.
    let main = along
        .iter()
        .filter(full)
        .max_by(|a, b| (a.3 - a.2).total_cmp(&(b.3 - b.2)))
        .or_else(|| along.iter().max_by(|a, b| (a.3 - a.2).total_cmp(&(b.3 - b.2))));
    let &(mp, moff, _, _) = main?;
    let face_off = moff - mp.half;
    let (a, b) = (p0 + n_out * face_off, p1 + n_out * face_off);
    // Openings: gaps in the full-height wall.
    let solid = union(along.iter().filter(full).map(|x| (x.2, x.3)).collect());
    let mut gaps: Vec<(f64, f64)> = Vec::new();
    let mut at = 0.0;
    for (s0, s1) in &solid {
        if *s0 - at >= OPENING_MIN_IN * inch {
            gaps.push((at, *s0));
        }
        at = at.max(*s1);
    }
    if len - at >= OPENING_MIN_IN * inch {
        gaps.push((at, len));
    }
    let openings = gaps
        .into_iter()
        .map(|(g0, g1)| {
            let over = |x: &&(&Piece, f64, f64, f64)| x.2 < g1 - inch && x.3 > g0 + inch;
            let head = along.iter().filter(over).filter(|x| x.0.z0 > floor + lvl_tol).map(|x| x.0.z0).fold(f64::INFINITY, f64::min);
            let sill = along
                .iter()
                .filter(over)
                .filter(|x| x.0.z0 <= floor + lvl_tol && x.0.z1 < top - lvl_tol)
                .map(|x| x.0.z1)
                .fold(f64::NEG_INFINITY, f64::max);
            let bottom = if sill.is_finite() { sill } else { floor };
            let head = if head.is_finite() { head } else { top };
            let kind = if bottom >= floor + SILL_MIN_IN * inch {
                "window"
            } else if head < top - lvl_tol {
                "door"
            } else {
                "opening"
            };
            Opening { kind, from: g0, to: g1, bottom, top: head }
        })
        .collect();
    let mut walls: Vec<Handle> = vec![mp.wall];
    for x in &along {
        if !walls.contains(&x.0.wall) {
            walls.push(x.0.wall);
        }
    }
    let (side, skew) = compass(n_out, north_deg);
    Some(Face {
        name: String::new(),
        room: String::new(),
        side,
        level: mp.level.clone(),
        wall: mp.wall,
        walls,
        wall_type: mp.wall_type.clone(),
        a,
        b,
        u,
        inward: n_out * -1.0,
        len,
        floor,
        top,
        half: mp.half,
        openings,
        prev: None,
        next: None,
        skew_deg: skew,
        order: 0,
    })
}

/// One edge's face before it is named.
struct Raw {
    room: usize,
    edge: usize,
    face: Face,
}

/// Every room's named faces.
pub(crate) fn faces(rooms: &[RoomRef], pieces: &[Piece], upf: f64, north_deg: f64) -> Vec<Face> {
    let inch = upf / 12.0;
    let tol = TRACE_TOL_IN * inch;
    let lvl_tol = LEVEL_TOL_IN * inch;
    // The pieces by level (lower case), with their boxes, for a quick look.
    let mut by_level: std::collections::HashMap<String, Vec<(&Piece, [f64; 4])>> = std::collections::HashMap::new();
    for p in pieces {
        let r = p.half + tol;
        let bx = [p.a.x.min(p.b.x) - r, p.a.y.min(p.b.y) - r, p.a.x.max(p.b.x) + r, p.a.y.max(p.b.y) + r];
        by_level.entry(p.level.clone().unwrap_or_default().to_ascii_lowercase()).or_default().push((p, bx));
    }
    // Labels that repeat on a level get a number (`CORRIDOR#2`).
    let mut seen: std::collections::HashMap<(String, String), usize> = std::collections::HashMap::new();
    let labels: Vec<String> = rooms
        .iter()
        .map(|r| {
            let base = if r.label.trim().is_empty() { "ROOM".to_string() } else { r.label.trim().to_string() };
            let k = seen.entry((r.level.clone().unwrap_or_default().to_ascii_lowercase(), base.to_ascii_uppercase())).or_default();
            *k += 1;
            if *k == 1 { base } else { format!("{base}#{k}") }
        })
        .collect();
    let mut raws: Vec<Raw> = Vec::new();
    let mut edges_of: Vec<Vec<(usize, f64, Option<usize>)>> = Vec::new();
    for (ri, r) in rooms.iter().enumerate() {
        let pts = simplify(&r.outline, inch * 0.1);
        let n = pts.len();
        let mut edges: Vec<(usize, f64, Option<usize>)> = Vec::new();
        if n < 3 {
            edges_of.push(edges);
            continue;
        }
        let empty = Vec::new();
        let list = by_level.get(&r.level.clone().unwrap_or_default().to_ascii_lowercase()).unwrap_or(&empty);
        for i in 0..n {
            let (Some(p0), Some(p1)) = (pts.get(i).copied(), pts.get((i + 1) % n).copied()) else { continue };
            let len = p0.dist(p1);
            if !(len.is_finite() && len > 0.0) {
                edges.push((i, 0.0, None));
                continue;
            }
            if len < FACE_MIN_IN * inch {
                edges.push((i, len, None));
                continue;
            }
            let Some(face) = edge_face(p0, p1, list, inch, tol, lvl_tol, north_deg, None) else {
                edges.push((i, len, None));
                continue;
            };
            edges.push((i, len, Some(raws.len())));
            raws.push(Raw { room: ri, edge: i, face: Face { room: labels.get(ri).cloned().unwrap_or_default(), level: r.level.clone(), ..face } });
            if raws.len() >= MAX_FACES {
                break;
            }
        }
        edges_of.push(edges);
        if raws.len() >= MAX_FACES {
            break;
        }
    }
    // Walls standing inside a room's outline (a stub, a pier, a partition the trace went round),
    // and walls the trace ran clear of: each stretch of a side that looks into a room and is no
    // face of it yet is that room's face, with open ends.
    let outlines: Vec<(usize, Vec<Vec2>, [f64; 4])> = rooms
        .iter()
        .enumerate()
        .filter(|(_, r)| r.outline.len() >= 3)
        .map(|(i, r)| {
            let b = r.outline.iter().fold([f64::INFINITY, f64::INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY], |b, p| {
                [b[0].min(p.x), b[1].min(p.y), b[2].max(p.x), b[3].max(p.y)]
            });
            (i, r.outline.clone(), b)
        })
        .collect();
    let room_at = |level: Option<&str>, p: Vec2| -> Option<usize> {
        outlines
            .iter()
            .filter(|(i, _, b)| {
                p.x >= b[0]
                    && p.x <= b[2]
                    && p.y >= b[1]
                    && p.y <= b[3]
                    && rooms.get(*i).is_some_and(|r| r.level.as_deref().zip(level).is_none_or(|(a, b)| a.eq_ignore_ascii_case(b)))
            })
            .find(|(_, o, _)| cadaddy_geom::point_in_polygon(o, p))
            .map(|(i, _, _)| *i)
    };
    // The outline faces by wall, for the stretches already taken.
    let mut of_wall: std::collections::HashMap<Handle, Vec<usize>> = std::collections::HashMap::new();
    for (k, r) in raws.iter().enumerate() {
        for w in &r.face.walls {
            of_wall.entry(*w).or_default().push(k);
        }
    }
    // Each wall's lines: its pieces merged by line and side.
    let mut lines: Vec<(Handle, Option<String>, Vec2, Vec2, f64)> = Vec::new();
    for p in pieces.iter().take(MAX_PIECES) {
        let l = p.a.dist(p.b);
        if !(l.is_finite() && l >= FACE_MIN_IN * inch) {
            continue;
        }
        let u = (p.b - p.a) * (1.0 / l);
        let merged = lines.iter_mut().find(|(w, _, a, b, _)| {
            let lu = (*b - *a) * (1.0 / a.dist(*b).max(1e-12));
            *w == p.wall && cross(lu, u).abs() <= PARALLEL_SIN && cross(lu, p.a - *a).abs() <= inch
        });
        match merged {
            Some((_, _, a, b, _)) => {
                let lu = (*b - *a) * (1.0 / a.dist(*b).max(1e-12));
                let ts = [0.0, dot(*b - *a, lu), dot(p.a - *a, lu), dot(p.b - *a, lu)];
                let (t0, t1) = ts.iter().fold((f64::INFINITY, f64::NEG_INFINITY), |(x, y), t| (x.min(*t), y.max(*t)));
                let o = *a;
                *a = o + lu * t0;
                *b = o + lu * t1;
            }
            None => lines.push((p.wall, p.level.clone(), p.a, p.b, p.half)),
        }
    }
    for (wall, level, la, lb, half) in lines {
        let l = la.dist(lb);
        if !(l.is_finite() && l > 0.0) {
            continue;
        }
        let u = (lb - la) * (1.0 / l);
        let n = Vec2::new(-u.y, u.x);
        let step = SAMPLE_IN * inch;
        let count = ((l / step).floor() as usize).clamp(1, MAX_SAMPLES);
        let empty = Vec::new();
        let list = by_level.get(&level.clone().unwrap_or_default().to_ascii_lowercase()).unwrap_or(&empty);
        for side in [1.0f64, -1.0] {
            let inward = n * side;
            // Along the wall, the room each stretch of this side looks into, where no face of that
            // room already runs.
            let mut runs: Vec<(usize, f64, f64)> = Vec::new();
            for k in 0..count {
                let t = (k as f64 + 0.5) * l / count as f64;
                let p = la + u * t + inward * (half + FACE_PROBE_IN * inch);
                let Some(ri) = room_at(level.as_deref(), p) else { continue };
                let taken = of_wall.get(&wall).into_iter().flatten().filter_map(|k| raws.get(*k)).any(|r| {
                    r.room == ri && dot(r.face.inward, inward) > 0.9 && {
                        let (t0, t1) = (dot(r.face.a - la, u), dot(r.face.b - la, u));
                        t >= t0.min(t1) - inch && t <= t0.max(t1) + inch
                    }
                });
                if taken {
                    continue;
                }
                let (s0, s1) = (k as f64 * l / count as f64, (k as f64 + 1.0) * l / count as f64);
                match runs.last_mut() {
                    Some(last) if last.0 == ri && (last.2 - s0).abs() <= 1e-9 => last.2 = s1,
                    _ => runs.push((ri, s0, s1)),
                }
            }
            for (ri, s0, s1) in runs {
                if s1 - s0 < FACE_MIN_IN * inch {
                    continue;
                }
                // The face line on the room's side, walked with the room on its left.
                let (fa, fb) = (la + u * s0 + inward * half, la + u * s1 + inward * half);
                let left = Vec2::new(-(fb - fa).y, (fb - fa).x);
                let (p0, p1) = if dot(left, inward) > 0.0 { (fa, fb) } else { (fb, fa) };
                let Some(face) = edge_face(p0, p1, list, inch, tol, lvl_tol, north_deg, Some(wall)) else { continue };
                let label = labels.get(ri).cloned().unwrap_or_default();
                raws.push(Raw { room: ri, edge: usize::MAX, face: Face { room: label, level: rooms.get(ri).and_then(|r| r.level.clone()), ..face } });
                if raws.len() >= MAX_FACES {
                    break;
                }
            }
        }
        if raws.len() >= MAX_FACES {
            break;
        }
    }
    // Names: per room and side, the square faces first, the longest first.
    let mut groups: std::collections::BTreeMap<(usize, &'static str), Vec<usize>> = std::collections::BTreeMap::new();
    for (k, r) in raws.iter().enumerate() {
        groups.entry((r.room, r.face.side)).or_default().push(k);
    }
    for ((_, side), mut ks) in groups {
        ks.sort_by(|&x, &y| {
            let (fx, fy) = (raws.get(x).map(|r| &r.face), raws.get(y).map(|r| &r.face));
            let key = |f: Option<&Face>| f.map_or((true, 0.0), |f| (f.skew_deg > SKEW_DEG, -f.len));
            let (kx, ky) = (key(fx), key(fy));
            kx.0.cmp(&ky.0).then(kx.1.total_cmp(&ky.1))
        });
        for (j, k) in ks.into_iter().enumerate() {
            if let Some(r) = raws.get_mut(k) {
                r.face.name = if j == 0 { format!("{}/{side}", r.face.room) } else { format!("{}/{side}{}", r.face.room, j + 1) };
            }
        }
    }
    // Neighbours round each room: the next face each way, past short jogs; a long edge with no
    // wall is open.
    let jog = JOG_IN.max(FACE_MIN_IN) * inch;
    let names: Vec<String> = raws.iter().map(|r| r.face.name.clone()).collect();
    for k in 0..raws.len() {
        let Some((room, edge)) = raws.get(k).map(|r| (r.room, r.edge)) else { continue };
        if edge == usize::MAX {
            // A wall inside the room: its ends are open.
            continue;
        }
        let Some(edges) = edges_of.get(room) else { continue };
        let n = edges.len();
        let pos = edges.iter().position(|e| e.0 == edge).unwrap_or(0);
        let look = |step: isize| -> Option<String> {
            for s in 1..n {
                let j = (pos as isize + step * s as isize).rem_euclid(n as isize) as usize;
                let (_, len, f) = edges.get(j)?;
                match f {
                    Some(fk) => return names.get(*fk).cloned(),
                    None if *len < jog => continue,
                    None => return None,
                }
            }
            None
        };
        let (prev, next) = (look(-1), look(1));
        if let Some(r) = raws.get_mut(k) {
            r.face.prev = prev;
            r.face.next = next;
            r.face.order = pos;
        }
    }
    raws.into_iter().map(|r| r.face).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn piece(wall: u64, a: (f64, f64), b: (f64, f64), half: f64, z: (f64, f64)) -> Piece {
        Piece {
            wall: Handle(wall),
            level: Some("L1".into()),
            a: Vec2::new(a.0, a.1),
            b: Vec2::new(b.0, b.1),
            half,
            z0: z.0,
            z1: z.1,
            wall_type: Some("P1".into()),
        }
    }

    fn room(label: &str, pts: &[(f64, f64)]) -> RoomRef {
        RoomRef { label: label.into(), level: Some("L1".into()), outline: pts.iter().map(|p| Vec2::new(p.0, p.1)).collect() }
    }

    #[test]
    fn compass_sides_from_plan_north() {
        assert_eq!(compass(Vec2::new(0.0, 1.0), 0.0).0, "N");
        assert_eq!(compass(Vec2::new(1.0, 0.0), 0.0).0, "E");
        assert_eq!(compass(Vec2::new(0.0, -1.0), 0.0).0, "S");
        assert_eq!(compass(Vec2::new(-1.0, 0.0), 0.0).0, "W");
        // Plan north turned 90° counter-clockwise (north is -X): +Y is east.
        assert_eq!(compass(Vec2::new(0.0, 1.0), 90.0).0, "E");
        let (s, skew) = compass(Vec2::new(1.0, -0.2), 0.0);
        assert_eq!(s, "E");
        assert!(skew > 10.0);
    }

    /// Two rooms 12 ft deep (inches), 101 west of a 6" demising wall at x=144..150, 103 east.
    #[test]
    fn four_named_faces_and_a_demising_walls_two_names() {
        let h = 3.0;
        let z = (0.0, 108.0);
        let ps = vec![
            piece(1, (-3.0, -3.0), (297.0, -3.0), h, z),   // south
            piece(2, (-3.0, 147.0), (297.0, 147.0), h, z), // north
            piece(3, (-3.0, -3.0), (-3.0, 147.0), h, z),   // west
            piece(4, (147.0, -3.0), (147.0, 147.0), h, z), // demising
            piece(5, (297.0, -3.0), (297.0, 147.0), h, z), // east
        ];
        let rooms = vec![
            room("101", &[(0.0, 0.0), (144.0, 0.0), (144.0, 144.0), (0.0, 144.0)]),
            room("103", &[(150.0, 0.0), (294.0, 0.0), (294.0, 144.0), (150.0, 144.0)]),
        ];
        let fs = faces(&rooms, &ps, 12.0, 0.0);
        let names: Vec<&str> = fs.iter().map(|f| f.name.as_str()).collect();
        for n in ["101/N", "101/S", "101/E", "101/W", "103/N", "103/S", "103/E", "103/W"] {
            assert!(names.contains(&n), "{n} in {names:?}");
        }
        let e101 = fs.iter().find(|f| f.name == "101/E").expect("101/E");
        let w103 = fs.iter().find(|f| f.name == "103/W").expect("103/W");
        assert_eq!(e101.wall, Handle(4));
        assert_eq!(w103.wall, Handle(4), "one wall, two faces");
        assert!((e101.a.x - 144.0).abs() < 1e-6 && (w103.a.x - 150.0).abs() < 1e-6, "{:?} {:?}", e101.a, w103.a);
        // Corner order (counter-clockwise): 101/S runs west→east, from 101/W to 101/E.
        let s101 = fs.iter().find(|f| f.name == "101/S").expect("101/S");
        assert_eq!(s101.prev.as_deref(), Some("101/W"));
        assert_eq!(s101.next.as_deref(), Some("101/E"));
        assert!((s101.len - 144.0).abs() < 1e-6 && (s101.top - s101.floor - 108.0).abs() < 1e-6);
        assert_eq!(s101.wall_type.as_deref(), Some("P1"));
    }

    #[test]
    fn a_wall_standing_inside_a_room_gets_the_rooms_faces_with_open_ends() {
        let h = 3.0;
        let z = (0.0, 108.0);
        let ps = vec![
            piece(1, (-3.0, -3.0), (147.0, -3.0), h, z),
            piece(2, (-3.0, 147.0), (147.0, 147.0), h, z),
            piece(3, (-3.0, -3.0), (-3.0, 147.0), h, z),
            piece(4, (147.0, -3.0), (147.0, 147.0), h, z),
            // A 4" stub 5 ft long standing in the room (its 4" ends are no faces).
            piece(9, (72.0, 40.0), (72.0, 100.0), 2.0, z),
        ];
        let fs = faces(&[room("12", &[(0.0, 0.0), (144.0, 0.0), (144.0, 144.0), (0.0, 144.0)])], &ps, 12.0, 0.0);
        let stub: Vec<&Face> = fs.iter().filter(|f| f.wall == Handle(9)).collect();
        assert_eq!(stub.len(), 2, "{:?}", fs.iter().map(|f| (&f.name, f.wall)).collect::<Vec<_>>());
        let names: Vec<&str> = stub.iter().map(|f| f.name.as_str()).collect();
        assert!(names.contains(&"12/E2") && names.contains(&"12/W2"), "{names:?}");
        assert!(stub.iter().all(|f| f.prev.is_none() && f.next.is_none() && (f.len - 60.0).abs() < 1e-6));
        let w2 = stub.iter().find(|f| f.name == "12/W2").expect("12/W2");
        assert!((w2.a.x - 74.0).abs() < 1e-6 && w2.inward.x > 0.9, "the stub's east face looks east into the room: {w2:?}");
    }

    #[test]
    fn a_skewed_face_on_the_same_side_gets_a_number_and_a_door_is_an_opening() {
        let h = 3.0;
        // A room whose south side has a square run and a skewed run.
        let pts = [(0.0, 0.0), (120.0, 0.0), (240.0, -24.0), (240.0, 144.0), (0.0, 144.0)];
        let mut ps = vec![
            piece(1, (0.0, -3.0), (120.0, -3.0), h, (0.0, 108.0)),
            piece(3, (243.0, -24.0), (243.0, 144.0), h, (0.0, 108.0)),
            piece(4, (0.0, 147.0), (240.0, 147.0), h, (0.0, 108.0)),
            piece(5, (-3.0, 0.0), (-3.0, 144.0), h, (0.0, 108.0)),
        ];
        // The skewed wall, offset outward by its half thickness.
        let d = Vec2::new(120.0, -24.0);
        let n = Vec2::new(d.y, -d.x) * (1.0 / d.len());
        let a = Vec2::new(120.0, 0.0) + n * h;
        let b = Vec2::new(240.0, -24.0) + n * h;
        ps.push(piece(2, (a.x, a.y), (b.x, b.y), h, (0.0, 108.0)));
        // A door in the north wall: 36" gap under a head at 84".
        ps.retain(|p| p.wall != Handle(4));
        ps.push(piece(4, (0.0, 147.0), (100.0, 147.0), h, (0.0, 108.0)));
        ps.push(piece(4, (136.0, 147.0), (240.0, 147.0), h, (0.0, 108.0)));
        ps.push(piece(4, (100.0, 147.0), (136.0, 147.0), h, (84.0, 108.0)));
        let fs = faces(&[room("7", &pts)], &ps, 12.0, 0.0);
        let s = fs.iter().find(|f| f.name == "7/S").expect("7/S");
        assert_eq!(s.wall, Handle(1), "the square face keeps the plain name");
        let s2 = fs.iter().find(|f| f.name == "7/S2").expect("7/S2");
        assert_eq!(s2.wall, Handle(2));
        assert!(s2.skew_deg > SKEW_DEG);
        let north = fs.iter().find(|f| f.name == "7/N").expect("7/N");
        assert_eq!(north.openings.len(), 1, "{:?}", north.openings);
        let o = &north.openings[0];
        assert_eq!(o.kind, "door");
        assert!((o.to - o.from - 36.0).abs() < 1e-6 && (o.top - 84.0).abs() < 1e-6, "{o:?}");
    }
}
