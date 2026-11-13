//! Through-wall units (Q6546, trade practice `M-THROUGH-WALL`): a packaged terminal unit or
//! another wall-sleeve unit goes through the outside wall its plan symbol stands against.
//!
//! Plans draw such a unit's room cabinet in front of the wall, so its plan box gives only its
//! place along the wall. The field sets its sleeve through the wall, flush with the outside face,
//! the outside grille in the sleeve's end, and slides the unit in from the room: its cabinet
//! projects into the room by the unit's depth less the wall's thickness. The depth comes from the
//! unit's product data, else the rule's. A unit no outside wall is near is a review item, built
//! where it is drawn, never a box left floating in the room silently.

use cadaddy_doc::{Common, Drawing, EntityKind, Handle, Mesh, MeshFace, RawTag, Space};
use cadaddy_geom::{Vec2, Vec3};
use serde::Deserialize;

use super::propfill::{Footprints, WallPiece};

/// The trade practice rule this placement follows.
pub(crate) const RULE: &str = "M-THROUGH-WALL";
/// Most samples the behind-the-wall check takes along its probe line.
const MAX_SAMPLES: usize = 400;
/// A wall face this close to parallel to the unit's long side (|cos|) can be its wall.
const PARALLEL_COS: f64 = 0.94;
/// Product depths outside this range (inches) are a misread, not a unit's depth.
const DEPTH_RANGE_IN: (f64, f64) = (4.0, 60.0);

/// The numbers and kinds of [`RULE`] (rule data `tradepractice.json`).
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub(crate) struct Rule {
    pub kinds: Vec<String>,
    pub reach_in: f64,
    pub unit_depth_in: f64,
    pub sleeve_depth_in: f64,
    pub grille_in: f64,
    pub probe_in: f64,
    pub behind_ft: f64,
    pub basis: String,
}

impl Rule {
    /// The rule as its data has it (an empty rule, naming no kind, when the data has none).
    pub(crate) fn load() -> Self {
        serde_json::from_value(super::provenance::rule_data(RULE)).unwrap_or_default()
    }

    /// Whether `kind` is a through-wall unit's.
    pub(crate) fn names(&self, kind: &str) -> bool {
        let k = kind.trim();
        !k.is_empty() && self.kinds.iter().any(|x| x.trim().eq_ignore_ascii_case(k))
    }

    fn inches(v: f64, upf: f64) -> f64 {
        if v.is_finite() && v > 0.0 { v / 12.0 * upf } else { 0.0 }
    }
}

/// A box on a middle line `a`–`b`, `depth` across it (drawing units).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct PlanBox {
    pub a: Vec2,
    pub b: Vec2,
    pub depth: f64,
}

/// A through-wall unit seated in its outside wall.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Seat {
    /// Its host wall.
    pub wall: Handle,
    /// The wall's room face direction (outward, toward the room) and its run.
    pub out: Vec2,
    pub along: Vec2,
    /// The unit: from the wall's outside face into the room.
    pub unit: PlanBox,
    /// Its sleeve through the wall, and the outside grille in the sleeve's end.
    pub sleeve: PlanBox,
    pub grille: PlanBox,
    /// The wall's thickness and how far the cabinet stands past its room face (drawing units).
    pub thickness: f64,
    pub projection: f64,
    /// Its middle as drawn and as seated.
    pub drawn: Vec2,
    pub seated: Vec2,
    /// Where its depth came from.
    pub depth_from: String,
}

/// Where a through-wall unit goes.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Seated {
    In(Box<Seat>),
    /// No outside wall within reach: built as drawn, with this review text.
    Review(String),
}

/// Whether `p` is inside the polygon `poly` (ray crossing).
fn inside(poly: &[Vec2], p: Vec2) -> bool {
    let n = poly.len();
    let mut c = false;
    for i in 0..n {
        let (Some(a), Some(b)) = (poly.get(i), poly.get((i + n - 1) % n)) else { continue };
        if (a.y > p.y) != (b.y > p.y) {
            let x = (b.x - a.x) * (p.y - a.y) / (b.y - a.y) + a.x;
            if p.x < x {
                c = !c;
            }
        }
    }
    c
}

/// The unit's depth from its product data (a `DEPTH` column or field), inches, and where it came
/// from; else the rule's.
fn depth_in(e: &cadaddy_walk3d::MepEquipment, rule: &Rule) -> (f64, String) {
    let product = e.properties.iter().take(64).find_map(|(k, v)| {
        let key = k.trim().to_ascii_uppercase();
        if !(key.contains("DEPTH") || key == "D") {
            return None;
        }
        let ft = cadaddy_mepfix::equip_reader::parse_length_ft(v, true)?;
        let inches = ft * 12.0;
        (inches.is_finite() && (DEPTH_RANGE_IN.0..=DEPTH_RANGE_IN.1).contains(&inches))
            .then(|| (inches, format!("its product data ({} {})", k.trim(), v.trim())))
    });
    product.unwrap_or_else(|| (rule.unit_depth_in, format!("{RULE}: a unit's usual depth, {} in, no product data giving one", rule.unit_depth_in)))
}

/// Where through-wall unit `e` (its plan box) on `level` goes: through the outside wall whose
/// face it stands against (within the rule's reach, the face along its long side), its sleeve
/// flush with the outside face. `walls`: the level's built wall pieces where the plans draw them
/// ([`super::propfill::wall_footprints_on_plans`]); `fps`: the levels' floor and shell outlines
/// (a wall is an outside wall when the floor ends past it; a level with none: no wall of its
/// level stands behind it within the rule's `behind_ft`); `z`: the unit's middle height (drawing
/// units), only walls standing there.
pub(crate) fn seat(
    rule: &Rule,
    e: &cadaddy_walk3d::MepEquipment,
    level: &str,
    walls: &[WallPiece],
    fps: Option<&Footprints>,
    upf: f64,
    z: f64,
) -> Seated {
    let (lo, hi) = (Vec2::new(e.min[0].min(e.max[0]), e.min[1].min(e.max[1])), Vec2::new(e.min[0].max(e.max[0]), e.min[1].max(e.max[1])));
    let c = (lo + hi) * 0.5;
    let (wx, wy) = (hi.x - lo.x, hi.y - lo.y);
    let (long, short, ax) = if wx >= wy { (wx, wy, Vec2::new(1.0, 0.0)) } else { (wy, wx, Vec2::new(0.0, 1.0)) };
    let reach = Rule::inches(rule.reach_in, upf) + short / 2.0;
    if !(c.is_finite() && long > 0.0 && reach > 0.0) {
        return Seated::Review(format!("review: a through-wall unit with no plan size: built as read, not through a wall ({RULE})"));
    }
    let on_level = |l: &Option<String>| l.as_deref().is_none_or(|l| l.eq_ignore_ascii_case(level));
    let at_height = |zr: &[f64; 2]| !zr[0].is_finite() || (zr[0] <= z && z <= zr[1]);
    let probe = Rule::inches(rule.probe_in, upf).max(upf / 12.0);
    let behind = Rule::inches(rule.behind_ft * 12.0, upf);
    let level_fp = fps.and_then(|f| f.has(level).then_some(f));
    // Is the far side of wall `h` at its outside face `back` (facing `-out`) out of the building?
    let outside = |h: Handle, back: Vec2, out: Vec2| -> bool {
        let p = back - out * probe;
        if let Some(f) = level_fp {
            return f.outside(level, p).is_some_and(|d| d > 1e-6 * upf.max(1.0));
        }
        let step = (upf / 4.0).max(1e-6);
        let n = ((behind / step).ceil() as usize).min(MAX_SAMPLES);
        !(0..=n).any(|i| {
            let q = p - out * (step * i as f64);
            walls.iter().filter(|(l, wh, _, zr)| *wh != h && on_level(l) && at_height(zr)).any(|(_, _, poly, _)| inside(poly, q))
        })
    };
    // Each candidate face: (distance, wall, point on the face, run, outward, thickness).
    let mut best: Option<(f64, Handle, Vec2, Vec2, Vec2, f64)> = None;
    let mut nearest_inside: Option<f64> = None;
    for (_, h, poly, _) in walls.iter().filter(|(l, _, _, zr)| on_level(l) && at_height(zr)).take(200_000) {
        let n = poly.len();
        let edges: Vec<(Vec2, Vec2)> = (0..n).filter_map(|i| Some((*poly.get(i)?, *poly.get((i + 1) % n)?))).collect();
        let longest = edges.iter().map(|(a, b)| a.dist(*b)).fold(0.0, f64::max);
        if longest.is_nan() || longest <= 1e-9 {
            continue;
        }
        let ccw = cadaddy_geom::shoelace(poly) > 0.0;
        for (a, b) in edges.iter().filter(|(a, b)| a.dist(*b) >= 0.5 * longest) {
            let ab = *b - *a;
            let len = ab.len();
            let u = ab * (1.0 / len);
            if (u.x * ax.x + u.y * ax.y).abs() < PARALLEL_COS {
                continue;
            }
            let t = ((c - *a).x * u.x + (c - *a).y * u.y).clamp(0.0, len);
            let q = *a + u * t;
            let dd = q.dist(c);
            if dd.is_nan() || dd > reach || best.as_ref().is_some_and(|x| x.0 <= dd) {
                continue;
            }
            let out = if ccw { Vec2::new(u.y, -u.x) } else { Vec2::new(-u.y, u.x) };
            // The wall's thickness behind this face: its farthest corner across it.
            let thick = poly.iter().map(|v| (q - *v).x * out.x + (q - *v).y * out.y).fold(0.0, f64::max);
            // The unit stands on this face's side (or in the wall), not behind the wall.
            let side = (c - q).x * out.x + (c - q).y * out.y;
            if thick.is_nan() || thick <= 1e-9 || side < -thick {
                continue;
            }
            if !outside(*h, q - out * thick, out) {
                nearest_inside = Some(nearest_inside.map_or(dd, |x: f64| x.min(dd)));
                continue;
            }
            best = Some((dd, *h, q, u, out, thick));
        }
    }
    let Some((_, wall, q, u, out, thick)) = best else {
        let why = match nearest_inside {
            Some(d) => {
                format!("the nearest wall ({:.0} in off) has the building behind it, not the outside", (d - short / 2.0).max(0.0) / upf * 12.0)
            }
            None => format!("no wall within {:.0} in of it", rule.reach_in),
        };
        return Seated::Review(format!(
            "review: a through-wall unit with no outside wall to go through: {why}; built where its plan draws it, not through a wall ({RULE})"
        ));
    };
    let (d_in, depth_from) = depth_in(e, rule);
    let unit_depth = Rule::inches(d_in, upf).max(thick);
    let sleeve_depth = Rule::inches(rule.sleeve_depth_in, upf).max(thick);
    let grille_depth = Rule::inches(rule.grille_in, upf).min(sleeve_depth).max(upf / 48.0);
    let base = q - out * thick;
    // Its place along the wall, as drawn.
    let pb = base + u * ((c - base).x * u.x + (c - base).y * u.y);
    let half = u * (long / 2.0);
    let on = |depth: f64| {
        let mid = pb + out * (depth / 2.0);
        PlanBox { a: mid - half, b: mid + half, depth }
    };
    let unit = on(unit_depth);
    Seated::In(Box::new(Seat {
        wall,
        out,
        along: u,
        unit,
        sleeve: on(sleeve_depth),
        grille: on(grille_depth),
        thickness: thick,
        projection: unit_depth - thick,
        drawn: c,
        seated: (unit.a + unit.b) * 0.5,
        depth_from,
    }))
}

/// The xdata a seated unit carries: its host wall and face, its sleeve, its place's basis
/// (derived: the plan's place along the wall and [`RULE`]) and its depth's source.
pub(crate) fn seat_props(s: &Seat, upf: f64) -> Vec<(String, String)> {
    let inch = |v: f64| v / upf * 12.0;
    vec![
        ("HOST_WALL".into(), s.wall.hex()),
        ("HOST_FACE".into(), format!("{:.3},{:.3}", s.out.x, s.out.y)),
        ("MOUNTING".into(), "through the outside wall in its wall sleeve".into()),
        (
            "PLACEMENT".into(),
            format!(
                "derived: its plan place along the wall, through the wall by {RULE} (sleeve {:.2} in deep through a {:.2} in wall, cabinet {:.2} in into the room)",
                inch(s.sleeve.depth),
                inch(s.thickness),
                inch(s.projection)
            ),
        ),
        ("SLEEVE".into(), format!("wall sleeve {:.2} in deep, flush with the outside face, outside grille in its end", inch(s.sleeve.depth))),
        ("DEPTH_IN".into(), format!("{:.2}", inch(s.unit.depth))),
        ("SOURCE depth".into(), s.depth_from.clone()),
    ]
}

/// The design-against-installed record of a seated unit's place (feet, on the plans): kept beside
/// the corrected place, with the reason and the rule.
pub(crate) fn location_correction(s: &Seat, upf: f64) -> Option<cadaddy_walk3d::RunCorrection> {
    let ft = |p: Vec2| format!("{:.2}, {:.2}", p.x / upf, p.y / upf);
    (s.drawn.dist(s.seated) > upf / 48.0).then(|| cadaddy_walk3d::RunCorrection {
        key: "location".into(),
        design: ft(s.drawn),
        corrected: ft(s.seated),
        reason: format!(
            "the plan draws the room cabinet in front of the wall; the unit goes through the outside wall in its sleeve ({:.1} in moved)",
            s.drawn.dist(s.seated) / upf * 12.0
        ),
        rule: RULE.into(),
    })
}

/// The kinds a seated unit's sleeve and outside grille are built as (their own elements, so the
/// takeoff counts them).
pub(crate) const SLEEVE_KIND: &str = "wall_sleeve";
pub(crate) const GRILLE_KIND: &str = "outside_grille";

/// Build a seated unit's wall sleeve and outside grille (heights `z0`..`z1`, drawing units) on
/// `layer`: derived elements of its trade, each naming the unit it serves and its host wall.
#[allow(clippy::too_many_arguments)]
pub(crate) fn build_parts(
    d: &mut Drawing,
    layer: &str,
    level: &str,
    e: &cadaddy_walk3d::MepEquipment,
    s: &Seat,
    z0: f64,
    z1: f64,
    upf: f64,
) -> usize {
    let basis = super::provenance::rule(RULE).basis_for(&format!("for unit {}", e.tag.trim()));
    let mut n = 0;
    for (kind, what, b) in [(SLEEVE_KIND, "SLEEVE", &s.sleeve), (GRILLE_KIND, "GRILLE", &s.grille)] {
        let mut m = super::wallgen::empty_mesh();
        super::wallgen::add_box(&mut m, b.a, b.b, b.depth, z0, z1);
        if m.faces.is_empty() {
            continue;
        }
        let t = |v: String| RawTag { code: 1000, value: v.chars().take(250).collect() };
        let mut x = vec![
            RawTag { code: 1001, value: super::wallgen::APP.into() },
            t("mep".into()),
            t(level.to_string()),
            t(Handle(0).hex()),
            t("kind:equipment".into()),
            t(format!("part:equipment:{kind}")),
            t(format!("tag:{} {what}", e.tag.trim())),
            t("trade:mechanical".into()),
            t(format!("prop:SERVES={}", e.tag.trim())),
            t(format!("prop:HOST_WALL={}", s.wall.hex())),
            t(format!("prop:HOST_FACE={:.3},{:.3}", s.out.x, s.out.y)),
            t(format!("prop:SIZE={:.2} in long x {:.2} in deep", b.a.dist(b.b) / upf * 12.0, b.depth / upf * 12.0)),
        ];
        x.extend(super::provenance::tags(cadaddy_walk3d::Made::Derived, &basis));
        if d.add(&Space::Model, Common { layer: layer.into(), xdata: x.into(), ..Common::default() }, EntityKind::Mesh(m)).is_ok() {
            n += 1;
        }
    }
    n
}

/// The part of `poly` where `f` ≤ `c` (Sutherland–Hodgman against one half-plane).
fn clip(poly: &[Vec2], f: &dyn Fn(Vec2) -> f64, c: f64) -> Vec<Vec2> {
    let n = poly.len();
    let mut out = Vec::with_capacity(n + 2);
    for i in 0..n {
        let (Some(&p), Some(&q)) = (poly.get(i), poly.get((i + 1) % n)) else { continue };
        let (fp, fq) = (f(p) - c, f(q) - c);
        if fp <= 0.0 {
            out.push(p);
        }
        if (fp < 0.0 && fq > 0.0) || (fp > 0.0 && fq < 0.0) {
            out.push(p + (q - p) * (fp / (fp - fq)));
        }
    }
    out
}

/// Add an upright prism over `poly` from `z0` to `z1` to `m` (its outline turned counter-clockwise).
fn add_prism(m: &mut Mesh, poly: &[Vec2], z0: f64, z1: f64) {
    if poly.len() < 3 || z1.is_nan() || z0.is_nan() || z1 <= z0 || cadaddy_geom::shoelace(poly).abs() <= 1e-12 {
        return;
    }
    let mut p = poly.to_vec();
    if cadaddy_geom::shoelace(&p) < 0.0 {
        p.reverse();
    }
    let n = p.len();
    let Ok(base) = u32::try_from(m.vertices.len()) else { return };
    let Ok(nn) = u32::try_from(n) else { return };
    for z in [z0, z1] {
        m.vertices.extend(p.iter().map(|v| Vec3::new(v.x, v.y, z)));
    }
    m.faces.push(MeshFace { v: (0..nn).rev().map(|i| base + i).collect(), hidden: 0 });
    m.faces.push(MeshFace { v: (0..nn).map(|i| base + nn + i).collect(), hidden: 0 });
    for i in 0..nn {
        let j = (i + 1) % nn;
        m.faces.push(MeshFace { v: vec![base + i, base + j, base + nn + j, base + nn + i], hidden: 0 });
    }
}

/// Cut each seated unit's sleeve opening (`z0`..`z1`, drawing units) in its host wall: the wall's
/// pieces the sleeve passes through split into the wall beside it, a sill under it and a head
/// over it, every opening of one wall at once. A wall whose mesh is not plain upright pieces (its
/// faces other than its pieces' tops, bottoms and sides) is left whole. Returns (walls cut,
/// openings cut, openings left uncut).
pub(crate) fn cut_sleeves(d: &mut Drawing, seats: &[(Seat, f64, f64)]) -> (usize, usize, usize) {
    let mut by_wall: std::collections::BTreeMap<Handle, Vec<&(Seat, f64, f64)>> = std::collections::BTreeMap::new();
    for s in seats {
        by_wall.entry(s.0.wall).or_default().push(s);
    }
    let (mut walls, mut cut, mut left) = (0, 0, 0);
    for (h, list) in by_wall {
        let Some(e) = d.model.get(h) else {
            left += list.len();
            continue;
        };
        let EntityKind::Mesh(m) = &e.kind else {
            left += list.len();
            continue;
        };
        let back = super::bspec::stacked_by(e);
        let pieces = super::propfill::mesh_pieces(m);
        let plain = pieces.iter().map(|(o, _)| 2 + o.len()).sum::<usize>() == m.faces.len();
        if !plain || pieces.is_empty() {
            left += list.len();
            continue;
        }
        let mut prisms: Vec<(Vec<Vec2>, f64, f64)> = pieces.into_iter().map(|(o, z)| (o, z[0], z[1])).collect();
        let mut done = 0;
        for (s, z0, z1) in list {
            let u = s.along;
            let out = s.out;
            let a = s.sleeve.a - out * (s.sleeve.depth / 2.0) + back;
            let len = s.sleeve.a.dist(s.sleeve.b);
            let f = move |p: Vec2| (p - a).x * u.x + (p - a).y * u.y;
            let g = move |p: Vec2| (p - a).x * out.x + (p - a).y * out.y;
            let tol = (s.thickness * 0.05).max(1e-6);
            let mut next = Vec::with_capacity(prisms.len() + 4);
            let mut hit = false;
            for (poly, za, zb) in prisms {
                let (fmin, fmax) = poly.iter().fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), p| (lo.min(f(*p)), hi.max(f(*p))));
                let (gmin, gmax) = poly.iter().fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), p| (lo.min(g(*p)), hi.max(g(*p))));
                let through = fmax > tol && fmin < len - tol && zb > *z0 && za < *z1 && gmin >= -tol && gmax <= s.thickness + tol;
                if !through {
                    next.push((poly, za, zb));
                    continue;
                }
                hit = true;
                let neg = move |p: Vec2| -f(p);
                let left_part = clip(&poly, &f, 0.0);
                let right_part = clip(&poly, &neg, -len);
                let mid = clip(&clip(&poly, &neg, 0.0), &f, len);
                for part in [left_part, right_part] {
                    if part.len() >= 3 {
                        next.push((part, za, zb));
                    }
                }
                if mid.len() >= 3 {
                    if *z0 > za {
                        next.push((mid.clone(), za, *z0));
                    }
                    if zb > *z1 {
                        next.push((mid, *z1, zb));
                    }
                }
            }
            prisms = next;
            if hit {
                done += 1;
            } else {
                left += 1;
            }
        }
        if done == 0 {
            continue;
        }
        let mut nm = super::wallgen::empty_mesh();
        for (poly, za, zb) in &prisms {
            add_prism(&mut nm, poly, *za, *zb);
        }
        if nm.faces.is_empty() {
            left += done;
            continue;
        }
        if d.model.modify(h, |e| e.kind = EntityKind::Mesh(nm)) {
            walls += 1;
            cut += done;
        } else {
            left += done;
        }
    }
    (walls, cut, left)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A wall piece: a rectangle `x0..x1` by `y0..y1` (inches, 12 units a foot), counter-clockwise.
    fn piece(h: u64, x0: f64, y0: f64, x1: f64, y1: f64) -> WallPiece {
        (Some("L1".into()), Handle(h), vec![Vec2::new(x0, y0), Vec2::new(x1, y0), Vec2::new(x1, y1), Vec2::new(x0, y1)], [0.0, 120.0])
    }

    fn ptac(x0: f64, y0: f64, x1: f64, y1: f64) -> cadaddy_walk3d::MepEquipment {
        cadaddy_walk3d::MepEquipment {
            tag: "U-1".into(),
            kind: "ptac".into(),
            level: "L1".into(),
            min: [x0, y0],
            max: [x1, y1],
            bottom_ft: 4.0 / 12.0,
            height_ft: 16.0 / 12.0,
            ..Default::default()
        }
    }

    /// A 20' x 10' room of 7.25" walls, the south wall 0..7.25 in.
    fn room() -> Vec<WallPiece> {
        vec![
            piece(1, 0.0, 0.0, 240.0, 7.25),
            piece(2, 0.0, 112.75, 240.0, 120.0),
            piece(3, 0.0, 0.0, 7.25, 120.0),
            piece(4, 232.75, 0.0, 240.0, 120.0),
        ]
    }

    #[test]
    fn the_rule_names_the_through_wall_kinds_in_its_data() {
        let r = Rule::load();
        assert!(r.names("PTAC") && r.names("through_wall_unit") && !r.names("ahu") && !r.names(""), "{r:?}");
        assert!(r.reach_in > 0.0 && r.unit_depth_in > r.sleeve_depth_in && r.sleeve_depth_in > 0.0, "{r:?}");
        assert!(!super::super::provenance::rule(RULE).basis.is_empty());
    }

    #[test]
    fn a_unit_drawn_in_front_of_the_outside_wall_goes_through_it() {
        let r = Rule::load();
        // Drawn 8" in front of the south wall's room face (7.25"), 42" x 12".
        let e = ptac(60.0, 15.25, 102.0, 27.25);
        let Seated::In(s) = seat(&r, &e, "L1", &room(), None, 12.0, 12.0) else { panic!("seated") };
        assert_eq!(s.wall, Handle(1));
        assert!((s.thickness - 7.25).abs() < 1e-6, "{s:?}");
        // From the outside face (y 0) into the room by the unit's depth; along the wall as drawn.
        let ys = [s.unit.a.y, s.unit.b.y];
        assert!(ys.iter().all(|y| (y - r.unit_depth_in / 2.0).abs() < 1e-6), "{s:?}");
        let (x0, x1) = (s.unit.a.x.min(s.unit.b.x), s.unit.a.x.max(s.unit.b.x));
        assert!((x0 - 60.0).abs() < 1e-6 && (x1 - 102.0).abs() < 1e-6, "{s:?}");
        assert!((s.unit.depth - r.unit_depth_in).abs() < 1e-6 && (s.projection - (r.unit_depth_in - 7.25)).abs() < 1e-6);
        assert!((s.sleeve.depth - r.sleeve_depth_in).abs() < 1e-6 && (s.sleeve.a.y - r.sleeve_depth_in / 2.0).abs() < 1e-6);
        assert!(s.grille.depth > 0.0 && s.grille.a.y < 1.0, "the grille at the outside face: {s:?}");
        assert!((s.out.y - 1.0).abs() < 1e-9, "faces the room: {s:?}");
        let c = location_correction(&s, 12.0).expect("moved");
        assert_eq!((c.key.as_str(), c.design.as_str(), c.rule.as_str()), ("location", "6.75, 1.77", RULE));
        let props = seat_props(&s, 12.0);
        assert!(props.iter().any(|(k, v)| k == "HOST_WALL" && v == &Handle(1).hex()), "{props:?}");
        assert!(props.iter().any(|(k, v)| k == "PLACEMENT" && v.starts_with("derived:") && v.contains(RULE)), "{props:?}");
    }

    #[test]
    fn its_product_depth_governs_over_the_rule() {
        let r = Rule::load();
        let mut e = ptac(60.0, 15.25, 102.0, 27.25);
        e.properties.insert("DEPTH (IN)".into(), "16".into());
        let Seated::In(s) = seat(&r, &e, "L1", &room(), None, 12.0, 12.0) else { panic!("seated") };
        assert!((s.unit.depth - 16.0).abs() < 1e-6 && s.depth_from.contains("product data"), "{s:?}");
        // A wall thicker than the unit: the unit is in the wall, never short of it.
        let mut thick = room();
        thick[0] = piece(1, 0.0, 0.0, 240.0, 18.0);
        let e = ptac(60.0, 26.0, 102.0, 38.0);
        let Seated::In(s) = seat(&r, &e, "L1", &thick, None, 12.0, 12.0) else { panic!("seated") };
        assert!(s.unit.depth >= 18.0 && s.projection >= 0.0, "{s:?}");
    }

    #[test]
    fn a_unit_with_no_outside_wall_near_is_a_review_item() {
        let r = Rule::load();
        // In the middle of the room: no wall within reach.
        let Seated::Review(why) = seat(&r, &ptac(100.0, 50.0, 142.0, 62.0), "L1", &room(), None, 12.0, 12.0) else { panic!("review") };
        assert!(why.starts_with("review:") && why.contains("no wall within"), "{why}");
        // Against a wall with the building behind it (a second room past the south wall).
        let mut two = room();
        two.push(piece(5, 0.0, -120.0, 240.0, -112.75));
        two.push(piece(6, 0.0, -120.0, 7.25, 0.0));
        let Seated::Review(why) = seat(&r, &ptac(60.0, 15.25, 102.0, 27.25), "L1", &two, None, 12.0, 12.0) else { panic!("review") };
        assert!(why.contains("the building behind it"), "{why}");
    }

    #[test]
    fn a_unit_across_the_wall_run_is_not_put_through_it() {
        // Its long side across the south wall: the south wall is no wall its sleeve goes through.
        let r = Rule::load();
        let e = ptac(60.0, 15.25, 72.0, 57.25);
        assert!(matches!(seat(&r, &e, "L1", &room()[..1], None, 12.0, 12.0), Seated::Review(_)));
    }
}
