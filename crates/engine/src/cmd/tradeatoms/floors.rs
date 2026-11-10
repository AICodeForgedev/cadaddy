//! Concrete and the floors' and roof's framing as pieces.
//!
//! - Each pour: the spec's concrete elements (`concrete`, as `concrete.*` keeps them), else the
//!   lowest level's slab on grade under its built floor: its volume, thickness, mix and strength
//!   (the mix submittals) and its reinforcement as pieces — bars cut to stock with their laps,
//!   welded wire sheets with their laps, post-tensioning tendons (strand, spacing, chairs and
//!   anchors from the PT design read) — the vapor barrier's strips under it, and the conduit
//!   and pipe pieces that cross it linked to it.
//! - The floors above, where the documents give their framing (an I-joist or floor truss
//!   submittal): joists at their spacing across each floor's shorter span, rim board round it,
//!   subfloor panels over it, and the underlayment the documents give.
//! - The roof: a roof the build framed (its members, Q6314) is framed by those members, one
//!   piece each with the member's body (bearing, spacing, slope) and the roof's sheathing on
//!   them (Q6405: one framing system per roof); else, where the documents give trusses, trusses
//!   at their spacing over the top storey and the roof's sheathing panels.

use super::*;
use cadaddy_walk3d::concrete::{ConcreteElement, Rebar};

/// Most bars, tendons, sheets or joists one element gets.
const MAX_PER_ELEMENT: usize = 20_000;
/// The largest a concrete element is laid out at (feet across, deep).
const MAX_ELEMENT_FT: f64 = 5_000.0;

/// A prism over plan polygons from `z0` up to `z1`.
fn prism(polys: &[Vec<Vec2>], z0: f64, z1: f64) -> Mesh {
    let mut m = super::super::wallgen::empty_mesh();
    for poly in polys.iter().take(10_000) {
        let n = poly.len();
        if n < 3 {
            continue;
        }
        let base = u32::try_from(m.vertices.len()).unwrap_or(u32::MAX);
        for p in poly {
            m.vertices.push(Vec3::new(p.x, p.y, z0));
        }
        for p in poly {
            m.vertices.push(Vec3::new(p.x, p.y, z1));
        }
        let n32 = u32::try_from(n).unwrap_or(0);
        m.faces.push(cadaddy_doc::MeshFace { v: (0..n32).rev().map(|i| base + i).collect(), hidden: 0 });
        m.faces.push(cadaddy_doc::MeshFace { v: (0..n32).map(|i| base + n32 + i).collect(), hidden: 0 });
        for i in 0..n32 {
            let j = (i + 1) % n32;
            m.faces.push(cadaddy_doc::MeshFace { v: vec![base + i, base + j, base + n32 + j, base + n32 + i], hidden: 0 });
        }
    }
    m
}

/// A plan polygon clipped to the box `lo`–`hi` (Sutherland–Hodgman: the box is convex).
pub(super) fn clip_box(poly: &[Vec2], lo: Vec2, hi: Vec2) -> Vec<Vec2> {
    let mut out: Vec<Vec2> = poly.to_vec();
    let edges: [(Vec2, f64); 4] =
        [(Vec2::new(1.0, 0.0), lo.x), (Vec2::new(-1.0, 0.0), -hi.x), (Vec2::new(0.0, 1.0), lo.y), (Vec2::new(0.0, -1.0), -hi.y)];
    for (nrm, c) in edges {
        let input = std::mem::take(&mut out);
        let n = input.len();
        if n == 0 {
            break;
        }
        let inside = |p: Vec2| p.dot(nrm) >= c - 1e-12;
        for i in 0..n {
            let (Some(cur), Some(prev)) = (input.get(i).copied(), input.get((i + n - 1) % n).copied()) else { continue };
            let (ci, pi) = (inside(cur), inside(prev));
            if ci != pi {
                let (dc, dp) = (cur.dot(nrm) - c, prev.dot(nrm) - c);
                let t = dp / (dp - dc);
                if t.is_finite() {
                    out.push(prev + (cur - prev) * t);
                }
            }
            if ci {
                out.push(cur);
            }
        }
    }
    out
}

/// A floor element's top: its highest flat faces as plan polygons, and their height.
fn floor_top(m: &Mesh) -> Option<(Vec<Vec<Vec2>>, f64)> {
    let mut faces: Vec<(Vec<Vec2>, f64)> = Vec::new();
    for f in m.faces.iter().take(200_000) {
        let pts: Vec<Vec3> = f.v.iter().filter_map(|i| m.vertices.get(*i as usize).copied()).collect();
        if pts.len() < 3 {
            continue;
        }
        let z = pts.iter().map(|p| p.z).sum::<f64>() / pts.len() as f64;
        if !pts.iter().all(|p| (p.z - z).abs() < 1e-6) {
            continue;
        }
        let poly: Vec<Vec2> = pts.iter().map(|p| Vec2::new(p.x, p.y)).collect();
        if cadaddy_geom::shoelace(&poly).abs() > 1e-9 {
            faces.push((poly, z));
        }
    }
    let top = faces.iter().map(|f| f.1).fold(f64::NEG_INFINITY, f64::max);
    if !top.is_finite() {
        return None;
    }
    let polys: Vec<Vec<Vec2>> = faces.into_iter().filter(|f| (f.1 - top).abs() < 1e-6).map(|f| f.0).collect();
    (!polys.is_empty()).then_some((polys, top))
}

fn area_of(polys: &[Vec<Vec2>]) -> f64 {
    polys.iter().map(|p| cadaddy_geom::shoelace(p).abs()).sum()
}

fn bounds_of(polys: &[Vec<Vec2>]) -> Option<(Vec2, Vec2)> {
    let b = cadaddy_geom::Bounds2::from_points(polys.iter().flatten().copied());
    (!b.is_empty() && b.max.x > b.min.x && b.max.y > b.min.y).then_some((b.min, b.max))
}

/// The spans along the line through `p` along `dir` inside any of the polygons, merged.
fn spans(polys: &[Vec<Vec2>], p: Vec2, dir: Vec2) -> Vec<(f64, f64)> {
    geo::merge(polys.iter().flat_map(|poly| geo::inside_spans(poly, p, dir)).collect())
}

/// Parallel lines across the polygons at `step` apart (perpendicular to `dir`), starting `edge`
/// in from the extents: each line's spans as (start, end) points.
fn lines_across(polys: &[Vec<Vec2>], dir: Vec2, step: f64, edge: f64) -> Vec<Vec<(Vec2, Vec2)>> {
    let Some((lo, hi)) = bounds_of(polys) else { return Vec::new() };
    if !(step > 0.0 && step.is_finite()) {
        return Vec::new();
    }
    let across = Vec2::new(-dir.y, dir.x);
    let corners = [lo, Vec2::new(hi.x, lo.y), hi, Vec2::new(lo.x, hi.y)];
    let (mut a0, mut a1) = (f64::INFINITY, f64::NEG_INFINITY);
    let (mut d0, mut d1) = (f64::INFINITY, f64::NEG_INFINITY);
    for c in corners {
        a0 = a0.min(c.dot(across));
        a1 = a1.max(c.dot(across));
        d0 = d0.min(c.dot(dir));
        d1 = d1.max(c.dot(dir));
    }
    let mut out = Vec::new();
    let mut a = a0 + edge;
    let mut n = 0;
    while a <= a1 - edge + 1e-9 && n < MAX_PER_ELEMENT {
        let origin = across * a + dir * d0;
        let sp: Vec<(Vec2, Vec2)> = spans(polys, origin, dir)
            .into_iter()
            .map(|(t0, t1)| (origin + dir * t0, origin + dir * t1))
            .filter(|(p, q)| p.dist(*q) > edge * 0.5)
            .collect();
        if !sp.is_empty() {
            out.push(sp);
        }
        a += step;
        n += 1;
    }
    let _ = d1;
    out
}

/// The outline of polygons that share edges (a plate cut into strips): the edges no two of them
/// share, joined into straight runs.
pub(super) fn outline_runs(polys: &[Vec<Vec2>], tol: f64) -> Vec<(Vec2, Vec2)> {
    let mut edges: Vec<(Vec2, Vec2)> = Vec::new();
    for poly in polys.iter().take(2000) {
        let n = poly.len();
        for i in 0..n {
            if let (Some(a), Some(b)) = (poly.get(i).copied(), poly.get((i + 1) % n).copied())
                && a.dist(b) > tol
            {
                edges.push((a, b));
            }
        }
    }
    edges.truncate(20_000);
    let same = |p: Vec2, q: Vec2| p.dist(q) <= tol;
    let mut keep = vec![true; edges.len()];
    for i in 0..edges.len() {
        for j in i + 1..edges.len() {
            let (Some(e), Some(f)) = (edges.get(i), edges.get(j)) else { continue };
            if (same(e.0, f.1) && same(e.1, f.0)) || (same(e.0, f.0) && same(e.1, f.1)) {
                if let Some(k) = keep.get_mut(i) {
                    *k = false;
                }
                if let Some(k) = keep.get_mut(j) {
                    *k = false;
                }
            }
        }
    }
    let mut runs: Vec<(Vec2, Vec2)> = edges.into_iter().zip(keep).filter(|(_, k)| *k).map(|(e, _)| e).collect();
    // Join collinear runs that meet end to end.
    let collinear = |a: (Vec2, Vec2), b: (Vec2, Vec2)| {
        let (u, v) = (a.1 - a.0, b.1 - b.0);
        (u.x * v.y - u.y * v.x).abs() <= tol * (u.len() + v.len()) && u.dot(v) > 0.0
    };
    let mut joined = true;
    let mut guard = 0;
    while joined && guard < 10_000 {
        joined = false;
        guard += 1;
        'outer: for i in 0..runs.len() {
            for j in 0..runs.len() {
                if i == j {
                    continue;
                }
                let (Some(a), Some(b)) = (runs.get(i).copied(), runs.get(j).copied()) else { continue };
                if same(a.1, b.0) && collinear(a, b) {
                    if let Some(x) = runs.get_mut(i) {
                        x.1 = b.1;
                    }
                    runs.remove(j);
                    joined = true;
                    break 'outer;
                }
            }
        }
    }
    runs
}

/// A run of `len` feet cut into stock pieces lapped `lap` feet: the pieces' lengths.
pub(super) fn lapped_pieces(len: f64, stock: f64, lap: f64) -> Vec<f64> {
    if !(len.is_finite() && len > 1e-6 && stock.is_finite() && stock > lap + 0.5 && lap >= 0.0) || len > (stock - lap) * 1_000.0 {
        return Vec::new();
    }
    let mut out = Vec::new();
    let mut covered = 0.0;
    while covered < len - 1e-6 && out.len() < 10_000 {
        let start = if out.is_empty() { 0.0 } else { covered - lap };
        let piece = (len - start).min(stock);
        out.push(piece);
        covered = start + piece;
    }
    out
}

/// The strand weight per foot for a strand diameter, from the rules' table.
fn strand_lb_ft(r: &Rules, dia: f64) -> f64 {
    let mut best = (f64::INFINITY, 0.52);
    for row in r.list("concrete.strand_lb_per_ft") {
        let Some(a) = row.as_array() else { continue };
        if let (Some(d), Some(w)) = (a.first().and_then(Value::as_f64), a.get(1).and_then(Value::as_f64))
            && (d - dia).abs() < best.0
        {
            best = ((d - dia).abs(), w);
        }
    }
    best.1
}

/// A welded wire style's weight per square foot, from the rules' table.
fn mesh_lb_sf(r: &Rules, style: &str) -> Option<f64> {
    let norm = |s: &str| s.to_ascii_uppercase().replace([' ', '.', '-'], "").replace('×', "X");
    let want = norm(style);
    r.list("concrete.mesh_lb_per_100sf").iter().find_map(|row| {
        let a = row.as_array()?;
        let (name, w) = (a.first()?.as_str()?, a.get(1)?.as_f64()?);
        want.contains(&norm(name)).then_some(w / 100.0)
    })
}

/// The documents' mix for a slab or pour: its text and strength, with where it was read.
fn mix_of(r: &Rules) -> (String, String, Source) {
    // The building's mix (a site or utility mix is for what is outside): the one that names a
    // slab, else the first; its strength from the same submittal.
    let building = |f: &&Fact| !f.scope.starts_with("site");
    let mixes: Vec<&Fact> = r.facts_of("concrete.mix").filter(building).collect();
    let mix = mixes.iter().find(|f| f.value.to_ascii_uppercase().contains("SLAB")).or(mixes.first()).copied();
    let psi = mix
        .and_then(|m| r.facts_of("concrete.strength_psi").find(|f| f.scope == m.scope))
        .or_else(|| r.facts_of("concrete.strength_psi").find(building));
    let text = mix.map(|f| f.value.clone()).unwrap_or_default();
    let strength = psi.and_then(|f| f.number).map(|v| format!("{} psi", v.round())).unwrap_or_default();
    let src = mix.or(psi).map_or_else(|| Source::from_doc(SourceKind::Default, "no concrete mix in the documents read"), |f| f.source_of());
    (text, strength, src)
}

pub(super) fn lay_out(d: &Drawing, sp: &BuildingSpec, r: &Rules, ctx: &Ctx, batch: &mut Batch, want: &dyn Fn(&str) -> bool) {
    let upf = ctx.upf;
    // The built floors by level (their top polygons).
    let mut floors: Vec<(String, Handle, Vec<Vec<Vec2>>, f64)> = Vec::new();
    for e in d.model.iter().filter(|e| super::super::wallgen::made_as(e).as_deref() == Some("floor")).take(MAX_ELEMENTS) {
        let EntityKind::Mesh(m) = &e.kind else { continue };
        if let Some((polys, z)) = floor_top(m) {
            floors.push((super::super::bspec::made_level(e).unwrap_or_default(), e.handle, polys, z));
        }
    }
    let mut levels: Vec<&cadaddy_walk3d::Level> = sp.levels.iter().filter(|l| l.sheet.is_none()).collect();
    levels.sort_by(|a, b| a.elevation_ft.total_cmp(&b.elevation_ft));
    let lowest = levels.first().map(|l| l.name.clone()).unwrap_or_default();
    // The MEP pieces near the ground: what may cross a slab on grade.
    let crossers: Vec<(Handle, String, [f64; 3], [f64; 3])> = if want("concrete") {
        d.model
            .iter()
            .filter(|e| super::super::wallgen::made_as(e).as_deref() == Some("mep"))
            .filter(|e| {
                super::super::bimlook::made_tag(e, "part")
                    .is_some_and(|p| p.starts_with("atom:conduit") || p.starts_with("segment") || p == "atom:cable")
            })
            .filter_map(|e| {
                super::super::fieldview::mesh_box(e).map(|(lo, hi)| (e.handle, super::super::bspec::made_level(e).unwrap_or_default(), lo, hi))
            })
            .take(MAX_ELEMENTS)
            .collect()
    } else {
        Vec::new()
    };
    if want("concrete") {
        let ce = super::super::concrete::elements(sp, &d.bim_types);
        let cr = super::super::concrete::Rules::of(sp);
        for x in ce.iter().take(cadaddy_walk3d::concrete::MAX_CONCRETE) {
            spec_pour(&x.e, sp, r, &cr, upf, batch);
        }
        let covered = ce.iter().any(|x| x.e.kind == "slab" && x.e.level.eq_ignore_ascii_case(&lowest));
        if !covered {
            match floors.iter().find(|f| f.0.eq_ignore_ascii_case(&lowest)) {
                Some((level, h, polys, z)) => slab_on_grade(level, *h, polys, *z, r, &cr, upf, batch, &crossers),
                None => {
                    batch.missing.insert("concrete".into(), "no slab: the spec has no concrete and the lowest level has no built floor".into());
                }
            }
        }
    }
    if want("framing") {
        let joists = r.fact("framing.floor_framing").is_some() || !r.is_default("framing.joist_spacing_in");
        let mut framed = 0;
        // The walls the floors bear on: each floor's joists span between the lines of the
        // storey's walls under it that carry them.
        let walls: Vec<super::walls::Wall> = if joists {
            d.model
                .iter()
                .filter(|e| super::super::wallgen::made_as(e).as_deref() == Some("wall"))
                .filter_map(|e| super::walls::read_wall(e))
                .take(MAX_ELEMENTS)
                .collect()
        } else {
            Vec::new()
        };
        for (i, level) in levels.iter().enumerate().skip(1) {
            if super::super::wallgen::roof_level(&sp.levels, &level.name) {
                continue;
            }
            let Some((lv, h, polys, z)) = floors.iter().find(|f| f.0.eq_ignore_ascii_case(&level.name)) else { continue };
            if joists {
                // The storey's walls under the floor (over its plate), else the floor's own.
                let below = levels.get(i.saturating_sub(1)).map(|l| l.name.as_str()).unwrap_or_default();
                let over = |w: &&super::walls::Wall| {
                    bounds_of(polys).is_some_and(|(lo, hi)| {
                        w.ends().iter().any(|p| p.x >= lo.x - upf && p.x <= hi.x + upf && p.y >= lo.y - upf && p.y <= hi.y + upf)
                    })
                };
                let mut under: Vec<&super::walls::Wall> = walls.iter().filter(|w| w.level.eq_ignore_ascii_case(below)).filter(over).collect();
                if under.is_empty() {
                    under = walls.iter().filter(|w| w.level.eq_ignore_ascii_case(lv)).filter(over).collect();
                }
                framed_floor(lv, *h, polys, *z, &under, sp, r, upf, batch);
                framed += 1;
            }
        }
        if !joists && levels.len() > 1 {
            batch.missing.insert(
                "floor framing".into(),
                "the documents read give no floor framing (no joist or floor truss submittal): the upper floors' framing is not laid out".into(),
            );
        }
        // The roof: one framing system per roof (Q6405). A roof the build framed (its members
        // from the roof plans' notes, bearing, spacing and slope) is framed by those members:
        // the trade's pieces are they, never a second layout over the storey under it.
        let members = roof_members(d, upf);
        if !members.is_empty() {
            framed_roof(d, &members, r, upf, batch);
        } else if r.fact("framing.roof_framing").is_some() || !r.is_default("framing.truss_spacing_in") {
            // Over the top storey, trusses where the documents give them.
            let roof: Vec<(String, Handle, Vec<Vec<Vec2>>, f64)> = d
                .model
                .iter()
                .filter(|e| super::super::wallgen::made_as(e).as_deref() == Some("roof"))
                .filter_map(|e| {
                    let EntityKind::Mesh(m) = &e.kind else { return None };
                    let (polys, z) = floor_top(m)?;
                    Some((super::super::bspec::made_level(e).unwrap_or_default(), e.handle, polys, z))
                })
                .take(1000)
                .collect();
            let top_storey = levels.iter().rev().find(|l| !super::super::wallgen::roof_level(&sp.levels, &l.name)).map(|l| l.name.clone());
            let over: Option<(String, Handle, Vec<Vec<Vec2>>, f64)> = top_storey.and_then(|t| {
                let level = sp.level(&t)?;
                let (_, h, polys, z) = floors.iter().find(|f| f.0.eq_ignore_ascii_case(&t))?;
                Some((t.clone(), *h, polys.clone(), z + (level.height_ft - 0.0) * upf))
            });
            match (roof.first().cloned(), over) {
                (_, Some((lv, h, polys, z))) => trusses(&lv, h, &polys, z, r, upf, batch),
                (Some((lv, h, polys, z)), None) => trusses(&lv, h, &polys, z, r, upf, batch),
                (None, None) => {
                    batch.missing.insert("roof framing".into(), "no top storey or roof built to lay the trusses over".into());
                }
            }
        } else {
            batch
                .missing
                .insert("roof framing".into(), "the documents read give no roof trusses or rafters: the roof's framing is not laid out".into());
        }
        let _ = framed;
    }
}

/// A spec concrete element as its pour and its reinforcement and embeds.
fn spec_pour(e: &ConcreteElement, sp: &BuildingSpec, r: &Rules, cr: &super::super::concrete::Rules, upf: f64, batch: &mut Batch) {
    let Some(level) = sp.level(&e.level) else { return };
    let depth = e.depth_in / 12.0 * upf;
    let z1 = (level.elevation_ft + e.top_ft) * upf;
    let z0 = z1 - depth;
    if !(depth > 0.0 && z0.is_finite() && z1.is_finite()) {
        return;
    }
    let lo = Vec2::new(e.min[0].min(e.max[0]), e.min[1].min(e.max[1]));
    let hi = Vec2::new(e.min[0].max(e.max[0]), e.min[1].max(e.max[1]));
    // Sizes no building has (hostile specs): left out and said.
    let span = MAX_ELEMENT_FT * upf;
    if !(lo.is_finite() && hi.is_finite()) || hi.x - lo.x > span || hi.y - lo.y > span || e.depth_in > MAX_ELEMENT_FT * 12.0 {
        batch.notes.push(format!("concrete element {}: its size is out of range, so it has no pieces", e.id.chars().take(40).collect::<String>()));
        return;
    }
    let path: Vec<Vec2> =
        e.path.iter().take(MAX_PER_ELEMENT).filter(|p| p.iter().all(|v| v.is_finite() && v.abs() < 1e12)).map(|p| Vec2::new(p[0], p[1])).collect();
    let width = e.width_in / 12.0 * upf;
    let (plan_area, mut blocks) = if path.len() >= 2 {
        let len: f64 = path.windows(2).map(|w| if let [a, b] = w { a.dist(*b) } else { 0.0 }).sum();
        (len * width, path.windows(2).filter_map(|w| if let [a, b] = w { Some(Block::new(*a, *b, width, z0, z1)) } else { None }).collect::<Vec<_>>())
    } else if e.round {
        let r0 = (hi.x - lo.x).min(hi.y - lo.y) / 2.0;
        let c = (lo + hi) * 0.5;
        (std::f64::consts::PI * r0 * r0, vec![Block::new(Vec2::new(c.x - r0, c.y), Vec2::new(c.x + r0, c.y), 2.0 * r0, z0, z1)])
    } else {
        let cy = (lo.y + hi.y) / 2.0;
        ((hi.x - lo.x) * (hi.y - lo.y), vec![Block::new(Vec2::new(lo.x, cy), Vec2::new(hi.x, cy), hi.y - lo.y, z0, z1)])
    };
    blocks.truncate(MAX_BLOCKS);
    let volume_cf = plan_area * (e.depth_in / 12.0) / (upf * upf);
    let (mix, strength, mix_src) = mix_of(r);
    let src = if e.source.is_empty() {
        Source::from_doc(SourceKind::Print, &format!("concrete element {}", e.id))
    } else {
        Source::from_doc(doc_kind(&e.source), &e.source)
    };
    let mut p = Piece::new("concrete", "pour", format!("concrete {}", e.kind.replace('_', " ")), &e.level, src.clone());
    p.ty = if strength.is_empty() { e.kind.replace('_', " ") } else { strength.clone() };
    p.size = inches_text(e.depth_in);
    p.volume_cf = volume_cf;
    p.area_sf = plan_area / (upf * upf);
    p.blocks = blocks;
    p = p.computed("element", e.id.clone()).computed("placement", e.kind.clone()).with("thickness_in", num_text(e.depth_in), src.clone());
    if !mix.is_empty() {
        p = p.with("mix", mix, mix_src.clone());
    }
    if !strength.is_empty() {
        p = p.with("strength", strength, mix_src);
    }
    if !e.pour.is_empty() {
        p = p.with("pour", e.pour.clone(), src.clone());
    }
    let pour = batch.push(p);
    // Bars.
    for b in e.rebar.iter().take(64) {
        bars(e, b, lo, hi, &path, z0, z1, cr, r, upf, &e.level, pour, batch);
    }
    // Welded wire over the top.
    if !e.mesh.is_empty() && path.len() < 2 && !e.round {
        let cy = (lo.y + hi.y) / 2.0;
        let _ = cy;
        let poly = vec![lo, Vec2::new(hi.x, lo.y), hi, Vec2::new(lo.x, hi.y)];
        mesh_sheets(&[poly], &e.mesh, src.clone(), z1 - depth / 2.0, &e.level, r, upf, pour, batch);
    }
    for em in e.embeds.iter().take(256) {
        let at = em.at.map(|a| Vec2::new(a[0], a[1])).unwrap_or((lo + hi) * 0.5);
        let mut q = Piece::new(
            "concrete",
            "embed",
            format!("{} {}", em.size, em.kind.replace('_', " ")).trim().to_string(),
            &e.level,
            if em.source.is_empty() { src.clone() } else { Source::from_doc(doc_kind(&em.source), &em.source) },
        );
        q.ty = em.kind.clone();
        q.size = em.size.clone();
        q.qty = f64::from(em.count.clamp(1, 10_000));
        q.length_ft = em.length_ft.max(0.0);
        let s = 2.0 / 12.0 * upf;
        q.blocks.push(Block::new(at - Vec2::new(s, 0.0), at + Vec2::new(s, 0.0), 2.0 * s, z0, z1));
        let qi = batch.push(q);
        batch.link(qi, pour);
    }
}

/// One rebar callout of an element as its bars (each run cut to stock with laps) and chairs.
#[allow(clippy::too_many_arguments)]
fn bars(
    e: &ConcreteElement,
    b: &Rebar,
    lo: Vec2,
    hi: Vec2,
    path: &[Vec2],
    z0: f64,
    z1: f64,
    cr: &super::super::concrete::Rules,
    r: &Rules,
    upf: f64,
    level: &str,
    pour: usize,
    batch: &mut Batch,
) {
    let Some((db, lb_ft)) = super::super::concrete::bar(&b.bar) else { return };
    let cover = (if b.cover_in > 0.0 && b.cover_in.is_finite() { b.cover_in } else { cr.cover(&e.kind) }) / 12.0 * upf;
    let lap_ft = if b.lap_in > 0.0 && b.lap_in.is_finite() { b.lap_in / 12.0 } else { cr.lap_db * db / 12.0 };
    let stock = cr.stock_ft.max(lap_ft + 1.0);
    let src = Source::from_doc(doc_kind(&e.source), if e.source.is_empty() { "the element's rebar callout" } else { &e.source });
    let item = format!("{} rebar", b.bar);
    let mut chairs_area = 0.0;
    let made = std::cell::Cell::new(0usize);
    let add_run = |batch: &mut Batch, a: Vec2, c: Vec2, z: f64| {
        if made.get() >= MAX_PER_ELEMENT {
            return;
        }
        let len_ft = a.dist(c) / upf;
        let dir = if a.dist(c) > 1e-9 { (c - a) * (1.0 / a.dist(c)) } else { Vec2::new(1.0, 0.0) };
        let mut at = 0.0;
        let pieces = lapped_pieces(len_ft, stock, lap_ft);
        let n = pieces.len();
        for (i, piece) in pieces.into_iter().enumerate() {
            let (pa, pc) = (a + dir * (at * upf), a + dir * ((at + piece) * upf));
            let mut p = Piece::new("concrete", "rebar", item.clone(), level, src.clone());
            p.size = b.bar.clone();
            p.ty = format!("{} {}", b.direction.replace('_', " "), b.bar);
            p.length_ft = piece;
            p.weight_lb = piece * lb_ft;
            p = p
                .with(
                    "lap_ft",
                    num_text(if i + 1 < n { lap_ft } else { 0.0 }),
                    Source::from_doc(SourceKind::Rule, &format!("lap {} bar diameters (concrete rules lap_db)", cr.lap_db)),
                )
                .with("stock_ft", num_text(stock), r.source("concrete.rebar_stock_ft"));
            let dbu = db / 12.0 * upf;
            p.blocks.push(Block::new(pa, pc, dbu, z - dbu, z));
            let pi = batch.push(p);
            batch.link(pi, pour);
            made.set(made.get() + 1);
            at += piece - lap_ft;
        }
    };
    let layers = b.layers.clamp(1, 4);
    match b.direction.as_str() {
        "x" | "y" | "each_way" if path.len() < 2 && !e.round => {
            let step = b.spacing_in.max(1.0) / 12.0 * upf;
            let (x0, x1, y0, y1) = (lo.x + cover, hi.x - cover, lo.y + cover, hi.y - cover);
            if !(x1 > x0 && y1 > y0) {
                return;
            }
            for layer in 0..layers {
                let z = if layer == 0 { z1 - cover } else { z0 + cover + db / 12.0 * upf };
                let mut n = 0;
                if b.direction != "y" {
                    let mut y = y0;
                    while y <= y1 + 1e-9 && n < MAX_PER_ELEMENT && made.get() < MAX_PER_ELEMENT {
                        add_run(batch, Vec2::new(x0, y), Vec2::new(x1, y), z);
                        y += step;
                        n += 1;
                    }
                }
                if b.direction != "x" {
                    let mut x = x0;
                    while x <= x1 + 1e-9 && n < MAX_PER_ELEMENT && made.get() < MAX_PER_ELEMENT {
                        add_run(batch, Vec2::new(x, y0), Vec2::new(x, y1), z - db / 12.0 * upf);
                        x += step;
                        n += 1;
                    }
                }
                chairs_area += (x1 - x0) * (y1 - y0);
            }
        }
        "long" if path.len() >= 2 => {
            for i in 0..b.count.clamp(1, 64) {
                let z = z0 + cover + f64::from(i % 2) * (z1 - z0 - 2.0 * cover);
                for w in path.windows(2) {
                    if let [a, c] = w {
                        add_run(batch, *a, *c, z);
                    }
                }
            }
        }
        "long" => {
            // A pier's vertical bars.
            let c = (lo + hi) * 0.5;
            for i in 0..b.count.clamp(1, 64) {
                let len_ft = (z1 - z0 - 2.0 * cover) / upf;
                let mut p = Piece::new("concrete", "rebar", item.clone(), level, src.clone());
                p.size = b.bar.clone();
                p.ty = format!("vertical {}", b.bar);
                p.length_ft = len_ft;
                p.weight_lb = len_ft * lb_ft;
                p = p.computed("bar", (i + 1).to_string());
                let dbu = db / 12.0 * upf;
                p.blocks.push(Block::new(c, c + Vec2::new(dbu, 0.0), dbu, z0 + cover, z1 - cover));
                let pi = batch.push(p);
                batch.link(pi, pour);
            }
        }
        "dowels" => {
            let mut p = Piece::new("concrete", "rebar", format!("{} dowel", b.bar), level, src.clone());
            p.size = b.bar.clone();
            p.ty = "dowel".into();
            p.qty = f64::from(b.count.clamp(1, 10_000));
            p.length_ft = b.length_in / 12.0 * p.qty;
            p.weight_lb = p.length_ft * lb_ft;
            let c = (lo + hi) * 0.5;
            p.blocks.push(Block::new(
                c,
                c + Vec2::new(db / 12.0 * upf, 0.0),
                db / 12.0 * upf,
                z1 - b.length_in / 24.0 * upf,
                z1 + b.length_in / 24.0 * upf,
            ));
            let pi = batch.push(p);
            batch.link(pi, pour);
        }
        _ => {}
    }
    if chairs_area > 0.0 {
        let cs = r.num("concrete.chair_spacing_ft", 4.0).max(1.0) * upf;
        let mut p = Piece::new("concrete", "chair", "rebar chair", level, r.source("concrete.chair_spacing_ft"));
        p.qty = (chairs_area / (cs * cs)).ceil().max(1.0);
        let c = (lo + hi) * 0.5;
        p.blocks.push(Block::new(c, c + Vec2::new(2.0 / 12.0 * upf, 0.0), 2.0 / 12.0 * upf, z0, z0 + 2.0 / 12.0 * upf));
        let pi = batch.push(p);
        batch.link(pi, pour);
    }
}

/// Welded wire sheets over polygons: each sheet (its stock size) where it lands, lapped.
#[allow(clippy::too_many_arguments)]
fn mesh_sheets(polys: &[Vec<Vec2>], style: &str, src: Source, z: f64, level: &str, r: &Rules, upf: f64, pour: usize, batch: &mut Batch) {
    let Some((lo, hi)) = bounds_of(polys) else { return };
    let dims = r.nums("concrete.mesh_sheet_ft");
    let (sw, sl) = (dims.first().copied().unwrap_or(8.0), dims.get(1).copied().unwrap_or(20.0));
    let lap = r.num("concrete.mesh_lap_in", 6.0) / 12.0;
    let (step_x, step_y) = ((sl - lap).max(1.0) * upf, (sw - lap).max(1.0) * upf);
    let lb_sf = mesh_lb_sf(r, style);
    let mut y = lo.y;
    let mut n = 0;
    while y < hi.y - 1e-6 && n < MAX_PER_ELEMENT {
        let mut x = lo.x;
        while x < hi.x - 1e-6 && n < MAX_PER_ELEMENT {
            let (a, b) = (Vec2::new(x, y), Vec2::new((x + sl * upf).min(hi.x), (y + sw * upf).min(hi.y)));
            let land: f64 = polys.iter().map(|p| cadaddy_geom::shoelace(&clip_box(p, a, b)).abs()).sum();
            if land > 1e-6 {
                let mut p = Piece::new("concrete", "mesh", format!("welded wire {style}, {}x{}", num_text(sw), num_text(sl)), level, src.clone());
                p.ty = style.to_string();
                p.area_sf = land / (upf * upf);
                if let Some(w) = lb_sf {
                    p.weight_lb = sw * sl * w;
                }
                p = p.with("sheet_ft", format!("{}x{}", num_text(sw), num_text(sl)), r.source("concrete.mesh_sheet_ft")).with(
                    "lap_in",
                    num_text(lap * 12.0),
                    r.source("concrete.mesh_lap_in"),
                );
                let cy = (a.y + b.y) / 2.0;
                p.blocks.push(Block::new(Vec2::new(a.x, cy), Vec2::new(b.x, cy), b.y - a.y, z - 0.1 / 12.0 * upf, z));
                let pi = batch.push(p);
                batch.link(pi, pour);
            }
            x += step_x;
            n += 1;
        }
        y += step_y;
    }
}

/// The lowest level's slab on grade under its built floor: the pour, its reinforcement as the
/// documents give it (post-tensioning tendons, welded wire), its vapor barrier, and the MEP
/// pieces that cross it.
#[allow(clippy::too_many_arguments)]
fn slab_on_grade(
    level: &str,
    host: Handle,
    polys: &[Vec<Vec2>],
    top: f64,
    r: &Rules,
    cr: &super::super::concrete::Rules,
    upf: f64,
    batch: &mut Batch,
    crossers: &[(Handle, String, [f64; 3], [f64; 3])],
) {
    let pt = r.fact("concrete.pt").is_some();
    let thick_fact = r.fact("concrete.slab_thickness_in");
    let (thick, t_src) = match thick_fact.and_then(|f| f.number.map(|n| (n, f.source_of()))) {
        Some(x) => x,
        None if pt => (r.num("concrete.pt_slab_in", 5.0), r.source("concrete.pt_slab_in")),
        None => (r.num("concrete.slab_on_grade_in", 4.0), r.source("concrete.slab_on_grade_in")),
    };
    let thick = thick.clamp(2.0, 48.0);
    let t = thick / 12.0 * upf;
    let area = area_of(polys);
    let (mix, strength, mix_src) = mix_of(r);
    let kind = if pt { "post-tensioned slab on grade" } else { "slab on grade" };
    let mut p = Piece::new("concrete", "pour", format!("concrete {kind}"), level, t_src.clone());
    p.host = Some(host);
    p.ty = if strength.is_empty() { kind.into() } else { strength.clone() };
    p.size = inches_text(thick);
    p.area_sf = area / (upf * upf);
    p.volume_cf = p.area_sf * thick / 12.0;
    p = p.computed("placement", kind).with("thickness_in", num_text(thick), t_src.clone());
    if !mix.is_empty() {
        p = p.with("mix", mix.clone(), mix_src.clone());
    }
    if !strength.is_empty() {
        p = p.with("strength", strength.clone(), mix_src.clone());
    }
    // Under-slab crossings: MEP pieces of the level under or in the slab, inside it.
    let below = top - t - 4.0 * upf;
    let mut crossing: Vec<Handle> = Vec::new();
    for (h, lv, lo, hi) in crossers {
        if !lv.eq_ignore_ascii_case(level) || lo[2] > top + 1e-6 || hi[2] < below {
            continue;
        }
        let c = Vec2::new((lo[0] + hi[0]) / 2.0, (lo[1] + hi[1]) / 2.0);
        if polys.iter().any(|poly| cadaddy_geom::point_in_polygon(poly, c)) && crossing.len() < 2000 {
            crossing.push(*h);
        }
    }
    if !crossing.is_empty() {
        p = p.computed("crossings", crossing.len().to_string());
    }
    p.touches = crossing.iter().copied().take(48).collect();
    let pi = batch.push(p);
    if let Some(last) = batch.pieces.get_mut(pi) {
        last.blocks.clear();
    }
    // The pour's body is the floor's outline, the slab's depth down.
    if let Some(last) = batch.pieces.get_mut(pi) {
        let m = prism(polys, top - t, top);
        last.extra.push(("body", "the built floor's outline".into(), Source::computed()));
        last.custom = Some(m);
    }
    let Some((lo, hi)) = bounds_of(polys) else { return };
    let long_x = hi.x - lo.x >= hi.y - lo.y;
    if pt {
        // Tendons each way at their spacing, an anchor at each end, chairs under them.
        let strand = r.fact("concrete.strand_in").and_then(|f| f.number).unwrap_or_else(|| r.num("concrete.tendon_strand_in", 0.5));
        let spacing = r.num("concrete.tendon_spacing_ft", 5.0).clamp(1.0, 50.0) * upf;
        let edge = r.num("concrete.tendon_edge_in", 12.0) / 12.0 * upf;
        let chair = r.num("concrete.chair_spacing_ft", 4.0).max(0.5);
        let lb_ft = strand_lb_ft(r, strand);
        let src_t = r.source("concrete.tendon_spacing_ft");
        let item = format!("{} 7-wire strand tendon", inches_text(strand));
        for (axis, dir) in [
            ("long", if long_x { Vec2::new(1.0, 0.0) } else { Vec2::new(0.0, 1.0) }),
            ("short", if long_x { Vec2::new(0.0, 1.0) } else { Vec2::new(1.0, 0.0) }),
        ] {
            let z = top - t / 2.0;
            for line in lines_across(polys, dir, spacing, edge) {
                for (a, b) in line {
                    let (a, b) = (a + dir * edge, b - dir * edge);
                    let len_ft = a.dist(b) / upf;
                    if len_ft < 1.0 {
                        continue;
                    }
                    let mut q = Piece::new("concrete", "tendon", item.clone(), level, src_t.clone());
                    q.host = Some(host);
                    q.size = inches_text(strand);
                    q.ty = "unbonded 270 ksi".into();
                    q.length_ft = len_ft;
                    q.weight_lb = len_ft * lb_ft;
                    q = q.computed("direction", axis).with(
                        "strand",
                        inches_text(strand),
                        r.fact("concrete.strand_in").map_or_else(|| r.source("concrete.tendon_strand_in"), |f| f.source_of()),
                    );
                    q.blocks.push(Block::new(a, b, strand / 12.0 * upf, z - strand / 24.0 * upf, z + strand / 24.0 * upf));
                    let ti = batch.push(q);
                    batch.link(ti, pi);
                    for (end, at) in [("live", a), ("dead", b)] {
                        let mut an = Piece::new(
                            "concrete",
                            "anchor",
                            format!("tendon anchor, {end} end"),
                            level,
                            Source::from_doc(SourceKind::Rule, "PTI: an anchor at each end of a tendon (stressing and fixed)"),
                        );
                        an.host = Some(host);
                        an.size = inches_text(strand);
                        an.ty = end.into();
                        let s = 3.0 / 12.0 * upf;
                        an.blocks.push(Block::new(at - dir * s, at, 4.0 / 12.0 * upf, z - s, z + s));
                        let ai = batch.push(an);
                        batch.link(ai, ti);
                    }
                    let mut ch = Piece::new(
                        "concrete",
                        "chair",
                        "tendon support chair",
                        level,
                        r.fact("concrete.chair_spacing_ft").map_or_else(|| r.source("concrete.chair_spacing_ft"), |f| f.source_of()),
                    );
                    ch.host = Some(host);
                    ch.qty = (len_ft / chair).ceil().max(1.0);
                    ch.blocks.push(Block::new(a, b, 1.0 / 12.0 * upf, top - t, top - t + 1.0 / 12.0 * upf));
                    let ci = batch.push(ch);
                    batch.link(ci, ti);
                }
            }
        }
    }
    if let Some(style) = r.fact("concrete.mesh").map(|f| (f.value.clone(), f.source_of())) {
        mesh_sheets(polys, &style.0, style.1, top - t / 2.0, level, r, upf, pi, batch);
    }
    // The vapor barrier: strips of its rolls across the slab's long way, lapped.
    let roll = r.nums("concrete.vapor_roll_ft");
    let (rw, rl) = (roll.first().copied().unwrap_or(12.0), roll.get(1).copied().unwrap_or(100.0));
    let lap = r.num("concrete.vapor_lap_in", 6.0) / 12.0;
    let mil = r.fact("concrete.vapor_mil").and_then(|f| f.number).unwrap_or_else(|| r.num("concrete.vapor_barrier_mil", 10.0));
    let v_src = r.fact("concrete.vapor_mil").map_or_else(|| r.source("concrete.vapor_barrier_mil"), |f| f.source_of());
    let dir = if long_x { Vec2::new(1.0, 0.0) } else { Vec2::new(0.0, 1.0) };
    for line in lines_across(polys, dir, (rw - lap).max(1.0) * upf, (rw / 2.0) * upf) {
        for (a, b) in line {
            let mut at = a;
            let total = a.dist(b);
            let mut done = 0.0;
            let mut n = 0;
            while done < total - 1e-6 && n < 1000 {
                let len = (total - done).min(rl * upf);
                let end = at + dir * len;
                let mut v = Piece::new("concrete", "vapor_barrier", format!("{} mil vapor barrier", num_text(mil)), level, v_src.clone());
                v.host = Some(host);
                v.size = format!("{} mil", num_text(mil));
                v.area_sf = len * rw * upf / (upf * upf);
                v.length_ft = len / upf;
                v = v.with("roll_ft", format!("{}x{}", num_text(rw), num_text(rl)), r.source("concrete.vapor_roll_ft"));
                v.blocks.push(Block::new(at, end, rw * upf, top - t - 0.1 / 12.0 * upf, top - t));
                let vi = batch.push(v);
                batch.link(vi, pi);
                at = end;
                done += len;
                n += 1;
            }
        }
    }
    let _ = cr;
}

/// How one floor is framed: its members, their depth, spacing and series, each with where it came
/// from. The floor's own (a joist layout's level heading, its framing plan's note) comes first;
/// else the documents' value for the building, else the rules' default, marked as not given for
/// the level.
struct FloorJoists {
    member: String,
    depth: f64,
    depth_src: Source,
    /// `level`, `building` or `default`: whether the documents give this floor its depth.
    depth_from: &'static str,
    /// Another document's depth for the floor (the framing plan's note beside a joist
    /// submittal's): the design value kept beside the one built.
    design_depth: Option<(f64, Source)>,
    spacing: f64,
    spacing_src: Source,
    series: String,
    src: Source,
}

fn floor_joists(r: &Rules, level: &str) -> FloorJoists {
    let mine = |key: &'static str| r.facts.iter().filter(move |f| f.key == key && docs::scope_level(f).is_some_and(|c| docs::same_level(c, level)));
    let by_level = |key: &'static str| r.facts.iter().any(|f| f.key == key && docs::scope_level(f).is_some());
    let depths: Vec<(f64, Source)> = mine("framing.joist_depth_in").filter_map(|f| f.number.map(|n| (n, f.source_of()))).collect();
    let building_depth = r.num("framing.joist_depth_in", 11.875);
    let (depth, depth_src, depth_from) = match depths.first() {
        Some((n, s)) => (*n, s.clone(), "level"),
        None if r.is_default("framing.joist_depth_in") => (building_depth, r.source("framing.joist_depth_in"), "default"),
        // The documents give depths by level, but none for this one: the building's is a default
        // here.
        None if by_level("framing.joist_depth_in") => (building_depth, r.source("framing.joist_depth_in"), "default"),
        None => (building_depth, r.source("framing.joist_depth_in"), "building"),
    };
    let design_depth = depths.iter().skip(1).find(|(n, _)| (n - depth).abs() > 1e-6).cloned();
    let (spacing, spacing_src) = match mine("framing.joist_spacing_in").find_map(|f| f.number.map(|n| (n, f.source_of()))) {
        Some(x) => x,
        None => (r.num("framing.joist_spacing_in", 16.0), r.source("framing.joist_spacing_in")),
    };
    let building_series = || {
        let same_depth = depth_from != "level" || (building_depth - depth).abs() < 1e-6;
        r.facts.iter().find(|f| f.key == "framing.joist_series" && docs::scope_level(f).is_none()).filter(|_| same_depth).map(|f| f.value.clone())
    };
    let series = mine("framing.joist_series").map(|f| f.value.clone()).next().or_else(building_series).unwrap_or_default();
    let member = mine("framing.floor_member").map(|f| f.value.clone()).next().unwrap_or_else(|| "joist".into());
    let src = mine("framing.floor_framing")
        .next()
        .or_else(|| r.facts.iter().find(|f| f.key == "framing.floor_framing" && docs::scope_level(f).is_none()))
        .map_or_else(|| r.source("framing.joist_spacing_in"), |f| f.source_of());
    FloorJoists {
        member,
        depth: depth.clamp(4.0, 48.0),
        depth_src,
        depth_from,
        design_depth,
        spacing: spacing.clamp(8.0, 48.0),
        spacing_src,
        series,
        src,
    }
}

/// The joist bearing rule's data ([`BEARING_RULE`] in `tradepractice.json`).
const BEARING_RULE: &str = "S-FLOOR-JOIST-BEARING";

/// A line inside a floor its joists bear on: `at` along the joists, from `lo` to `hi` across
/// them, and what it is.
#[derive(Clone, Debug)]
struct Bearing {
    at: f64,
    lo: f64,
    hi: f64,
    what: String,
}

/// The lines a floor's joists bear on between its outside walls ([`BEARING_RULE`]): the walls under
/// it that run across the joists and carry them — one its type calls bearing, or a line of walls
/// running most of the floor's length (a double-loaded corridor's walls, a bearing line with its
/// doors) — never one its type calls non-bearing. A partition legend's stud material doesn't
/// decide it: the structure's notes govern what bears (a wood-framed floor's corridor walls are
/// framed to carry it whatever the legend reads), and a short partition never makes a line.
fn bearing_lines(walls: &[&super::walls::Wall], sp: &BuildingSpec, dir: Vec2, polys: &[Vec<Vec2>], upf: f64) -> (Vec<Bearing>, BearingLook) {
    let mut look = BearingLook { walls: walls.len(), ..BearingLook::default() };
    let Some((lo, hi)) = bounds_of(polys) else { return (Vec::new(), look) };
    let rule = super::super::provenance::rule_data(BEARING_RULE);
    let num = |k: &str, d: f64| rule.get(k).and_then(Value::as_f64).filter(|x| x.is_finite() && *x >= 0.0).unwrap_or(d);
    let words = |k: &str| -> Vec<String> {
        rule.get(k)
            .and_then(Value::as_array)
            .map(|a| a.iter().filter_map(Value::as_str).map(str::to_ascii_uppercase).take(64).collect())
            .unwrap_or_default()
    };
    let (bearing_words, non_words) = (words("bearing_words"), words("non_bearing_words"));
    let fraction = num("line_fraction", 0.75).clamp(0.05, 1.0);
    let tol = num("line_tol_in", 6.0) / 12.0 * upf;
    let skew = num("angle_tol_deg", 10.0).clamp(0.0, 45.0).to_radians().sin();
    let across = Vec2::new(-dir.y, dir.x);
    let corners = [lo, Vec2::new(hi.x, lo.y), hi, Vec2::new(lo.x, hi.y)];
    let (a0, a1) = corners.iter().fold((f64::INFINITY, f64::NEG_INFINITY), |(m, x), c| (m.min(c.dot(across)), x.max(c.dot(across))));
    let length = a1 - a0;
    if !(length.is_finite() && length > 0.0) {
        return (Vec::new(), look);
    }
    let mut out: Vec<Bearing> = Vec::new();
    // (at, lo, hi) of the walls that may make a bearing line.
    let mut lines: Vec<(f64, f64, f64, &str)> = Vec::new();
    for w in walls.iter().take(MAX_ELEMENTS) {
        if w.u.dot(dir).abs() > skew {
            continue;
        }
        let [p, q] = w.ends();
        let (s0, s1) = (p.dot(across).min(q.dot(across)).max(a0), p.dot(across).max(q.dot(across)).min(a1));
        if s1 <= s0 {
            continue;
        }
        look.across += 1;
        let at = (p.dot(dir) + q.dot(dir)) / 2.0;
        let wt = sp.wall_types.iter().find(|t| !w.wall_type.is_empty() && t.name.eq_ignore_ascii_case(&w.wall_type));
        let text = wt.map(|t| {
            let mut x = t.name.to_ascii_uppercase();
            if let Some(a) = &t.assembly {
                x.push(' ');
                x.push_str(&a.tag.to_ascii_uppercase());
                x.push(' ');
                x.push_str(&a.description.to_ascii_uppercase());
                for l in a.layers.iter().take(cadaddy_walk3d::MAX_ASSEMBLY_LAYERS) {
                    if l.category.eq_ignore_ascii_case("framing") {
                        x.push(' ');
                        x.push_str(&l.material.to_ascii_uppercase());
                    }
                }
            }
            x
        });
        let text = text.unwrap_or_default();
        if non_words.iter().any(|n| text.contains(n.as_str())) {
            look.not_bearing += 1;
            continue;
        }
        if bearing_words.iter().any(|b| text.contains(b.as_str())) {
            out.push(Bearing { at, lo: s0 - tol, hi: s1 + tol, what: format!("bearing wall type {}", w.wall_type) });
            continue;
        }
        lines.push((at, s0, s1, w.wall_type.as_str()));
    }
    // The other walls' lines: the walls on one line (within the tolerance), and the share of the
    // floor's length they run.
    lines.sort_by(|a, b| a.0.total_cmp(&b.0));
    let mut i = 0usize;
    while i < lines.len() {
        let first = lines.get(i).map_or(0.0, |l| l.0);
        let group: Vec<(f64, f64, f64, &str)> = lines.iter().skip(i).take_while(|l| l.0 - first <= tol).copied().collect();
        i += group.len().max(1);
        let mut spans: Vec<(f64, f64)> = group.iter().map(|g| (g.1, g.2)).collect();
        spans.sort_by(|a, b| a.0.total_cmp(&b.0));
        let mut run = 0.0;
        let mut cur: Option<(f64, f64)> = None;
        for (s0, s1) in spans {
            cur = match cur {
                Some((c0, c1)) if s0 <= c1 => Some((c0, c1.max(s1))),
                Some((c0, c1)) => {
                    run += c1 - c0;
                    Some((s0, s1))
                }
                None => Some((s0, s1)),
            };
        }
        if let Some((c0, c1)) = cur {
            run += c1 - c0;
        }
        let share = run / length;
        look.longest = look.longest.max(share);
        if share + 1e-9 >= fraction {
            let n = group.len() as f64;
            let at = group.iter().map(|g| g.0).sum::<f64>() / n;
            let (glo, ghi) = group.iter().fold((f64::INFINITY, f64::NEG_INFINITY), |(m, x), g| (m.min(g.1), x.max(g.2)));
            // Its ends reach the walls it meets.
            out.push(Bearing {
                at,
                lo: glo - tol,
                hi: ghi + tol,
                what: format!("a wall line running {}% of the floor's length{}", (share * 100.0).round(), types_of(&group)),
            });
        }
    }
    out.truncate(1000);
    look.fraction = fraction;
    (out, look)
}

/// The wall types of a line of walls, the longest run first (`: types B6A, A5A`).
fn types_of(group: &[(f64, f64, f64, &str)]) -> String {
    let mut by: Vec<(&str, f64)> = Vec::new();
    for g in group.iter().filter(|g| !g.3.is_empty()) {
        match by.iter_mut().find(|b| b.0 == g.3) {
            Some(b) => b.1 += g.2 - g.1,
            None => by.push((g.3, g.2 - g.1)),
        }
    }
    by.sort_by(|a, b| b.1.total_cmp(&a.1));
    if by.is_empty() { String::new() } else { format!(": types {}", by.iter().take(3).map(|b| b.0).collect::<Vec<_>>().join(", ")) }
}

/// What [`bearing_lines`] looked at: the walls under the floor, those across its joists, those
/// their types keep from bearing, the longest line's share of the floor's length and the share a
/// bearing line needs.
#[derive(Clone, Copy, Debug, Default)]
struct BearingLook {
    walls: usize,
    across: usize,
    not_bearing: usize,
    longest: f64,
    fraction: f64,
}

/// The pieces a joist line from `a` to `b` (along `dir`, `x` across) is cut into at the bearing
/// lines it crosses, each piece's ends: no cut closer than `min_span` to another, nor than `end`
/// to the line's ends (a bearing that near the floor's edge is the joists' end bearing: they run
/// on over it to the edge, a cantilever of one piece).
fn joist_spans(a: Vec2, b: Vec2, dir: Vec2, x: f64, bearings: &[Bearing], min_span: f64, end: f64) -> Vec<(Vec2, Vec2, Option<String>)> {
    let (ta, tb) = (a.dot(dir), b.dot(dir));
    let (ta, tb, a, b) = if ta <= tb { (ta, tb, a, b) } else { (tb, ta, b, a) };
    let eps = 1e-6 * (1.0 + x.abs());
    let end = end.max(min_span);
    let mut cuts: Vec<(f64, &str)> = bearings
        .iter()
        .filter(|g| g.lo - eps <= x && x <= g.hi + eps && g.at > ta + end && g.at < tb - end)
        .map(|g| (g.at, g.what.as_str()))
        .collect();
    cuts.sort_by(|p, q| p.0.total_cmp(&q.0));
    let mut kept: Vec<(f64, &str)> = Vec::new();
    for c in cuts {
        if kept.last().is_none_or(|k| c.0 - k.0 >= min_span) {
            kept.push(c);
        }
    }
    if kept.is_empty() {
        return vec![(a, b, None)];
    }
    let at = |t: f64| a + dir * (t - ta);
    let mut out = Vec::new();
    let mut from = (ta, None::<&str>);
    for (t, what) in kept.iter().copied().chain(std::iter::once((tb, ""))) {
        let ends: Vec<&str> = [from.1, (!what.is_empty()).then_some(what)].into_iter().flatten().collect();
        out.push((at(from.0), at(t), Some(ends.join("; "))));
        from = (t, Some(what));
    }
    out
}

/// A floor framed with joists across its shorter span, each line cut into pieces at the bearing
/// lines under it (counted per span), rim board round it, subfloor panels and the documents'
/// underlayment.
#[allow(clippy::too_many_arguments)]
fn framed_floor(
    level: &str,
    host: Handle,
    polys: &[Vec<Vec2>],
    top: f64,
    walls: &[&super::walls::Wall],
    sp: &BuildingSpec,
    r: &Rules,
    upf: f64,
    batch: &mut Batch,
) {
    let Some((lo, hi)) = bounds_of(polys) else { return };
    let fj = floor_joists(r, level);
    let (spacing, depth, series) = (fj.spacing, fj.depth, fj.series.clone());
    let j_src = fj.src.clone();
    let sub_t = r.num("framing.subfloor_in", 0.75) / 12.0 * upf;
    let dz = depth / 12.0 * upf;
    let (z1, z0) = (top - sub_t, top - sub_t - dz);
    // Joists span the shorter way: they run across the long side, spaced along it.
    let along_x = hi.x - lo.x >= hi.y - lo.y;
    let dir = if along_x { Vec2::new(0.0, 1.0) } else { Vec2::new(1.0, 0.0) };
    let truss = fj.member == "truss";
    let kind_name = if truss { "floor truss" } else { "I-joist" };
    let item = if series.is_empty() { format!("{} {kind_name}", inches_text(depth)) } else { format!("{} {series} {kind_name}", inches_text(depth)) };
    let w = 2.5 / 12.0 * upf;
    let (bearings, look) = bearing_lines(walls, sp, dir, polys, upf);
    let pct = |v: f64| (v * 100.0).round();
    batch.notes.push(if bearings.is_empty() {
        format!(
            "{level}: the joists span the floor in one piece: no bearing line under it ({} walls, {} across the joists, {} kept from bearing by their type; the longest line of the others runs {}% of the floor's length, a bearing line {}%)",
            look.walls,
            look.across,
            look.not_bearing,
            pct(look.longest),
            pct(look.fraction)
        )
    } else {
        let d0 = polys.iter().flatten().map(|p| p.dot(dir)).fold(f64::INFINITY, f64::min);
        let lines: Vec<String> = bearings.iter().take(12).map(|g| format!("{} ({})", cadaddy_walk3d::feet((g.at - d0) / upf), g.what)).collect();
        format!(
            "{level}: the joists span between the outside walls and {} bearing lines under the floor ({BEARING_RULE}): {}",
            bearings.len(),
            lines.join(", ")
        )
    });
    let rule = super::super::provenance::rule(BEARING_RULE);
    let rule_ft = |k: &str, d: f64| {
        super::super::provenance::rule_data(BEARING_RULE).get(k).and_then(Value::as_f64).filter(|v| v.is_finite() && *v > 0.0).unwrap_or(d) * upf
    };
    let (min_span, end) = (rule_ft("min_span_ft", 2.0), rule_ft("end_bearing_ft", 4.0));
    let across = Vec2::new(-dir.y, dir.x);
    if fj.depth_from == "default" {
        batch.notes.push(format!(
            "{level}: the documents give no joist depth for this floor, so its joists are the default {} ({})",
            inches_text(depth),
            fj.depth_src.doc.as_deref().unwrap_or_default().chars().take(120).collect::<String>()
        ));
    }
    if let Some((d, s)) = &fj.design_depth {
        batch.notes.push(format!(
            "{level}: joists built {} as the joist layout gives them; the design value {} ({}) is kept beside them",
            inches_text(depth),
            inches_text(*d),
            s.doc.as_deref().unwrap_or_default().chars().take(120).collect::<String>()
        ));
    }
    for line in lines_across(polys, dir, spacing / 12.0 * upf, w) {
        for (a, b) in line {
            let x = a.dot(across);
            for (pa, pb, bears) in joist_spans(a, b, dir, x, &bearings, min_span, end) {
                let mut p = Piece::new("framing", "joist", item.clone(), level, j_src.clone());
                p.host = Some(host);
                p.size = inches_text(depth);
                p.ty = if !series.is_empty() {
                    series.clone()
                } else if truss {
                    "floor truss".into()
                } else {
                    "I-joist".into()
                };
                p.length_ft = pa.dist(pb) / upf;
                p = p.with("spacing_in", num_text(spacing), fj.spacing_src.clone()).with("depth_in", num_text(depth), fj.depth_src.clone());
                if fj.depth_from != "level" {
                    p = p.computed(
                        "depth_from",
                        if fj.depth_from == "default" { "default: not given for this floor" } else { "the documents' depth for the building" },
                    );
                }
                if let Some((d, s)) = &fj.design_depth {
                    p = p.with("design_depth_in", num_text(*d), s.clone());
                }
                if let Some(bears) = bears {
                    p = p.with("bears_on", bears, Source::from_doc(SourceKind::Rule, &rule.basis_for("")));
                }
                p.blocks.push(Block::new(pa, pb, w, z0, z1));
                batch.push(p);
            }
        }
    }
    // Rim board round the outline: one piece per straight run of its edge.
    for (a, b) in outline_runs(polys, upf * 1e-3) {
        let mut p = Piece::new("framing", "rim", format!("{} {}", inches_text(depth), r.text("framing.rim_item")), level, j_src.clone());
        p.host = Some(host);
        p.size = inches_text(depth);
        p.length_ft = a.dist(b) / upf;
        p.blocks.push(Block::new(a, b, 1.25 / 12.0 * upf, z0, z1));
        batch.push(p);
    }
    panels(
        polys,
        "framing",
        "sheathing",
        &format!("{} subfloor panel", inches_text(r.num("framing.subfloor_in", 0.75))),
        r,
        "framing.subfloor_in",
        level,
        host,
        &|_, _| (z1, top),
        upf,
        batch,
    );
    if let Some(u) = r.fact("concrete.underlayment_in").and_then(|f| f.number.map(|n| (n, f.source_of()))) {
        let area = area_of(polys) / (upf * upf);
        let mut p = Piece::new("concrete", "underlayment", format!("{} gypsum cement underlayment", inches_text(u.0)), level, u.1);
        p.host = Some(host);
        p.size = inches_text(u.0);
        p.area_sf = area;
        p.volume_cf = area * u.0 / 12.0;
        p.custom = Some(prism(polys, top, top + u.0 / 12.0 * upf));
        batch.push(p);
    }
}

/// Panels (subfloor, roof sheathing) over polygons, each where it lands; `z` gives a panel's
/// underside and top from its corners (a roof's sheathing lies on the members under it).
#[allow(clippy::too_many_arguments)]
fn panels(
    polys: &[Vec<Vec2>],
    trade: &'static str,
    kind: &'static str,
    item: &str,
    r: &Rules,
    thick_path: &str,
    level: &str,
    host: Handle,
    z: &dyn Fn(Vec2, Vec2) -> (f64, f64),
    upf: f64,
    batch: &mut Batch,
) {
    let Some((lo, hi)) = bounds_of(polys) else { return };
    let dims = r.nums("framing.panel_ft");
    let (pw, pl) = (dims.first().copied().unwrap_or(4.0) * upf, dims.get(1).copied().unwrap_or(8.0) * upf);
    let mut y = lo.y;
    let mut n = 0;
    while y < hi.y - 1e-6 && n < MAX_PER_ELEMENT * 4 {
        let mut x = lo.x;
        while x < hi.x - 1e-6 && n < MAX_PER_ELEMENT * 4 {
            let (a, b) = (Vec2::new(x, y), Vec2::new((x + pl).min(hi.x), (y + pw).min(hi.y)));
            let land: f64 = polys.iter().map(|p| cadaddy_geom::shoelace(&clip_box(p, a, b)).abs()).sum();
            if land > 0.01 * upf * upf {
                let mut p = Piece::new(trade, kind, format!("{item}, {}x{}", num_text(pw / upf), num_text(pl / upf)), level, r.source(thick_path));
                p.host = Some(host);
                p.size = inches_text(r.num(thick_path, 0.75));
                p.area_sf = land / (upf * upf);
                p = p.computed("cut", if land < pw * pl * 0.999 { "true" } else { "false" }).with(
                    "sheet_ft",
                    format!("{}x{}", num_text(pw / upf), num_text(pl / upf)),
                    r.source("framing.panel_ft"),
                );
                let cy = (a.y + b.y) / 2.0;
                let (z0, z1) = z(a, b);
                p.blocks.push(Block::new(Vec2::new(a.x, cy), Vec2::new(b.x, cy), b.y - a.y, z0, z1));
                batch.push(p);
            }
            x += pl;
            n += 1;
        }
        y += pw;
    }
}

/// A framing member the roof build made over a roof (`exterior::ROOF_MEMBER`): its element,
/// level, member kind (`truss`, `joist`), body, and the values it was laid out with.
struct RoofMember {
    handle: Handle,
    level: String,
    kind: &'static str,
    mesh: Mesh,
    block: Block,
    lf: f64,
    depth_in: f64,
    spacing_in: f64,
}

/// Most roof members a build takes.
const MAX_ROOF_MEMBERS: usize = 50_000;

/// The roof's framing members the roof build made, in model order.
fn roof_members(d: &Drawing, upf: f64) -> Vec<RoofMember> {
    let num = |e: &cadaddy_doc::Entity, k: &str| super::super::bimlook::made_tag(e, k).and_then(|v| v.parse::<f64>().ok()).filter(|v| v.is_finite());
    d.model
        .iter()
        .filter(|e| super::super::wallgen::made_as(e).as_deref() == Some(super::super::exterior::ROOF_MEMBER))
        .filter_map(|e| {
            let EntityKind::Mesh(m) = &e.kind else { return None };
            let block = member_block(m)?;
            let kind = match super::super::bimlook::made_tag(e, "kind") {
                Some(k) if k.eq_ignore_ascii_case("joist") => "joist",
                _ => "truss",
            };
            Some(RoofMember {
                handle: e.handle,
                level: super::super::bspec::made_level(e).unwrap_or_default(),
                kind,
                mesh: m.clone(),
                lf: num(e, "lf").filter(|v| *v > 0.0).unwrap_or(block.a.dist(block.b) / upf),
                depth_in: num(e, "depth_in").unwrap_or(0.0),
                spacing_in: num(e, "spacing_in").unwrap_or(0.0),
                block,
            })
        })
        .take(MAX_ROOF_MEMBERS)
        .collect()
}

/// A member's body as a block: the box `wallgen::add_box` makes (its first four corners at the
/// bottom, `a - n`, `b - n`, `b + n`, `a + n`, the next four over them), else its extents along
/// their longer side.
fn member_block(m: &Mesh) -> Option<Block> {
    if !m.vertices.iter().all(|v| v.is_finite()) {
        return None;
    }
    if let [v0, v1, v2, v3, v4, ..] = m.vertices.as_slice()
        && m.vertices.len() == 8
    {
        let p = |v: &Vec3| Vec2::new(v.x, v.y);
        let b = Block::new((p(v0) + p(v3)) * 0.5, (p(v1) + p(v2)) * 0.5, p(v0).dist(p(v3)), v0.z, v4.z);
        if b.ok() && b.a.dist(b.b) > 1e-9 {
            return Some(b);
        }
    }
    let big = Vec3::new(f64::INFINITY, f64::INFINITY, f64::INFINITY);
    let (lo, hi) = m.vertices.iter().fold((big, -big), |(lo, hi), v| {
        (Vec3::new(lo.x.min(v.x), lo.y.min(v.y), lo.z.min(v.z)), Vec3::new(hi.x.max(v.x), hi.y.max(v.y), hi.z.max(v.z)))
    });
    let (w, h) = (hi.x - lo.x, hi.y - lo.y);
    let c = Vec2::new((lo.x + hi.x) / 2.0, (lo.y + hi.y) / 2.0);
    let b = if w >= h {
        Block::new(Vec2::new(lo.x, c.y), Vec2::new(hi.x, c.y), h, lo.z, hi.z)
    } else {
        Block::new(Vec2::new(c.x, lo.y), Vec2::new(c.x, hi.y), w, lo.z, hi.z)
    };
    (b.ok() && b.a.dist(b.b) > 1e-9).then_some(b)
}

/// A built roof's top faces (its upward faces, in plan): where its sheathing lies.
fn roof_top(m: &Mesh) -> Vec<Vec<Vec2>> {
    let mut out = Vec::new();
    for f in m.faces.iter().take(200_000) {
        let pts: Vec<Vec3> = f.v.iter().filter_map(|i| m.vertices.get(*i as usize).copied()).collect();
        if pts.len() < 3 || !pts.iter().all(|p| p.is_finite()) {
            continue;
        }
        // Newell's normal: up-facing and more level than steep.
        let n = pts
            .iter()
            .zip(pts.iter().cycle().skip(1))
            .fold(Vec3::ZERO, |n, (a, b)| n + Vec3::new((a.y - b.y) * (a.z + b.z), (a.z - b.z) * (a.x + b.x), (a.x - b.x) * (a.y + b.y)));
        let len = n.len();
        if !(len > 1e-12 && n.z / len > 0.5) {
            continue;
        }
        out.push(pts.iter().map(|p| Vec2::new(p.x, p.y)).collect());
    }
    out
}

/// A roof the build framed, as the framing trade's pieces: each of its members one piece (a
/// truss or a rafter joist), hosted on the member with its body, as the roof plans' notes and
/// the framing rule laid it out (bearing, spacing, depth, the roof's slope); and the roof's
/// sheathing panels on the members, hosted on the roof (Q6405: one framing system per roof).
fn framed_roof(d: &Drawing, members: &[RoofMember], r: &Rules, upf: f64, batch: &mut Batch) {
    let src = r.fact("framing.roof_framing").map_or_else(|| r.source("framing.truss_spacing_in"), |f| f.source_of());
    for m in members {
        let item = if m.kind == "joist" { "roof joist" } else { "roof truss" };
        let mut p = Piece::new("framing", m.kind, item, &m.level, src.clone());
        p.host = Some(m.handle);
        p.length_ft = m.lf;
        p.size = format!("{}' span", num_text(m.lf.round()));
        if m.spacing_in > 0.0 {
            p = p.with("spacing_in", num_text(m.spacing_in), src.clone());
        }
        if m.depth_in > 0.0 {
            p = p.with("depth_in", num_text(m.depth_in), src.clone());
        }
        p = p.computed("body", "the roof's member as the roof is framed: its bearing, spacing and depth, up to the deck as the roof slopes");
        p.blocks.push(m.block);
        p.custom = Some(m.mesh.clone());
        batch.push(p);
    }
    // The sheathing over each framed roof, on the members under each panel.
    let levels: BTreeSet<&str> = members.iter().map(|m| m.level.as_str()).collect();
    let th = r.num("framing.roof_sheathing_in", 0.625);
    let rt = th / 12.0 * upf;
    for e in d.model.iter().filter(|e| super::super::wallgen::made_as(e).as_deref() == Some("roof")).take(1000) {
        let level = super::super::bspec::made_level(e).unwrap_or_default();
        if !levels.contains(level.as_str()) {
            continue;
        }
        let EntityKind::Mesh(m) = &e.kind else { continue };
        let polys = roof_top(m);
        if polys.is_empty() {
            continue;
        }
        let under: Vec<&RoofMember> = members.iter().filter(|m| m.level == level).collect();
        // On the highest member under the panel (its tops follow the roof up), else the nearest.
        let on = |a: Vec2, b: Vec2| {
            let (lo, hi) = (Vec2::new(a.x.min(b.x), a.y.min(b.y)), Vec2::new(a.x.max(b.x), a.y.max(b.y)));
            let crosses = |k: &Block| {
                let (p, q) = (Vec2::new(k.a.x.min(k.b.x), k.a.y.min(k.b.y)), Vec2::new(k.a.x.max(k.b.x), k.a.y.max(k.b.y)));
                p.x <= hi.x && q.x >= lo.x && p.y <= hi.y && q.y >= lo.y
            };
            let c = (lo + hi) * 0.5;
            let z = under.iter().filter(|m| crosses(&m.block)).map(|m| m.block.z1).fold(f64::NEG_INFINITY, f64::max);
            let z = if z.is_finite() {
                z
            } else {
                under
                    .iter()
                    .min_by(|x, y| x.block.a.lerp(x.block.b, 0.5).dist(c).total_cmp(&y.block.a.lerp(y.block.b, 0.5).dist(c)))
                    .map_or(0.0, |m| m.block.z1)
            };
            (z, z + rt)
        };
        panels(
            &polys,
            "framing",
            "sheathing",
            &format!("{} roof sheathing panel", inches_text(th)),
            r,
            "framing.roof_sheathing_in",
            &level,
            e.handle,
            &on,
            upf,
            batch,
        );
    }
}

/// Roof trusses over a footprint at their spacing across its shorter span, and its sheathing.
fn trusses(level: &str, host: Handle, polys: &[Vec<Vec2>], z: f64, r: &Rules, upf: f64, batch: &mut Batch) {
    let Some((lo, hi)) = bounds_of(polys) else { return };
    let spacing = r.num("framing.truss_spacing_in", 24.0).clamp(12.0, 96.0);
    let src = r.fact("framing.roof_framing").map_or_else(|| r.source("framing.truss_spacing_in"), |f| f.source_of());
    let along_x = hi.x - lo.x >= hi.y - lo.y;
    let dir = if along_x { Vec2::new(0.0, 1.0) } else { Vec2::new(1.0, 0.0) };
    let w = 1.5 / 12.0 * upf;
    // A truss's body: its span at the bearing height, 2' deep (its profile is the submittal's).
    let depth = 2.0 * upf;
    for line in lines_across(polys, dir, spacing / 12.0 * upf, w) {
        for (a, b) in line {
            let span = a.dist(b) / upf;
            let mut p = Piece::new("framing", "truss", "roof truss", level, src.clone());
            p.host = Some(host);
            p.length_ft = span;
            p.size = format!("{}' span", num_text(span.round()));
            p = p
                .with("spacing_in", num_text(spacing), r.source("framing.truss_spacing_in"))
                .computed("body", "span at the bearing, 2' deep (the profile is the truss submittal's)");
            p.blocks.push(Block::new(a, b, w, z, z + depth));
            batch.push(p);
        }
    }
    let rt = r.num("framing.roof_sheathing_in", 0.625) / 12.0 * upf;
    panels(
        polys,
        "framing",
        "sheathing",
        &format!("{} roof sheathing panel", inches_text(r.num("framing.roof_sheathing_in", 0.625))),
        r,
        "framing.roof_sheathing_in",
        level,
        host,
        &|_, _| (z + depth, z + depth + rt),
        upf,
        batch,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A wall from `a` to `b` (12 units a foot) of type `ty` on level 1.
    fn wall(a: Vec2, b: Vec2, ty: &str) -> super::super::walls::Wall {
        let len = a.dist(b);
        let u = (b - a) * (1.0 / len);
        super::super::walls::Wall {
            handle: Handle::default(),
            level: "1".into(),
            wall_type: ty.into(),
            o: a,
            u,
            n: u.perp(),
            s0: 0.0,
            s1: len,
            t: 5.5,
            zf: 0.0,
            zt: 120.0,
            holes: Vec::new(),
        }
    }

    #[test]
    fn joists_bear_on_long_wall_lines_and_bearing_types_never_on_non_bearing_or_short_walls() {
        let plate = vec![vec![Vec2::new(0.0, 0.0), Vec2::new(960.0, 0.0), Vec2::new(960.0, 744.0), Vec2::new(0.0, 744.0)]];
        let dir = Vec2::new(0.0, 1.0);
        let mut sp = BuildingSpec::default();
        let asm = |tag: &str, description: &str, material: &str| cadaddy_walk3d::Assembly {
            tag: tag.into(),
            description: description.into(),
            layers: vec![cadaddy_walk3d::AssemblyLayer { material: material.into(), category: "framing".into(), ..Default::default() }],
            ..Default::default()
        };
        for (name, a) in [
            ("NB", asm("NB", "NON-BEARING PARTITION", "2x4 wood studs")),
            ("LB", asm("LB", "LOAD BEARING WALL", "2x6 wood studs")),
            ("MS", asm("MS", "PARTITION", "3 5/8\" 20 ga metal studs")),
        ] {
            sp.wall_types.push(cadaddy_walk3d::WallType {
                name: name.into(),
                height_ft: None,
                thickness_in: 5.5,
                from: cadaddy_walk3d::WallFrom::Pairs,
                assembly: Some(a),
            });
        }
        let walls = [
            // A corridor wall the whole length in two pieces (a door's gap between).
            wall(Vec2::new(7.0, 336.0), Vec2::new(450.0, 336.0), ""),
            wall(Vec2::new(486.0, 336.0), Vec2::new(953.0, 336.0), ""),
            // A short closet wall across the joists, and one along them: no bearing line.
            wall(Vec2::new(120.0, 120.0), Vec2::new(216.0, 120.0), ""),
            wall(Vec2::new(240.0, 7.0), Vec2::new(240.0, 330.0), ""),
            // A short wall its type calls bearing bears; a long one typed non-bearing doesn't; a long
            // line of a type the legend reads as steel studs does (the legend's material doesn't
            // decide what bears).
            wall(Vec2::new(600.0, 600.0), Vec2::new(700.0, 600.0), "LB"),
            wall(Vec2::new(7.0, 500.0), Vec2::new(953.0, 500.0), "NB"),
            wall(Vec2::new(7.0, 560.0), Vec2::new(953.0, 560.0), "MS"),
        ];
        let refs: Vec<&super::super::walls::Wall> = walls.iter().collect();
        let (mut got, look) = bearing_lines(&refs, &sp, dir, &plate, 12.0);
        assert_eq!((look.walls, look.across, look.not_bearing), (7, 6, 1), "{look:?}");
        got.sort_by(|a, b| a.at.total_cmp(&b.at));
        let at: Vec<f64> = got.iter().map(|b| b.at).collect();
        assert_eq!(at, vec![336.0, 560.0, 600.0], "{got:?}");
        // Across the joists the line runs the whole plate (`across` is -x here).
        assert!(got.first().is_some_and(|b| b.lo <= -957.5 && b.hi >= -2.5 && b.what.contains("wall line")), "{got:?}");
        // A joist line crossing the two long lines: three spans; one under the bearing wall too: four.
        let spans = joist_spans(Vec2::new(26.5, 0.0), Vec2::new(26.5, 744.0), dir, -26.5, &got, 24.0, 48.0);
        assert_eq!(spans.len(), 3, "{spans:?}");
        assert!((spans.first().map_or(0.0, |s| s.0.dist(s.1)) - 336.0).abs() < 1e-9);
        assert!(spans.iter().all(|s| s.2.as_deref().is_some_and(|w| w.contains("wall line"))));
        let x = 650.0;
        assert_eq!(joist_spans(Vec2::new(x, 0.0), Vec2::new(x, 744.0), dir, -x, &got, 24.0, 48.0).len(), 4);
        // A bearing nearer an end than the end bearing's reach is no cut (the joist cantilevers).
        let near = [Bearing { at: 10.0, lo: -1e9, hi: 1e9, what: "x".into() }];
        assert_eq!(joist_spans(Vec2::new(0.0, 0.0), Vec2::new(0.0, 744.0), dir, 0.0, &near, 24.0, 48.0).len(), 1);
        // Hostile input never panics.
        let nan = [Bearing { at: f64::NAN, lo: f64::NAN, hi: f64::NAN, what: String::new() }];
        let _ = joist_spans(Vec2::new(0.0, 0.0), Vec2::new(0.0, f64::INFINITY), dir, 0.0, &nan, 24.0, 48.0);
        let _ = bearing_lines(&refs, &sp, dir, &[], 12.0);
    }

    #[test]
    fn laps_and_clips() {
        // A 50' bar of 20' stock lapped 2': 20 + 20 + 14 = 54' bought, 3 pieces.
        let p = lapped_pieces(50.0, 20.0, 2.0);
        assert_eq!(p.len(), 3);
        assert!((p.iter().sum::<f64>() - 54.0).abs() < 1e-9, "{p:?}");
        assert_eq!(lapped_pieces(15.0, 20.0, 2.0), vec![15.0]);
        assert!(lapped_pieces(f64::NAN, 20.0, 2.0).is_empty());
        let sq = vec![Vec2::new(0.0, 0.0), Vec2::new(10.0, 0.0), Vec2::new(10.0, 10.0), Vec2::new(0.0, 10.0)];
        let c = clip_box(&sq, Vec2::new(5.0, 5.0), Vec2::new(20.0, 20.0));
        assert!((cadaddy_geom::shoelace(&c).abs() - 25.0).abs() < 1e-9);
        let lines = lines_across(std::slice::from_ref(&sq), Vec2::new(1.0, 0.0), 2.0, 1.0);
        assert_eq!(lines.len(), 5);
        // A plate cut in three strips: its outline is four runs.
        let strip = |y0: f64, y1: f64| vec![Vec2::new(0.0, y0), Vec2::new(10.0, y0), Vec2::new(10.0, y1), Vec2::new(0.0, y1)];
        let runs = outline_runs(&[strip(0.0, 1.0), strip(1.0, 9.0), strip(9.0, 10.0)], 1e-6);
        assert_eq!(runs.len(), 4, "{runs:?}");
        assert!((runs.iter().map(|(a, b)| a.dist(*b)).sum::<f64>() - 40.0).abs() < 1e-9);
        let r = Rules::default();
        assert!((strand_lb_ft(&r, 0.5) - 0.52).abs() < 1e-9);
        assert_eq!(mesh_lb_sf(&r, "6x6-W2.9xW2.9"), Some(0.42));
        assert_eq!(mesh_lb_sf(&r, "6X6 W2.9XW2.9"), Some(0.42));
        assert_eq!(mesh_lb_sf(&r, "nothing"), None);
    }
}
