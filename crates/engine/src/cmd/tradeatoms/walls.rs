//! The walls' pieces: each built wall laid out from its wall type's layers (the partition legend)
//! — studs at their spacing with king, jack and cripple studs at the openings, plates or tracks
//! cut to stock, headers and sills, fire blocking, backing behind what hangs on the wall,
//! masonry courses, a batt in each stud bay, sheathing and board sheets per face and layer with
//! the openings taken out, furring rows, corner bead, tape, joint compound, screws, and paint
//! per coat on the faces that face a room.
//!
//! A built wall's mesh is the boxes `wallgen` made of it (its pieces between the openings, the
//! headers over them, the sills under windows), so its centreline, thickness, height and
//! openings are read back from them; side 1 is left of its centreline's direction.

use super::*;
use cadaddy_walk3d::{Assembly, AssemblyLayer, LayerSide};

/// Most walls laid out (hostile models).
const MAX_WALLS: usize = 200_000;
/// Most studs, sheets or courses one wall gets.
const MAX_PER_WALL: usize = 5_000;

/// One box of a wall's mesh: centreline ends, thickness, bottom, top (sheet units).
#[derive(Clone, Copy, Debug)]
struct WBox {
    a: Vec2,
    b: Vec2,
    t: f64,
    z0: f64,
    z1: f64,
}

/// The boxes `wallgen::add_box` made a mesh of (8 corners each); `None` when the mesh is not
/// made of such boxes (then its extents stand for one box along its longer side).
fn boxes(m: &Mesh) -> Option<Vec<WBox>> {
    let v = &m.vertices;
    if v.is_empty() || !v.len().is_multiple_of(8) || m.faces.len() != v.len() / 8 * 6 {
        return None;
    }
    let mut out = Vec::with_capacity(v.len() / 8);
    for c in v.chunks(8) {
        let [p0, p1, p2, p3, q0, q1, q2, q3] = c else { return None };
        let flat = |a: &Vec3, b: &Vec3| (a.z - b.z).abs() < 1e-6;
        let above = |a: &Vec3, b: &Vec3| (a.x - b.x).abs() < 1e-6 && (a.y - b.y).abs() < 1e-6;
        if !(flat(p0, p1) && flat(p0, p2) && flat(p0, p3) && flat(q0, q1) && flat(q0, q2) && flat(q0, q3)) {
            return None;
        }
        if !(above(p0, q0) && above(p1, q1) && above(p2, q2) && above(p3, q3)) {
            return None;
        }
        let xy = |p: &Vec3| Vec2::new(p.x, p.y);
        let a = (xy(p0) + xy(p3)) * 0.5;
        let b = (xy(p1) + xy(p2)) * 0.5;
        let t = xy(p0).dist(xy(p3));
        if !(a.is_finite() && b.is_finite() && t.is_finite() && t > 0.0) {
            return None;
        }
        out.push(WBox { a, b, t, z0: p0.z.min(q0.z), z1: p0.z.max(q0.z) });
    }
    Some(out)
}

/// One box standing for a mesh that is not made of boxes: its extents along the longer side.
fn extents_box(m: &Mesh) -> Option<WBox> {
    let pts: Vec<Vec3> = m.vertices.iter().copied().filter(|p| p.is_finite()).take(100_000).collect();
    let (mut lo, mut hi) =
        (Vec3::new(f64::INFINITY, f64::INFINITY, f64::INFINITY), Vec3::new(f64::NEG_INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY));
    for p in &pts {
        lo = Vec3::new(lo.x.min(p.x), lo.y.min(p.y), lo.z.min(p.z));
        hi = Vec3::new(hi.x.max(p.x), hi.y.max(p.y), hi.z.max(p.z));
    }
    if !(lo.is_finite() && hi.is_finite() && hi.z > lo.z) {
        return None;
    }
    let (dx, dy) = (hi.x - lo.x, hi.y - lo.y);
    let (a, b, t) = if dx >= dy {
        (Vec2::new(lo.x, (lo.y + hi.y) / 2.0), Vec2::new(hi.x, (lo.y + hi.y) / 2.0), dy)
    } else {
        (Vec2::new((lo.x + hi.x) / 2.0, lo.y), Vec2::new((lo.x + hi.x) / 2.0, hi.y), dx)
    };
    (t > 0.0 && a.dist(b) > 0.0).then_some(WBox { a, b, t, z0: lo.z, z1: hi.z })
}

/// An opening through a wall: along it from `s0` to `s1`, from `z0` up to `z1` (sheet units).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Hole {
    pub s0: f64,
    pub s1: f64,
    pub z0: f64,
    pub z1: f64,
}

impl Hole {
    fn overlap(&self, s0: f64, s1: f64, z0: f64, z1: f64) -> f64 {
        (self.s1.min(s1) - self.s0.max(s0)).max(0.0) * (self.z1.min(z1) - self.z0.max(z0)).max(0.0)
    }
}

/// A built wall read back: its axis (origin, direction, side 1's normal), length range,
/// thickness, floor and top, and openings.
#[derive(Clone, Debug)]
pub(super) struct Wall {
    pub handle: Handle,
    pub level: String,
    pub wall_type: String,
    pub o: Vec2,
    pub u: Vec2,
    pub n: Vec2,
    pub s0: f64,
    pub s1: f64,
    pub t: f64,
    pub zf: f64,
    pub zt: f64,
    pub holes: Vec<Hole>,
}

impl Wall {
    /// A point on the wall: `s` along it, `off` towards side 1 from its centreline.
    pub fn at(&self, s: f64, off: f64) -> Vec2 {
        self.o + self.u * s + self.n * off
    }
    /// A block along the wall from `s0` to `s1`, centred `off` towards side 1, `depth` across it.
    fn block(&self, s0: f64, s1: f64, off: f64, depth: f64, z0: f64, z1: f64) -> Block {
        Block::new(self.at(s0, off), self.at(s1, off), depth, z0, z1)
    }
    fn len(&self) -> f64 {
        self.s1 - self.s0
    }
    /// Its end points on the centreline.
    pub fn ends(&self) -> [Vec2; 2] {
        [self.at(self.s0, 0.0), self.at(self.s1, 0.0)]
    }
}

/// Read a built wall back from its mesh.
pub(super) fn read_wall(e: &cadaddy_doc::Entity) -> Option<Wall> {
    let EntityKind::Mesh(m) = &e.kind else { return None };
    let bx = boxes(m).filter(|b| !b.is_empty()).or_else(|| extents_box(m).map(|b| vec![b]))?;
    let main = bx.iter().copied().max_by(|x, y| x.a.dist(x.b).total_cmp(&y.a.dist(y.b)))?;
    let len = main.a.dist(main.b);
    if len.is_nan() || len <= 1e-9 {
        return None;
    }
    let u = (main.b - main.a) * (1.0 / len);
    let n = u.perp();
    let o = main.a;
    // The boxes on its line (a header over the gap after a wall may stand on another line).
    let on: Vec<(f64, f64, f64, f64)> = bx
        .iter()
        .filter(|b| {
            let d = b.b - b.a;
            let l = d.len();
            let along = l < 1e-9 || (d.x * u.y - d.y * u.x).abs() / l < 0.02;
            along && ((b.a - o).dot(n)).abs() < main.t.max(b.t)
        })
        .map(|b| {
            let (sa, sb) = ((b.a - o).dot(u), (b.b - o).dot(u));
            (sa.min(sb), sa.max(sb), b.z0, b.z1)
        })
        .collect();
    let s0 = on.iter().map(|r| r.0).fold(f64::INFINITY, f64::min);
    let s1 = on.iter().map(|r| r.1).fold(f64::NEG_INFINITY, f64::max);
    let zf = on.iter().map(|r| r.2).fold(f64::INFINITY, f64::min);
    let zt = on.iter().map(|r| r.3).fold(f64::NEG_INFINITY, f64::max);
    if !(s0.is_finite() && s1 > s0 && zf.is_finite() && zt > zf) {
        return None;
    }
    let holes = holes_of(&on, s0, s1, zf, zt, main.t.max(1e-6));
    Some(Wall {
        handle: e.handle,
        level: super::super::bspec::made_level(e).unwrap_or_default(),
        wall_type: super::super::bimlook::made_tag(e, "type").unwrap_or_default().to_string(),
        o,
        u,
        n,
        s0,
        s1,
        t: main.t,
        zf,
        zt,
        holes,
    })
}

/// The openings the boxes leave in the wall's face (`[s0, s1]` by `[zf, zt]`).
fn holes_of(rects: &[(f64, f64, f64, f64)], s0: f64, s1: f64, zf: f64, zt: f64, t: f64) -> Vec<Hole> {
    let tol = (t * 0.05).max(1e-4);
    let mut xs: Vec<f64> = rects.iter().flat_map(|r| [r.0, r.1]).chain([s0, s1]).collect();
    xs.sort_by(f64::total_cmp);
    xs.dedup_by(|a, b| (*a - *b).abs() < tol);
    let mut cells: Vec<Hole> = Vec::new();
    for w in xs.windows(2).take(MAX_PER_WALL) {
        let [x0, x1] = w else { continue };
        if x1 - x0 < tol {
            continue;
        }
        let xm = (x0 + x1) / 2.0;
        let cover = geo::merge(rects.iter().filter(|r| r.0 <= xm && r.1 >= xm).map(|r| (r.2, r.3)).collect());
        let mut z = zf;
        for (lo, hi) in cover.iter().copied().chain(std::iter::once((zt, zt))) {
            if lo - z > tol {
                cells.push(Hole { s0: *x0, s1: *x1, z0: z, z1: lo.min(zt) });
            }
            z = z.max(hi);
        }
    }
    // Neighbouring cells of the same height are one opening.
    let mut out: Vec<Hole> = Vec::new();
    for c in cells {
        match out.last_mut() {
            Some(l) if (l.s1 - c.s0).abs() < tol && (l.z0 - c.z0).abs() < tol && (l.z1 - c.z1).abs() < tol => l.s1 = c.s1,
            _ => out.push(c),
        }
    }
    out.retain(|h| h.s1 - h.s0 > tol * 4.0 && h.z1 - h.z0 > tol * 4.0);
    out
}

/// A layer of the wall: its assembly layer and where it sits across the wall (its centre's
/// offset towards side 1 and its depth, sheet units).
struct Placed<'a> {
    layer: &'a AssemblyLayer,
    off: f64,
    depth: f64,
}

/// The assembly's layers placed across a wall `t` thick (side 1's face first), scaled so they
/// fill it; cavity insulation shares its studs' place.
fn place_layers<'a>(asm: &'a Assembly, t: f64, k: f64) -> Vec<Placed<'a>> {
    let total: f64 = asm.layers.iter().filter(|l| !l.in_cavity).map(|l| l.thickness_in.max(0.0)).sum::<f64>() * k;
    let scale = if total > 1e-9 { (t / total).clamp(0.25, 4.0) } else { 1.0 };
    let mut c = t / 2.0;
    let mut out = Vec::new();
    let mut last_core: Option<(f64, f64)> = None;
    for l in asm.layers.iter().take(cadaddy_walk3d::MAX_ASSEMBLY_LAYERS) {
        let depth = l.thickness_in.max(0.0) * k * scale;
        if l.in_cavity {
            let (off, d) = last_core.unwrap_or((0.0, depth));
            out.push(Placed { layer: l, off, depth: d.min(depth.max(d)) });
            continue;
        }
        let off = c - depth / 2.0;
        c -= depth;
        if l.side == LayerSide::Core {
            last_core = Some((off, depth));
        }
        out.push(Placed { layer: l, off, depth });
    }
    out
}

/// The board layers of a side, its face first.
fn face_layers<'a, 'b>(placed: &'b [Placed<'a>], side: LayerSide, cats: &[&str]) -> Vec<&'b Placed<'a>> {
    let mut v: Vec<&Placed> = placed.iter().filter(|p| p.layer.side == side && cats.contains(&p.layer.category.as_str())).collect();
    if side == LayerSide::Side2 {
        v.reverse();
    }
    v
}

/// Inches a material's text begins with or names (`3 1/2" metal studs` → 3.5, `5/8"` → 0.625).
pub(super) fn text_inches(text: &str) -> Option<f64> {
    let t = text.replace(['”', '″'], "\"");
    let q = t.find('"')?;
    let head = t.get(..q)?.trim_end();
    let tail: String = head.chars().rev().take_while(|c| c.is_ascii_digit() || *c == '/' || *c == ' ' || *c == '-' || *c == '.').collect();
    let s: String = tail.chars().rev().collect();
    let s = s.trim().replace('-', " ");
    super::docs::whole_and_fraction(&s).filter(|v| *v > 0.0 && *v < 100.0)
}

/// The spacing a text names (`@ 16" o.c.`, `24" OC`), inches.
pub(super) fn text_spacing(text: &str) -> Option<f64> {
    let up = text.to_ascii_uppercase().replace(['”', '″'], "\"");
    let i = up.find("O.C").or_else(|| up.find(" OC"))?;
    let before = up.get(..i)?.trim_end().trim_end_matches('"').trim_end();
    // The number just before it (`@ 16`, `channels 24`).
    let last: String =
        before.chars().rev().take_while(|c| c.is_ascii_digit() || matches!(c, '/' | ' ' | '.' | '-')).collect::<String>().chars().rev().collect();
    text_inches(&format!("{}\"", last.trim()))
}

/// A framing layer's depth: its thickness, else the size its text begins with.
fn layer_depth(l: &AssemblyLayer) -> f64 {
    if l.thickness_in > 0.0 && l.thickness_in.is_finite() { l.thickness_in } else { text_inches(&l.material).unwrap_or(0.0) }
}

/// The gauge a text names (`20 ga`, `18 GA.`, `20 gauge`).
fn text_gauge(text: &str) -> Option<String> {
    let up = text.to_ascii_uppercase();
    let i = up.find(" GA").or_else(|| up.find("GA "))?;
    let head = up.get(..i)?.trim_end();
    let num: String = head.chars().rev().take_while(char::is_ascii_digit).collect::<String>().chars().rev().collect();
    (!num.is_empty() && num.len() <= 2).then(|| format!("{num} ga"))
}

/// Whether a framing material is wood (else steel), by the rules' words.
fn is_wood(r: &Rules, material: &str) -> bool {
    let up = format!(" {} ", material.to_ascii_uppercase());
    let wood = r.words("framing.wood_words").iter().any(|w| up.contains(w.as_str()));
    let steel = r.words("framing.steel_words").iter().any(|w| up.contains(w.as_str()));
    wood && !steel
}

/// A wood stud's name by its depth: 3 1/2" → 2x4.
fn lumber(depth_in: f64) -> String {
    let nominal = match depth_in {
        d if d <= 2.75 => 3,
        d if d <= 4.0 => 4,
        d if d <= 6.0 => 6,
        d if d <= 8.0 => 8,
        d if d <= 10.0 => 10,
        _ => 12,
    };
    format!("2x{nominal}")
}

/// A board's type by the rules' words in its material (`Type X`, `Moisture resistant`).
pub(super) fn board_type(r: &Rules, material: &str) -> String {
    let up = format!(" {} ", material.to_ascii_uppercase());
    for t in r.list("drywall.types") {
        let words: Vec<String> = t
            .get("words")
            .and_then(Value::as_array)
            .map(|a| a.iter().filter_map(Value::as_str).map(str::to_ascii_uppercase).collect())
            .unwrap_or_default();
        if words.iter().any(|w| up.contains(w.as_str())) {
            return t.get("type").and_then(Value::as_str).unwrap_or_default().to_string();
        }
    }
    r.text("drywall.regular_type")
}

/// The R-value a material names (`R-13`, `R13`), else the rules' for the cavity depth.
pub(super) fn r_value(r: &Rules, material: &str, depth_in: f64) -> (String, Source) {
    let up = material.to_ascii_uppercase();
    if let Some(i) = up
        .find("R-")
        .or_else(|| up.find("R "))
        .or_else(|| up.find('R').filter(|i| up.get(i + 1..i + 2).is_some_and(|c| c.chars().all(|c| c.is_ascii_digit()))))
    {
        let num: String =
            up.get(i + 1..).unwrap_or_default().trim_start_matches(['-', ' ']).chars().take_while(|c| c.is_ascii_digit() || *c == '.').collect();
        if !num.is_empty() && num.parse::<f64>().is_ok_and(|v| v > 0.0 && v < 100.0) {
            return (format!("R-{num}"), Source::computed());
        }
    }
    for row in r.list("insulation.r_by_cavity_in") {
        let Some(a) = row.as_array() else { continue };
        let (Some(d), Some(v)) = (a.first().and_then(Value::as_f64), a.get(1).and_then(Value::as_str)) else { continue };
        if depth_in <= d + 0.01 {
            return (v.to_string(), r.source("insulation.r_by_cavity_in"));
        }
    }
    (String::new(), r.source("insulation.r_by_cavity_in"))
}

/// A wall end's neighbours: how many walls meet it, and whether one runs on through it.
#[derive(Clone, Copy, Default)]
struct EndJoin {
    /// Another wall's end meets this end (an L corner).
    corner: bool,
    /// This end stops against another wall's face (a T).
    tee: bool,
}

/// Each wall's two ends: what meets them (walls of one level).
fn joins(walls: &[Wall]) -> Vec<[EndJoin; 2]> {
    let mut out = vec![[EndJoin::default(); 2]; walls.len()];
    // A grid of end points and segments by cell.
    let cell = walls.iter().map(|w| w.t).fold(0.0f64, f64::max).max(1.0) * 4.0;
    let key = |p: Vec2| ((p.x / cell).floor() as i64, (p.y / cell).floor() as i64);
    let mut grid: HashMap<(i64, i64), Vec<usize>> = HashMap::new();
    for (i, w) in walls.iter().enumerate() {
        let [a, b] = w.ends();
        let (k0, k1) = (key(Vec2::new(a.x.min(b.x), a.y.min(b.y))), key(Vec2::new(a.x.max(b.x), a.y.max(b.y))));
        if (k1.0 - k0.0 + 1).saturating_mul(k1.1 - k0.1 + 1) > 4_000 {
            continue;
        }
        for x in k0.0 - 1..=k1.0 + 1 {
            for y in k0.1 - 1..=k1.1 + 1 {
                grid.entry((x, y)).or_default().push(i);
            }
        }
    }
    for (i, w) in walls.iter().enumerate() {
        for (k, p) in w.ends().into_iter().enumerate() {
            let mut j = EndJoin::default();
            for &o in grid.get(&key(p)).into_iter().flatten() {
                let Some(ow) = walls.get(o).filter(|_| o != i) else { continue };
                let reach = (w.t + ow.t) / 2.0 + w.t.max(ow.t) * 0.5;
                let [oa, ob] = ow.ends();
                if oa.dist(p) < reach || ob.dist(p) < reach {
                    j.corner = true;
                } else {
                    let s = (p - ow.o).dot(ow.u);
                    let off = (p - ow.o).dot(ow.n).abs();
                    if s > ow.s0 && s < ow.s1 && off < reach {
                        j.tee = true;
                    }
                }
            }
            if let Some(slot) = out.get_mut(i).and_then(|x| x.get_mut(k)) {
                *slot = j;
            }
        }
    }
    out
}

/// Lay out every built wall's pieces.
pub(super) fn lay_out(d: &Drawing, sp: &BuildingSpec, r: &Rules, ctx: &Ctx, batch: &mut Batch, want: &dyn Fn(&str) -> bool) {
    let k = ctx.upf / 12.0;
    let walls: Vec<Wall> = d
        .model
        .iter()
        .filter(|e| super::super::wallgen::made_as(e).as_deref() == Some("wall"))
        .filter_map(|e| read_wall(e))
        .take(MAX_WALLS)
        .collect();
    // The ends of each level's walls.
    let mut by_level: BTreeMap<String, Vec<usize>> = BTreeMap::new();
    for (i, w) in walls.iter().enumerate() {
        by_level.entry(w.level.to_ascii_uppercase()).or_default().push(i);
    }
    let mut end_joins: Vec<[EndJoin; 2]> = vec![[EndJoin::default(); 2]; walls.len()];
    for idx in by_level.values() {
        let lw: Vec<Wall> = idx.iter().filter_map(|i| walls.get(*i).cloned()).collect();
        for (j, jn) in joins(&lw).into_iter().enumerate() {
            if let Some(slot) = idx.get(j).and_then(|i| end_joins.get_mut(*i)) {
                *slot = jn;
            }
        }
    }
    let mut untyped = 0usize;
    let mut no_layers: BTreeSet<String> = BTreeSet::new();
    for (i, w) in walls.iter().enumerate() {
        let Some(wt) = sp.wall_types.iter().find(|t| !w.wall_type.is_empty() && t.name.eq_ignore_ascii_case(&w.wall_type)) else {
            untyped += 1;
            continue;
        };
        let Some(asm) = wt.assembly.as_ref().filter(|a| !a.layers.is_empty()) else {
            no_layers.insert(wt.name.clone());
            continue;
        };
        let jn = end_joins.get(i).copied().unwrap_or_default();
        // A corner's bead and tape go with the first of its two walls.
        let corner_mine = |end: usize| -> bool {
            let p = w.ends().get(end).copied().unwrap_or(w.o);
            !walls.iter().take(i).any(|o| o.level.eq_ignore_ascii_case(&w.level) && o.ends().iter().any(|q| q.dist(p) < (w.t + o.t)))
        };
        lay_wall(w, wt, asm, r, ctx, k, batch, want, jn, [corner_mine(0), corner_mine(1)]);
    }
    if untyped > 0 {
        batch.missing.insert("walls with no wall type".into(), format!("{untyped} walls: no type mark or legend type, so no layers to lay out"));
    }
    if !no_layers.is_empty() {
        let n = walls.iter().filter(|w| no_layers.iter().any(|t| t.eq_ignore_ascii_case(&w.wall_type))).count();
        batch.missing.insert(
            "wall types with no layers".into(),
            format!("{n} walls of {} : the partition legend gives these types no layers", no_layers.iter().cloned().collect::<Vec<_>>().join(", ")),
        );
    }
}

/// What a wall type is: its mark and legend, for every piece's source.
fn type_source(wt: &cadaddy_walk3d::WallType, asm: &Assembly) -> Source {
    let doc = if asm.source.is_empty() { format!("wall type {}", wt.name) } else { asm.source.clone() };
    Source::from_doc(SourceKind::Legend, &doc)
}

#[allow(clippy::too_many_arguments)]
fn lay_wall(
    w: &Wall,
    wt: &cadaddy_walk3d::WallType,
    asm: &Assembly,
    r: &Rules,
    ctx: &Ctx,
    k: f64,
    batch: &mut Batch,
    want: &dyn Fn(&str) -> bool,
    jn: [EndJoin; 2],
    corner_mine: [bool; 2],
) {
    let src = type_source(wt, asm);
    let placed = place_layers(asm, w.t, k);
    let upf = ctx.upf;
    let rating = asm.fire_rating.clone().unwrap_or_default();
    let base = |p: Piece| -> Piece {
        let mut p = p;
        p.host = Some(w.handle);
        p.extra.push(("wall_type", wt.name.clone(), src.clone()));
        if !rating.is_empty() {
            p.extra.push(("fire_rating", rating.clone(), src.clone()));
        }
        p
    };
    // ── Framing and masonry ──
    let mut studs: Vec<(f64, usize)> = Vec::new();
    let mut flange = 0.0;
    let mut plates_h = (0.0, 0.0);
    for pl in placed.iter().filter(|p| p.layer.category == "framing") {
        if !want("framing") && !want("insulation") {
            break;
        }
        let (s, f, ph) = frame(w, wt, asm, pl, r, k, upf, batch, &base, &src, want("framing"));
        if studs.is_empty() {
            studs = s;
            flange = f;
            plates_h = ph;
        }
    }
    if want("masonry") {
        for pl in placed.iter().filter(|p| p.layer.category == "masonry") {
            masonry(w, pl, r, k, upf, batch, &base, &src);
        }
    }
    // ── Insulation in the stud bays ──
    if want("insulation") {
        for pl in placed.iter().filter(|p| p.layer.category == "insulation") {
            if pl.layer.in_cavity && studs.len() >= 2 {
                batts(w, pl, r, k, upf, batch, &base, &src, &studs, flange, plates_h);
            } else if !pl.layer.in_cavity {
                let side = pl.layer.side;
                sheets(w, pl, r, k, upf, batch, &base, &src, "insulation", "board", side, 1, ctx, 0);
            }
        }
    }
    // ── Sheathing, furring, boards ──
    let mut face_area = [0.0f64; 2];
    let mut face_room = [String::new(), String::new()];
    for (si, side) in [LayerSide::Side1, LayerSide::Side2].into_iter().enumerate() {
        let sign = if side == LayerSide::Side1 { 1.0 } else { -1.0 };
        let probe = w.at((w.s0 + w.s1) / 2.0, sign * (w.t / 2.0 + 1.5 * upf));
        let room = ctx.room_at(&w.level, probe);
        if let Some(rm) = room {
            face_room[si] = Ctx::room_label(rm);
        }
        if want("framing") {
            for pl in face_layers(&placed, side, &["sheathing"]) {
                sheets(w, pl, r, k, upf, batch, &base, &src, "framing", "sheathing", side, 1, ctx, 0);
            }
            for pl in face_layers(&placed, side, &["furring"]) {
                furring(w, pl, r, k, upf, batch, &base, &src, side, &face_room[si]);
            }
        }
        if want("drywall") {
            let boards = face_layers(&placed, side, &["board"]);
            let n_boards = boards.len();
            for (li, pl) in boards.into_iter().enumerate() {
                let (area, _) =
                    sheets(w, pl, r, k, upf, batch, &base, &src, "drywall", "sheet", side, li + 1, ctx, if li == 0 { n_boards } else { 0 });
                if li == 0 {
                    face_area[si] = area;
                }
            }
        }
    }
    // ── Corner bead and inside-corner tape ──
    if want("drywall") {
        let height_ft = (w.zt - w.zf) / upf;
        let faces_with_board: Vec<usize> = [LayerSide::Side1, LayerSide::Side2]
            .iter()
            .enumerate()
            .filter(|(_, s)| !face_layers(&placed, **s, &["board"]).is_empty())
            .map(|(i, _)| i)
            .collect();
        if !faces_with_board.is_empty() {
            for (end, j) in jn.iter().enumerate() {
                let s = if end == 0 { w.s0 } else { w.s1 };
                let (beads, tapes) = match (j.corner, j.tee) {
                    (false, false) => (faces_with_board.len(), 0),
                    (true, _) if corner_mine[end] => (1, 1),
                    (true, _) => (0, 0),
                    (false, true) => (0, faces_with_board.len()),
                };
                for b in 0..beads {
                    let side_sign = if faces_with_board.get(b).copied().unwrap_or(0) == 0 { 1.0 } else { -1.0 };
                    let off = side_sign * w.t / 2.0;
                    let bk = 1.25 * k;
                    let p = base(Piece::new(
                        "drywall",
                        "corner_bead",
                        r.text("drywall.corner_bead_item"),
                        &w.level,
                        r.source("drywall.corner_bead_item"),
                    ));
                    let mut p = p.computed("at", if j.corner { "outside corner" } else { "free end" });
                    p.length_ft = height_ft;
                    p.room = face_room.get(faces_with_board.get(b).copied().unwrap_or(0)).cloned().unwrap_or_default();
                    p.blocks.push(w.block(s - bk / 2.0, s + bk / 2.0, off, bk, w.zf, w.zt));
                    batch.push(p);
                }
                if tapes > 0 {
                    let mut p = base(Piece::new("drywall", "tape", r.text("drywall.tape_item"), &w.level, r.source("drywall.tape_item")));
                    p = p.computed("at", if j.tee { "inside corners against the wall it stops at" } else { "inside corner" });
                    p.length_ft = height_ft * tapes as f64;
                    p.blocks.push(w.block(s - 1.0 * k, s + 1.0 * k, 0.0, w.t, w.zf, w.zt));
                    batch.push(p);
                }
            }
        }
    }
    // ── Paint on the faces that face a room ──
    if want("finishes") {
        let coats = r.num("finishes.paint_coats", 2.0).clamp(0.0, 6.0).round() as usize;
        for (si, side) in [LayerSide::Side1, LayerSide::Side2].into_iter().enumerate() {
            let area = face_area.get(si).copied().unwrap_or(0.0);
            let room = face_room.get(si).cloned().unwrap_or_default();
            if area <= 0.0 || room.is_empty() {
                continue;
            }
            let sign = if side == LayerSide::Side1 { 1.0 } else { -1.0 };
            for coat in 1..=coats {
                let mut p = base(Piece::new("finishes", "paint", r.text("finishes.paint_item"), &w.level, r.source("finishes.paint_coats")));
                p.ty = if coat == 1 && coats > 1 { "primer".into() } else { "finish coat".into() };
                p.area_sf = area;
                p.room = room.clone();
                p = p.computed("coat", coat.to_string()).computed("face", if si == 0 { "side 1" } else { "side 2" });
                let th = 0.02 * k;
                p.blocks.push(w.block(w.s0, w.s1, sign * (w.t / 2.0 + th * coat as f64), th, w.zf, w.zt));
                batch.push(p);
            }
        }
    }
    // ── Backing behind what hangs on the wall ──
    if want("framing") && flange > 0.0 {
        backing(w, r, ctx, k, upf, batch, &base, &studs, flange);
    }
}

/// The studs, plates or tracks, headers, sills and blocking of a framing layer; the studs'
/// positions (along the wall) and indices, the stud's width along the wall, the plates' heights
/// (bottom, top).
#[allow(clippy::too_many_arguments)]
fn frame(
    w: &Wall,
    wt: &cadaddy_walk3d::WallType,
    asm: &Assembly,
    pl: &Placed,
    r: &Rules,
    k: f64,
    upf: f64,
    batch: &mut Batch,
    base: &dyn Fn(Piece) -> Piece,
    src: &Source,
    emit: bool,
) -> (Vec<(f64, usize)>, f64, (f64, f64)) {
    let material = pl.layer.material.clone();
    let wood = is_wood(r, &material);
    let depth_in = layer_depth(pl.layer).max(1.5);
    // Spacing: the layer's, its text's, the project's for the wall type, else the rules'.
    let documented =
        pl.layer.spacing_in.filter(|v| *v > 0.0).or_else(|| text_spacing(&material)).or_else(|| asm.notes.iter().find_map(|n| text_spacing(n)));
    let (spacing_in, sp_src) = match (r.stud_by_type.get(&wt.name.to_ascii_uppercase()), documented) {
        // A person's rule for the wall type wins over what the legend says.
        (Some((v, why)), _) => (*v, Source::from_doc(SourceKind::Rule, why)),
        (None, Some(v)) => (v, src.clone()),
        (None, None) => (r.num("framing.stud_spacing_in", 16.0), r.source("framing.stud_spacing_in")),
    };
    let spacing_in = spacing_in.clamp(4.0, 48.0);
    let (gauge, g_src) = match text_gauge(&material) {
        Some(g) => (g, src.clone()),
        None if wood => (String::new(), src.clone()),
        None => (r.text("framing.steel_gauge"), r.source("framing.steel_gauge")),
    };
    let size = inches_text(depth_in);
    // A wood stud as its text names it (`2x6`), else by its depth.
    let named = {
        let up = material.to_ascii_uppercase();
        up.find("2X").and_then(|i| {
            let n: String = up.get(i + 2..).unwrap_or_default().chars().take_while(char::is_ascii_digit).collect();
            (!n.is_empty()).then(|| format!("2x{n}"))
        })
    };
    let (ty, item) = if wood {
        let l = named.clone().unwrap_or_else(|| lumber(depth_in));
        (format!("wood {l}"), format!("{l} wood stud"))
    } else {
        (format!("steel {gauge}"), format!("{size} {gauge} steel stud"))
    };
    let f = if wood { r.num("framing.wood_thickness_in", 1.5) } else { r.num("framing.steel_flange_in", 1.25) } * k;
    let plates = r.nums(if wood { "framing.wood_plates" } else { "framing.steel_plates" });
    let (n_bottom, n_top) = (
        plates.first().copied().unwrap_or(1.0).round().clamp(0.0, 4.0) as usize,
        plates.get(1).copied().unwrap_or(1.0).round().clamp(0.0, 4.0) as usize,
    );
    let plate_t = if wood { 1.5 * k } else { 0.0 };
    let (bottom_h, top_h) = (plate_t * n_bottom as f64, plate_t * n_top as f64);
    let short = if wood { 0.0 } else { r.num("framing.steel_stud_short_in", 0.5) * k };
    let (z0, z1) = (w.zf + bottom_h, w.zt - top_h - short);
    let stud_len_ft = ((z1 - z0) / upf).max(0.0);
    let mut out: Vec<(f64, usize)> = Vec::new();
    let mk_stud = |batch: &mut Batch, s: f64, za: f64, zb: f64, role: &str| -> usize {
        if !emit {
            return usize::MAX;
        }
        let mut p = base(Piece::new("framing", "stud", item.clone(), &w.level, src.clone()));
        p.ty = ty.clone();
        p.size = size.clone();
        p.length_ft = ((zb - za) / upf).max(0.0);
        p = p.computed("role", role).with("spacing_in", num_text(spacing_in), sp_src.clone());
        if !gauge.is_empty() {
            p = p.with("gauge", gauge.clone(), g_src.clone());
        }
        p.blocks.push(w.block(s - f / 2.0, s + f / 2.0, pl.off, pl.depth, za, zb));
        batch.push(p)
    };
    let sp = spacing_in * k;
    let in_hole = |s: f64| w.holes.iter().find(|h| s > h.s0 - f * 0.5 && s < h.s1 + f * 0.5);
    // Kings and jacks either side of each opening.
    let jambs = r.num("framing.jamb_studs", 1.0).round().clamp(0.0, 4.0) as usize;
    let jacks = if wood { r.num("framing.jack_studs", 1.0).round().clamp(0.0, 4.0) as usize } else { 0 };
    let mut fixed: Vec<f64> = Vec::new();
    for h in &w.holes {
        let headed = h.z1 < w.zt - 1e-6;
        for side in [-1.0f64, 1.0] {
            let edge = if side < 0.0 { h.s0 } else { h.s1 };
            for j in 0..jacks.min(usize::from(headed) * jacks) {
                let s = edge - side * (f * (j as f64 + 0.5));
                let i = mk_stud(batch, s, z0, h.z1, "jack");
                out.push((s, i));
                fixed.push(s);
            }
            for j in 0..jambs {
                let s = edge + side * (f * ((j + if headed { jacks } else { 0 }) as f64 + 0.5));
                if s < w.s0 || s > w.s1 {
                    continue;
                }
                let i = mk_stud(batch, s, z0, z1, "king");
                out.push((s, i));
                fixed.push(s);
            }
        }
    }
    // The layout from the wall's start, the last stud at its end.
    let mut s = w.s0 + f / 2.0;
    let mut n = 0usize;
    let mut layout: Vec<f64> = Vec::new();
    while s < w.s1 - f / 2.0 + 1e-6 && n < MAX_PER_WALL {
        layout.push(s);
        s += sp;
        n += 1;
    }
    if layout.last().is_none_or(|l| w.s1 - f / 2.0 - l > f) {
        layout.push(w.s1 - f / 2.0);
    }
    let min_cripple = r.num("framing.min_cripple_in", 4.0) * k;
    for s in layout {
        if fixed.iter().any(|x| (x - s).abs() < f) {
            continue;
        }
        match in_hole(s) {
            None => {
                let i = mk_stud(batch, s, z0, z1, "common");
                out.push((s, i));
            }
            Some(h) => {
                if h.z0 - z0 > min_cripple {
                    mk_stud(batch, s, z0, h.z0 - plate_t.max(f * 0.0), "cripple");
                }
                let top = h.z1 + if wood { header_depth(r, (h.s1 - h.s0) / upf) * k } else { 0.0 };
                if z1 - top > min_cripple {
                    mk_stud(batch, s, top, z1, "cripple");
                }
            }
        }
    }
    out.sort_by(|a, b| a.0.total_cmp(&b.0));
    if !emit {
        return (out, f, (bottom_h, top_h));
    }
    let stud_idx: Vec<usize> = out.iter().map(|x| x.1).collect();
    // Plates (wood) or tracks (steel): the bottom stops at door openings.
    let (kind, stock, pitem) = if wood {
        ("plate", r.num("framing.plate_stock_ft", 16.0), format!("{} wood plate", named.clone().unwrap_or_else(|| lumber(depth_in))))
    } else {
        ("track", r.num("framing.track_stock_ft", 10.0), format!("{size} {gauge} steel track"))
    };
    let doors: Vec<(f64, f64)> = w.holes.iter().filter(|h| h.z0 <= w.zf + 1e-6).map(|h| (h.s0, h.s1)).collect();
    let mut bottom_runs: Vec<(f64, f64)> = Vec::new();
    let mut at = w.s0;
    for (a, b) in geo::merge(doors) {
        if a > at {
            bottom_runs.push((at, a));
        }
        at = at.max(b);
    }
    if w.s1 > at {
        bottom_runs.push((at, w.s1));
    }
    let pt = if wood { 1.5 * k } else { 0.5 * k };
    let add_run = |batch: &mut Batch, a: f64, b: f64, z: f64, role: &str| {
        let mut sa = a;
        for (len, cut) in geo::stock_cuts((b - a) / upf, stock) {
            let sb = (sa + len * upf).min(b);
            let mut p = base(Piece::new("framing", kind, pitem.clone(), &w.level, src.clone()));
            p.ty = ty.clone();
            p.size = size.clone();
            p.length_ft = len;
            p = p.computed("role", role).computed("cut", if cut { "true" } else { "false" }).with(
                "stock_ft",
                num_text(stock),
                r.source(if wood { "framing.plate_stock_ft" } else { "framing.track_stock_ft" }),
            );
            p.blocks.push(w.block(sa, sb, pl.off, pl.depth, z, z + pt));
            // A plate's studs stand on it.
            let mine: Vec<usize> = out.iter().filter(|(s, i)| *s >= sa - f && *s <= sb + f && *i != usize::MAX).map(|(_, i)| *i).take(48).collect();
            let pi = batch.push(p);
            for si in mine {
                batch.link(pi, si);
            }
            sa = sb;
        }
    };
    for n in 0..n_bottom.max(usize::from(!wood)) {
        for (a, b) in &bottom_runs {
            add_run(batch, *a, *b, w.zf + pt * n as f64, "bottom");
        }
    }
    for n in 0..n_top.max(usize::from(!wood)) {
        add_run(batch, w.s0, w.s1, w.zt - pt * (n as f64 + 1.0), "top");
    }
    // Headers over the openings and sills under windows.
    for h in &w.holes {
        let width_ft = (h.s1 - h.s0) / upf;
        if h.z1 < w.zt - 1e-6 {
            let (item, depth, bearing) = if wood {
                let name = header_name(r, width_ft);
                let plies = r.num("framing.header_plies", 2.0).round().clamp(1.0, 4.0);
                (format!("{} header, {} plies", name, plies), header_depth(r, width_ft) * k, r.num("framing.header_bearing_in", 3.0) * k)
            } else {
                (format!("{size} {gauge} track header"), 1.25 * k, r.num("framing.steel_header_bearing_in", 6.0) * k)
            };
            let mut p = base(Piece::new(
                "framing",
                "header",
                item,
                &w.level,
                r.source(if wood { "framing.wood_header_by_span" } else { "framing.header_bearing_in" }),
            ));
            p.ty = ty.clone();
            p.size = size.clone();
            p.length_ft = (h.s1 - h.s0 + 2.0 * bearing) / upf;
            p = p.computed("opening_ft", num_text(width_ft));
            p.blocks.push(w.block(h.s0 - bearing, h.s1 + bearing, pl.off, pl.depth, h.z1, (h.z1 + depth).min(w.zt)));
            let hi = batch.push(p);
            for (s, i) in &out {
                if (*s - h.s0).abs() < f * 4.0 || (*s - h.s1).abs() < f * 4.0 {
                    batch.link(hi, *i);
                }
            }
        }
        if h.z0 > w.zf + 1e-6 {
            let mut p = base(Piece::new("framing", if wood { "plate" } else { "track" }, pitem.clone(), &w.level, src.clone()));
            p.ty = ty.clone();
            p.size = size.clone();
            p.length_ft = width_ft;
            p = p.computed("role", "sill");
            p.blocks.push(w.block(h.s0, h.s1, pl.off, pl.depth, h.z0 - pt, h.z0));
            batch.push(p);
        }
    }
    // Fire blocking in tall wood walls.
    let every = r.num("framing.fire_block_over_ft", 10.0) * upf;
    if wood && every > 0.0 && w.zt - w.zf > every {
        let mut z = w.zf + every;
        while z < w.zt - 1e-6 {
            for pair in out.windows(2).take(MAX_PER_WALL) {
                let [(a, ia), (b, ib)] = pair else { continue };
                if b - a <= f * 1.01 || in_hole((a + b) / 2.0).is_some() {
                    continue;
                }
                let mut p = base(Piece::new(
                    "framing",
                    "blocking",
                    format!("{} fire blocking", named.clone().unwrap_or_else(|| lumber(depth_in))),
                    &w.level,
                    r.source("framing.fire_block_over_ft"),
                ));
                p.ty = ty.clone();
                p.size = size.clone();
                p.length_ft = (b - a - f) / upf;
                p.blocks.push(w.block(a + f / 2.0, b - f / 2.0, pl.off, pl.depth, z - 0.75 * k, z + 0.75 * k));
                let bi = batch.push(p);
                batch.link(bi, *ia);
                batch.link(bi, *ib);
            }
            z += every;
        }
    }
    let _ = stud_idx;
    let _ = stud_len_ft;
    (out, f, (bottom_h, top_h))
}

/// A wood header's name by the opening's width (feet), from the rules' table.
fn header_name(r: &Rules, width_ft: f64) -> String {
    for row in r.list("framing.wood_header_by_span") {
        let Some(a) = row.as_array() else { continue };
        if let (Some(span), Some(name)) = (a.first().and_then(Value::as_f64), a.get(1).and_then(Value::as_str))
            && width_ft <= span + 1e-6
        {
            return name.to_string();
        }
    }
    "header".into()
}

/// A wood header's depth (inches) by the opening's width: its lumber's actual depth.
fn header_depth(r: &Rules, width_ft: f64) -> f64 {
    let name = header_name(r, width_ft);
    match name.split('x').nth(1).and_then(|n| n.split_whitespace().next()).and_then(|n| n.parse::<u32>().ok()) {
        Some(n) if n >= 8 => f64::from(n) - 0.75,
        Some(n) => f64::from(n) - 0.5,
        None => 11.875,
    }
}

/// A masonry layer's courses: each course a piece of its blocks.
#[allow(clippy::too_many_arguments)]
fn masonry(w: &Wall, pl: &Placed, r: &Rules, k: f64, upf: f64, batch: &mut Batch, base: &dyn Fn(Piece) -> Piece, src: &Source) {
    let unit = r.nums("masonry.unit_in");
    let (ul, uh) = (unit.first().copied().unwrap_or(16.0) * k, unit.get(1).copied().unwrap_or(8.0) * k);
    let nominal = (layer_depth(pl.layer) + 0.375).round().max(4.0);
    let item = format!("{}\" {}", nominal, r.text("masonry.item"));
    let mut z = w.zf;
    let mut n = 0usize;
    while z < w.zt - 1e-6 && n < MAX_PER_WALL {
        let zb = (z + uh).min(w.zt);
        let open: f64 =
            w.holes.iter().filter(|h| h.z0 < zb - 1e-6 && h.z1 > z + 1e-6).map(|h| h.s1.min(w.s1) - h.s0.max(w.s0)).filter(|v| *v > 0.0).sum();
        let len = (w.len() - open).max(0.0);
        if len > 1e-6 {
            let mut p = base(Piece::new("masonry", "cmu", item.clone(), &w.level, src.clone()));
            p.size = format!("{nominal}\"");
            p.ty = pl.layer.material.clone();
            p.length_ft = len / upf;
            p.qty = (len / ul).ceil().max(1.0);
            p.area_sf = len * (zb - z) / (upf * upf);
            p = p.computed("course", (n + 1).to_string()).with(
                "unit",
                format!("{}x{}", num_text(ul / k), num_text(uh / k)),
                r.source("masonry.unit_in"),
            );
            p.blocks.push(w.block(w.s0, w.s1, pl.off, pl.depth, z, zb));
            batch.push(p);
        }
        z = zb;
        n += 1;
    }
}

/// A batt in each bay between the studs.
#[allow(clippy::too_many_arguments)]
fn batts(
    w: &Wall,
    pl: &Placed,
    r: &Rules,
    k: f64,
    upf: f64,
    batch: &mut Batch,
    base: &dyn Fn(Piece) -> Piece,
    src: &Source,
    studs: &[(f64, usize)],
    f: f64,
    plates: (f64, f64),
) {
    let depth_in = pl.layer.thickness_in.max(0.0);
    let (rv, r_src) = r_value(r, &pl.layer.material, depth_in);
    let item = if rv.is_empty() { pl.layer.material.clone() } else { format!("{}, {rv}", pl.layer.material) };
    let (z0, z1) = (w.zf + plates.0, w.zt - plates.1);
    for pair in studs.windows(2).take(MAX_PER_WALL) {
        let [(a, ia), (b, ib)] = pair else { continue };
        let (sa, sb) = (a + f / 2.0, b - f / 2.0);
        if sb - sa < 1.0 * k {
            continue;
        }
        let gross = (sb - sa) * (z1 - z0);
        let open: f64 = w.holes.iter().map(|h| h.overlap(sa, sb, z0, z1)).sum();
        let net = (gross - open) / (upf * upf);
        if net < 0.5 {
            continue;
        }
        let mut p = base(Piece::new("insulation", "batt", item.clone(), &w.level, src.clone()));
        p.ty = rv.clone();
        p.size = inches_text(depth_in);
        p.area_sf = net;
        p = p.with("r_value", rv.clone(), r_src.clone()).computed("bay_in", num_text((sb - sa) / k));
        p.blocks.push(w.block(sa, sb, pl.off, pl.depth * 0.98, z0, z1));
        let bi = batch.push(p);
        batch.link(bi, *ia);
        batch.link(bi, *ib);
    }
}

/// Rectangles of `[s0, s1] x [z0, z1]` left when the holes are taken out (at most 16).
fn minus_holes(s0: f64, s1: f64, z0: f64, z1: f64, holes: &[Hole]) -> Vec<(f64, f64, f64, f64)> {
    let mut rects = vec![(s0, s1, z0, z1)];
    for h in holes {
        let mut next = Vec::new();
        for (a, b, c, e) in rects {
            if h.s1 <= a || h.s0 >= b || h.z1 <= c || h.z0 >= e {
                next.push((a, b, c, e));
                continue;
            }
            let (hs0, hs1, hz0, hz1) = (h.s0.max(a), h.s1.min(b), h.z0.max(c), h.z1.min(e));
            if hs0 > a {
                next.push((a, hs0, c, e));
            }
            if hs1 < b {
                next.push((hs1, b, c, e));
            }
            if hz0 > c {
                next.push((hs0, hs1, c, hz0));
            }
            if hz1 < e {
                next.push((hs0, hs1, hz1, e));
            }
        }
        next.truncate(16);
        rects = next;
    }
    rects
}

/// Sheets (board, sheathing, rigid insulation) over a face for one layer: the net area of the
/// face it covers and the joints (feet of tape) its layout makes. `finish`: for the face's outer
/// layer, the face's number of board layers: it takes the face's tape and compound, and the
/// screws of all its layers.
#[allow(clippy::too_many_arguments)]
fn sheets(
    w: &Wall,
    pl: &Placed,
    r: &Rules,
    k: f64,
    upf: f64,
    batch: &mut Batch,
    base: &dyn Fn(Piece) -> Piece,
    src: &Source,
    trade: &'static str,
    kind: &'static str,
    side: LayerSide,
    layer_no: usize,
    ctx: &Ctx,
    finish: usize,
) -> (f64, f64) {
    let material = pl.layer.material.clone();
    let th_in = layer_depth(pl.layer);
    let (width_ft, lengths, size_src) = if trade == "drywall" {
        (r.num("drywall.sheet_width_ft", 4.0), r.nums("drywall.sheet_lengths_ft"), r.source("drywall.sheet_lengths_ft"))
    } else {
        let p = r.nums(if trade == "insulation" { "insulation.board_sheet_ft" } else { "framing.panel_ft" });
        (
            p.first().copied().unwrap_or(4.0),
            vec![p.get(1).copied().unwrap_or(8.0)],
            r.source(if trade == "insulation" { "insulation.board_sheet_ft" } else { "framing.panel_ft" }),
        )
    };
    let vertical = r.text("drywall.orientation") != "horizontal" || trade != "drywall";
    let ty = if trade == "drywall" { board_type(r, &material) } else { String::new() };
    let face = if side == LayerSide::Side1 { "side 1" } else { "side 2" };
    let probe_sign = if side == LayerSide::Side1 { 1.0 } else { -1.0 };
    let room = ctx.room_at(&w.level, w.at((w.s0 + w.s1) / 2.0, probe_sign * (w.t / 2.0 + 1.5 * upf))).map(Ctx::room_label).unwrap_or_default();
    let (wid, height) = (width_ft * upf, w.zt - w.zf);
    let max_len = lengths.iter().copied().fold(0.0f64, f64::max).max(1.0) * upf;
    // The sheet rectangles: columns of sheets along the wall (vertical), or rows of sheets up it.
    let mut rects: Vec<(f64, f64, f64, f64, f64, f64)> = Vec::new(); // s0, s1, z0, z1, stock w, stock l
    if vertical {
        let rows: Vec<(f64, f64)> = {
            let mut v = Vec::new();
            let mut z = w.zf;
            while z < w.zt - 1e-6 && v.len() < 16 {
                let need = (w.zt - z) / upf;
                let l = geo::stock_for(need, &lengths).unwrap_or(need) * upf;
                let l = l.min(max_len).max(0.5 * upf);
                v.push((z, l));
                z += l;
            }
            v
        };
        let mut s = w.s0;
        let mut n = 0;
        while s < w.s1 - 1e-6 && n < MAX_PER_WALL {
            let sb = (s + wid).min(w.s1);
            for (z, l) in &rows {
                rects.push((s, sb, *z, (*z + l).min(w.zt), wid, *l));
            }
            s = sb;
            n += 1;
        }
    } else {
        let mut z = w.zf;
        let mut n = 0;
        while z < w.zt - 1e-6 && n < MAX_PER_WALL {
            let zb = (z + wid).min(w.zt);
            let mut s = w.s0;
            while s < w.s1 - 1e-6 && n < MAX_PER_WALL {
                let need = (w.s1 - s) / upf;
                let l = geo::stock_for(need, &lengths).unwrap_or(need).min(max_len / upf) * upf;
                let sb = (s + l).min(w.s1);
                rects.push((s, sb, z, zb, l, wid));
                s = sb;
                n += 1;
            }
            z = zb;
        }
    }
    let mut net_total = 0.0;
    let mut joints = 0.0;
    let mut sheet_idx: Vec<usize> = Vec::new();
    let item_of = |sw: f64, sl: f64| format!("{material}, {}x{}", num_text((sw / upf).min(sl / upf)), num_text((sw / upf).max(sl / upf)));
    for (a, b, c, e, sw, sl) in &rects {
        let gross = (b - a) * (e - c);
        let open: f64 = w.holes.iter().map(|h| h.overlap(*a, *b, *c, *e)).sum();
        if gross <= 1e-9 || open >= gross * 0.98 {
            continue;
        }
        let net = (gross - open) / (upf * upf);
        net_total += net;
        let cut = (b - a) < *sw - 1e-6 || (e - c) < *sl - 1e-6 || open > 0.0;
        let mut p = base(Piece::new(trade, kind, item_of(*sw, *sl), &w.level, src.clone()));
        p.ty = ty.clone();
        p.size = inches_text(th_in);
        p.area_sf = net;
        p.room = room.clone();
        p = p
            .with("sheet_ft", format!("{}x{}", num_text(sw / upf), num_text(sl / upf)), size_src.clone())
            .computed("cut", if cut { "true" } else { "false" })
            .computed("face", face)
            .computed("layer", layer_no.to_string());
        if open > 0.0 {
            p = p.computed("opening_cut_sf", num_text(open / (upf * upf)));
        }
        for (ra, rb, rc, re) in minus_holes(*a, *b, *c, *e, &w.holes) {
            p.blocks.push(w.block(ra, rb, pl.off, pl.depth, rc, re));
        }
        sheet_idx.push(batch.push(p));
    }
    if finish == 0 || trade != "drywall" || net_total <= 0.0 {
        return (net_total, 0.0);
    }
    // The joints its layout makes on the face: between columns (vertical) or rows, less what
    // falls in the openings.
    if vertical {
        let mut s = w.s0 + wid;
        while s < w.s1 - 1e-6 {
            let open: f64 = w.holes.iter().filter(|h| s > h.s0 && s < h.s1).map(|h| h.z1 - h.z0).sum();
            joints += (height - open).max(0.0);
            s += wid;
        }
        let mut z = w.zf;
        let mut seams = Vec::new();
        while z < w.zt - 1e-6 && seams.len() < 16 {
            let need = (w.zt - z) / upf;
            let l = geo::stock_for(need, &lengths).unwrap_or(need) * upf;
            z += l.min(max_len).max(0.5 * upf);
            if z < w.zt - 1e-6 {
                seams.push(z);
            }
        }
        for z in seams {
            let open: f64 = w.holes.iter().filter(|h| z > h.z0 && z < h.z1).map(|h| h.s1 - h.s0).sum();
            joints += (w.len() - open).max(0.0);
        }
    } else {
        let mut z = w.zf + wid;
        while z < w.zt - 1e-6 {
            let open: f64 = w.holes.iter().filter(|h| z > h.z0 && z < h.z1).map(|h| h.s1 - h.s0).sum();
            joints += (w.len() - open).max(0.0);
            z += wid;
        }
    }
    // Around each opening the board's cut edges are taped too.
    for h in &w.holes {
        joints += 2.0 * (h.z1 - h.z0) + if h.z1 < w.zt - 1e-6 { h.s1 - h.s0 } else { 0.0 } + if h.z0 > w.zf + 1e-6 { h.s1 - h.s0 } else { 0.0 };
    }
    let sign = if side == LayerSide::Side1 { 1.0 } else { -1.0 };
    let marker = |s: f64| w.block(s - 1.0 * k, s + 1.0 * k, sign * (w.t / 2.0 + 0.5 * k), 1.0 * k, w.zf + 4.0 * k, w.zf + 6.0 * k);
    let mid = (w.s0 + w.s1) / 2.0;
    let mut tape = base(Piece::new("drywall", "tape", r.text("drywall.tape_item"), &w.level, r.source("drywall.tape_item")));
    tape.length_ft = joints / upf;
    tape.room = room.clone();
    tape = tape.computed("face", face).computed("at", "the sheets' joints and cut edges");
    tape.blocks.push(marker(mid - 3.0 * k));
    let ti = batch.push(tape);
    let mut mud = base(Piece::new("drywall", "compound", r.text("drywall.compound_item"), &w.level, r.source("drywall.compound_lb_per_sf")));
    mud.weight_lb = net_total * r.num("drywall.compound_lb_per_sf", 0.14);
    mud.extra.push(("finished_sf", num_text(net_total), Source::computed()));
    mud.room = room.clone();
    mud = mud.computed("face", face).with("finish_level", r.text("drywall.finish_level"), r.source("drywall.finish_level"));
    mud.blocks.push(marker(mid));
    let mi = batch.push(mud);
    let mut screws = base(Piece::new("drywall", "screws", r.text("drywall.screw_item"), &w.level, r.source("drywall.screws_per_sf")));
    // Every layer is screwed off: the face's layers' area at the rule's rate.
    let layers_here = finish as f64;
    screws.qty = (net_total * layers_here * r.num("drywall.screws_per_sf", 1.0)).round().max(1.0);
    screws.room = room.clone();
    screws = screws.computed("face", face);
    screws.blocks.push(marker(mid + 3.0 * k));
    let si = batch.push(screws);
    for x in [ti, mi, si] {
        for &sh in sheet_idx.iter().take(8) {
            batch.link(x, sh);
        }
    }
    (net_total, joints / upf)
}

/// Furring rows (resilient channel, hat channel) across a face at their spacing.
#[allow(clippy::too_many_arguments)]
fn furring(
    w: &Wall,
    pl: &Placed,
    r: &Rules,
    k: f64,
    upf: f64,
    batch: &mut Batch,
    base: &dyn Fn(Piece) -> Piece,
    src: &Source,
    side: LayerSide,
    room: &str,
) {
    let (spacing, sp_src) = match pl.layer.spacing_in.or_else(|| text_spacing(&pl.layer.material)) {
        Some(v) => (v, src.clone()),
        None => (24.0, Source::from_doc(SourceKind::Default, "company default: furring rows 24\" o.c. (GA-600 resilient channel)")),
    };
    let step = spacing.clamp(8.0, 48.0) * k;
    let stock = r.num("framing.track_stock_ft", 10.0).max(12.0);
    let mut z = w.zf + 6.0 * k;
    let mut n = 0;
    while z < w.zt - 1e-6 && n < 200 {
        let open: Vec<(f64, f64)> = w.holes.iter().filter(|h| z > h.z0 && z < h.z1).map(|h| (h.s0, h.s1)).collect();
        let mut at = w.s0;
        let mut runs = Vec::new();
        for (a, b) in geo::merge(open) {
            if a > at {
                runs.push((at, a));
            }
            at = at.max(b);
        }
        if w.s1 > at {
            runs.push((at, w.s1));
        }
        for (a, b) in runs {
            let mut sa = a;
            for (len, cut) in geo::stock_cuts((b - a) / upf, stock) {
                let sb = (sa + len * upf).min(b);
                let mut p = base(Piece::new("framing", "furring", pl.layer.material.clone(), &w.level, src.clone()));
                p.size = inches_text(pl.layer.thickness_in);
                p.length_ft = len;
                p.room = room.to_string();
                p = p
                    .with("spacing_in", num_text(spacing), sp_src.clone())
                    .computed("cut", if cut { "true" } else { "false" })
                    .computed("face", if side == LayerSide::Side1 { "side 1" } else { "side 2" });
                p.blocks.push(w.block(sa, sb, pl.off, pl.depth, z - 0.75 * k, z + 0.75 * k));
                batch.push(p);
                sa = sb;
            }
        }
        z += step;
        n += 1;
    }
}

/// Backing between the studs behind each device, fixture or casework that needs it.
#[allow(clippy::too_many_arguments)]
fn backing(w: &Wall, r: &Rules, ctx: &Ctx, k: f64, upf: f64, batch: &mut Batch, base: &dyn Fn(Piece) -> Piece, studs: &[(f64, usize)], f: f64) {
    let rules = r.list("framing.backing");
    for (level, kind, c, h) in &ctx.devices {
        if !level.eq_ignore_ascii_case(&w.level) {
            continue;
        }
        let p2 = Vec2::new(c.x, c.y);
        let s = (p2 - w.o).dot(w.u);
        let off = (p2 - w.o).dot(w.n);
        if s < w.s0 || s > w.s1 || off.abs() > w.t / 2.0 + 8.0 * k {
            continue;
        }
        let Some(rule) = rules.iter().find(|x| {
            x.get("kinds").and_then(Value::as_array).is_some_and(|a| a.iter().filter_map(Value::as_str).any(|kk| kk.eq_ignore_ascii_case(kind)))
        }) else {
            continue;
        };
        let item = rule.get("item").and_then(Value::as_str).unwrap_or("backing").to_string();
        let src = Source::from_doc(SourceKind::Default, rule.get("source").and_then(Value::as_str).unwrap_or("company default"));
        let width = rule.get("width_in").and_then(Value::as_f64).filter(|v| *v > 0.0).unwrap_or(24.0) * k;
        let height = rule.get("height_in").and_then(Value::as_f64).filter(|v| *v > 0.0).unwrap_or(6.0) * k;
        let zc = rule.get("at_aff_in").and_then(Value::as_f64).map_or(c.z, |a| w.zf + a * k);
        // Between the studs round it.
        let left = studs.iter().rev().find(|(x, _)| *x <= s - width / 2.0 + f).or_else(|| studs.iter().rev().find(|(x, _)| *x <= s));
        let right = studs.iter().find(|(x, _)| *x >= s + width / 2.0 - f).or_else(|| studs.iter().find(|(x, _)| *x >= s));
        let (a, b) = (left.map_or(s - width / 2.0, |x| x.0 + f / 2.0), right.map_or(s + width / 2.0, |x| x.0 - f / 2.0));
        if b - a < 1.0 * k {
            continue;
        }
        let side = if off >= 0.0 { 1.0 } else { -1.0 };
        let mut p = base(Piece::new("framing", "backing", item, &w.level, src));
        p.length_ft = (b - a) / upf;
        p.room = ctx.room_at(&w.level, w.at(s, side * (w.t / 2.0 + 1.5 * upf))).map(Ctx::room_label).unwrap_or_default();
        p.size = format!("{}x{}", num_text((b - a) / k), num_text(height / k));
        p = p.computed("behind", kind.clone());
        p.touches.push(*h);
        p.blocks.push(w.block(a, b, side * w.t / 4.0, w.t / 2.0 - 0.75 * k, zc - height / 2.0, zc + height / 2.0));
        let bi = batch.push(p);
        for (_, si) in [left, right].into_iter().flatten() {
            batch.link(bi, *si);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_walls_boxes_give_its_openings() {
        // A 20' wall 10' high with a 3' door at 5' and a 4' window (sill 3', head 7') at 12'.
        let mut m = super::super::super::wallgen::empty_mesh();
        let k = 12.0;
        let at = |s: f64| Vec2::new(s * k, 0.0);
        let add = |m: &mut Mesh, s0: f64, s1: f64, z0: f64, z1: f64| super::super::super::wallgen::add_box(m, at(s0), at(s1), 6.0, z0 * k, z1 * k);
        add(&mut m, 0.0, 5.0, 0.0, 10.0);
        add(&mut m, 5.0, 8.0, 7.0, 10.0);
        add(&mut m, 8.0, 12.0, 0.0, 10.0);
        add(&mut m, 12.0, 16.0, 0.0, 3.0);
        add(&mut m, 12.0, 16.0, 7.0, 10.0);
        add(&mut m, 16.0, 20.0, 0.0, 10.0);
        let bx = boxes(&m).unwrap();
        assert_eq!(bx.len(), 6);
        let rects: Vec<(f64, f64, f64, f64)> = bx.iter().map(|b| (b.a.x.min(b.b.x), b.a.x.max(b.b.x), b.z0, b.z1)).collect();
        let holes = holes_of(&rects, 0.0, 240.0, 0.0, 120.0, 6.0);
        assert_eq!(holes.len(), 2, "{holes:?}");
        assert_eq!(holes[0], Hole { s0: 60.0, s1: 96.0, z0: 0.0, z1: 84.0 });
        assert_eq!(holes[1], Hole { s0: 144.0, s1: 192.0, z0: 36.0, z1: 84.0 });
    }

    #[test]
    fn material_texts_read() {
        assert_eq!(text_inches("3 1/2\" metal studs"), Some(3.5));
        assert_eq!(text_inches("5/8\" Type X gypsum board"), Some(0.625));
        assert_eq!(text_inches("3-5/8\" studs"), Some(3.625));
        assert_eq!(text_inches("no size"), None);
        assert_eq!(text_spacing("3 5/8\" 20 ga metal studs @ 16\" o.c."), Some(16.0));
        assert_eq!(text_spacing("7/8\" furring channels 24\" OC"), Some(24.0));
        assert_eq!(text_gauge("3 5/8\" 20 ga metal studs"), Some("20 ga".into()));
        assert_eq!(text_gauge("metal studs"), None);
        let r = Rules::default();
        assert!(is_wood(&r, "5 1/2\" wood studs"));
        assert!(!is_wood(&r, "3 5/8\" 20 ga metal studs"));
        assert_eq!(board_type(&r, "5/8\" Type X gypsum board"), "Type X");
        assert_eq!(board_type(&r, "5/8\" WATER RESISTANT TYPE-X GYP. BD."), "Moisture resistant");
        assert_eq!(board_type(&r, "1/2\" gypsum board"), "Regular");
        assert_eq!(r_value(&r, "sound batt insulation", 3.5).0, "R-13");
        assert_eq!(r_value(&r, "R-19 batt", 3.5).0, "R-19");
        assert_eq!(lumber(5.5), "2x6");
        assert_eq!(header_name(&r, 3.0), "2x8");
        assert_eq!(header_depth(&r, 5.0), 9.25);
    }

    /// A wall 20' long and 10' high along x (inch drawing units), a 3'x7' door from 5' to 8'.
    fn wall(len_ft: f64, holes: Vec<Hole>) -> Wall {
        Wall {
            handle: Handle(0x42),
            level: "L1".into(),
            wall_type: "P9".into(),
            o: Vec2::new(0.0, 0.0),
            u: Vec2::new(1.0, 0.0),
            n: Vec2::new(0.0, 1.0),
            s0: 0.0,
            s1: len_ft * 12.0,
            t: 4.875,
            zf: 0.0,
            zt: 120.0,
            holes,
        }
    }

    fn layer(material: &str, th: f64, side: LayerSide, category: &str, cavity: bool, spacing: Option<f64>) -> AssemblyLayer {
        AssemblyLayer { material: material.into(), thickness_in: th, side, category: category.into(), in_cavity: cavity, spacing_in: spacing }
    }

    fn lay(w: &Wall, asm: Assembly, r: &Rules) -> Vec<Piece> {
        let wt = cadaddy_walk3d::WallType {
            name: "P9".into(),
            height_ft: None,
            thickness_in: 4.875,
            from: cadaddy_walk3d::WallFrom::Pairs,
            assembly: Some(asm.clone()),
        };
        let ctx = Ctx { upf: 12.0, rooms: Vec::new(), devices: Vec::new() };
        let mut batch = Batch::default();
        let all = |_: &str| true;
        lay_wall(w, &wt, &asm, r, &ctx, 1.0, &mut batch, &all, [EndJoin::default(); 2], [true, true]);
        batch.pieces
    }

    fn count(p: &[Piece], kind: &str) -> usize {
        p.iter().filter(|x| x.kind == kind).count()
    }

    fn role(p: &[Piece], kind: &str, role: &str) -> usize {
        p.iter().filter(|x| x.kind == kind && x.extra.iter().any(|(k, v, _)| *k == "role" && v == role)).count()
    }

    #[test]
    fn a_steel_partition_with_a_door_is_laid_out_piece_by_piece() {
        let asm = Assembly {
            tag: "P9".into(),
            source: "sample legend, mark P9".into(),
            fire_rating: Some("1 HR".into()),
            layers: vec![
                layer("5/8\" Type X gypsum board", 0.625, LayerSide::Side1, "board", false, None),
                layer("3 5/8\" 20 ga metal studs", 3.625, LayerSide::Core, "framing", false, Some(16.0)),
                layer("sound batt insulation", 3.5, LayerSide::Core, "insulation", true, None),
                layer("5/8\" Type X gypsum board", 0.625, LayerSide::Side2, "board", false, None),
            ],
            ..Assembly::default()
        };
        let door = Hole { s0: 60.0, s1: 96.0, z0: 0.0, z1: 84.0 };
        let p = lay(&wall(20.0, vec![door]), asm, &Rules::default());
        // Studs: 16 layout positions at 16" from the start and one at the end; two fall in the
        // door (cripples over it), one is the right king; two kings.
        assert_eq!(role(&p, "stud", "common"), 13);
        assert_eq!(role(&p, "stud", "king"), 2);
        assert_eq!(role(&p, "stud", "cripple"), 2);
        assert_eq!(count(&p, "stud"), 17);
        let common = p.iter().find(|x| x.kind == "stud").unwrap();
        assert_eq!(common.item, "3 5/8\" 20 ga steel stud");
        assert_eq!(common.source.doc.as_deref(), Some("sample legend, mark P9"));
        assert!((common.length_ft - 119.5 / 12.0).abs() < 1e-9 || common.extra.iter().any(|(k, v, _)| *k == "role" && v != "common"));
        // Tracks: the bottom stops at the door (5' + 12' = 1 + 2 pieces of 10' stock), the top
        // runs through (2 pieces).
        let tracks: Vec<&Piece> = p.iter().filter(|x| x.kind == "track").collect();
        assert_eq!(tracks.len(), 5);
        let bottom: f64 = tracks.iter().filter(|x| x.extra.iter().any(|(k, v, _)| *k == "role" && v == "bottom")).map(|x| x.length_ft).sum();
        let top: f64 = tracks.iter().filter(|x| x.extra.iter().any(|(k, v, _)| *k == "role" && v == "top")).map(|x| x.length_ft).sum();
        assert!((bottom - 17.0).abs() < 1e-9 && (top - 20.0).abs() < 1e-9, "{bottom} {top}");
        assert!(tracks.iter().all(|t| t.length_ft <= 10.0 + 1e-9));
        // One header over the door, lapping 6" each side.
        assert_eq!(count(&p, "header"), 1);
        assert!((p.iter().find(|x| x.kind == "header").unwrap().length_ft - 4.0).abs() < 1e-9);
        // Sheets: five 4x10 columns a face; the one over the door is cut round it.
        let sheets: Vec<&Piece> = p.iter().filter(|x| x.kind == "sheet").collect();
        assert_eq!(sheets.len(), 10);
        assert!(sheets.iter().all(|s| s.ty == "Type X" && s.size == "5/8\""));
        let area: f64 = sheets.iter().map(|s| s.area_sf).sum();
        assert!((area - 2.0 * (200.0 - 21.0)).abs() < 1e-6, "{area}");
        assert_eq!(sheets.iter().filter(|s| s.extra.iter().any(|(k, v, _)| *k == "cut" && v == "true")).count(), 2);
        // A batt in each of the 14 bays (the one over the door is the 3' x 3' above it).
        assert_eq!(count(&p, "batt"), 14);
        assert!(p.iter().filter(|x| x.kind == "batt").all(|b| b.ty == "R-13"));
        // Each face's tape: four 10' joints and the door's cut edges (7' + 7' + 3').
        let tape: Vec<f64> = p.iter().filter(|x| x.kind == "tape").map(|x| x.length_ft).collect();
        assert_eq!(tape.len(), 2);
        assert!(tape.iter().all(|t| (t - 57.0).abs() < 1e-6), "{tape:?}");
        // Free ends: a bead on each face at each end.
        assert_eq!(count(&p, "corner_bead"), 4);
        assert_eq!(p.iter().filter(|x| x.kind == "screws").map(|x| x.qty).sum::<f64>(), 2.0 * 179.0);
        assert!((p.iter().filter(|x| x.kind == "compound").map(|x| x.weight_lb).sum::<f64>() - 2.0 * 179.0 * 0.14).abs() < 1e-6);
        // No room beside it: no paint.
        assert_eq!(count(&p, "paint"), 0);
        // Every piece is the wall's and cites its type.
        assert!(p.iter().all(|x| x.host == Some(Handle(0x42))));
        assert!(p.iter().filter(|x| x.trade != "firestopping").all(|x| x.extra.iter().any(|(k, v, _)| *k == "wall_type" && v == "P9")));
    }

    #[test]
    fn a_wood_wall_takes_its_texts_spacing_double_top_plate_and_sheathing() {
        let asm = Assembly {
            source: "sample legend, mark W2".into(),
            layers: vec![
                layer("1/2\" wood structural panel sheathing", 0.5, LayerSide::Side1, "sheathing", false, None),
                layer("2x6 wood studs @ 24\" o.c.", 5.5, LayerSide::Core, "framing", false, None),
                layer("R-21 batt", 5.5, LayerSide::Core, "insulation", true, None),
                layer("1/2\" moisture resistant gypsum board", 0.5, LayerSide::Side2, "board", false, None),
                layer("1/2\" moisture resistant gypsum board", 0.5, LayerSide::Side2, "board", false, None),
            ],
            ..Assembly::default()
        };
        let p = lay(&wall(12.0, Vec::new()), asm, &Rules::default());
        // 24" o.c. over 12': positions 0.75 + 24k below 143.25, and the end stud: 7.
        assert_eq!(role(&p, "stud", "common"), 7);
        let stud = p.iter().find(|x| x.kind == "stud").unwrap();
        assert_eq!(stud.item, "2x6 wood stud");
        assert!((stud.length_ft - (120.0 - 4.5) / 12.0).abs() < 1e-9, "{}", stud.length_ft);
        assert!(stud.extra.iter().any(|(k, v, _)| *k == "spacing_in" && v == "24"));
        // One bottom plate and a double top plate, 12' each (16' stock).
        let plates: Vec<&Piece> = p.iter().filter(|x| x.kind == "plate").collect();
        assert_eq!(plates.len(), 3);
        assert!(plates.iter().all(|x| (x.length_ft - 12.0).abs() < 1e-9));
        // Sheathing panels on side 1 (4x8: three columns, 8' + a 2' piece each), two board
        // layers on side 2 (three 4x10 sheets each).
        assert_eq!(p.iter().filter(|x| x.kind == "sheathing").count(), 6);
        let sheets: Vec<&Piece> = p.iter().filter(|x| x.kind == "sheet").collect();
        assert_eq!(sheets.len(), 6);
        assert!(sheets.iter().all(|s| s.ty == "Moisture resistant"));
        // Only the face's outer layer is taped; every layer is screwed. The ends are free: a
        // bead at each on the boarded face.
        assert_eq!(count(&p, "tape"), 1);
        assert_eq!(count(&p, "corner_bead"), 2);
        assert_eq!(p.iter().filter(|x| x.kind == "screws").map(|x| x.qty).sum::<f64>(), 2.0 * 120.0);
        assert!(p.iter().filter(|x| x.kind == "batt").all(|b| b.ty == "R-21"));
    }

    #[test]
    fn a_projects_stud_rule_for_the_wall_type_wins() {
        let asm =
            Assembly { layers: vec![layer("3 5/8\" metal studs", 3.625, LayerSide::Core, "framing", false, Some(16.0))], ..Assembly::default() };
        let mut r = Rules::default();
        r.stud_by_type.insert("P9".into(), (24.0, "project rule stud_layout/P9: 24".into()));
        let p = lay(&wall(20.0, Vec::new()), asm, &r);
        // 0.625 + 24k below 239.375 (10 studs) and the end.
        assert_eq!(count(&p, "stud"), 11);
        let s = p.iter().find(|x| x.kind == "stud").unwrap();
        assert!(s.extra.iter().any(|(k, v, src)| *k == "spacing_in" && v == "24" && src.kind == SourceKind::Rule));
        assert!(s.extra.iter().any(|(k, v, src)| *k == "gauge" && v == "20 ga" && src.kind == SourceKind::Default));
    }

    #[test]
    fn holes_cut_rectangles() {
        let h = Hole { s0: 2.0, s1: 4.0, z0: 0.0, z1: 7.0 };
        let r = minus_holes(0.0, 8.0, 0.0, 10.0, &[h]);
        let area: f64 = r.iter().map(|(a, b, c, e)| (b - a) * (e - c)).sum();
        assert!((area - (80.0 - 14.0)).abs() < 1e-9);
    }
}
