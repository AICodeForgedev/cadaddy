//! Ceilings from reflected ceiling plans, into the building spec's `rcp`, built by `model.build`.
//!
//! `rcp.read {region, level, offset?, apply?}` reads one level's RCP (a region of the sheet):
//! - **ceiling areas**: each height tag (`9'-0" A.F.F.`, `CLG 8'-6"`) gives the area round it,
//!   traced on the walls alone (the wall layers' objects: the grid lines would cut it into tiles),
//!   with the room named inside it;
//! - **suspended grids**: the area's two families of evenly spaced parallel lines, giving the tile
//!   size (2×2, 2×4) and where a grid line crosses;
//! - **soffits and bulkheads**: hatched areas with a height note (else 1' below the ceiling);
//! - **light fixtures and diffusers**: blocks on the lighting layers or named like fixtures
//!   (2X4, LT, LIGHT, FIX) or diffusers (DIFF, SD, RG, GRILLE), centred in their grid tile.
//!
//! The RCP sits elsewhere on the sheet than the floor plan: `offset` moves it onto the plan;
//! without one the lower-left corners of the walls on both are lined up.

use cadaddy_doc::{Common, Drawing, Entity, EntityKind, Handle, Mesh, MeshFace, MeshKind, RawTag, Space};
use cadaddy_geom::{Bounds2, Vec2, Vec3};
use cadaddy_walk3d::{BuildingSpec, CeilingArea, CeilingGrid, CeilingItem, ItemKind, Role, Soffit, level_at};
use serde_json::{Value, json};
use std::sync::Arc;

use super::bspec::{current, plan_objects};
use super::perceive::{feet_inches, texts};
use super::*;
use crate::{Result, Session};

/// Items and lines looked at, at most (hostile drawings).
const MAX_ITEMS: usize = 20_000;
const MAX_LINES: usize = 100_000;

pub fn specs() -> Vec<CommandSpec> {
    vec![CommandSpec::new("rcp.read", "Read Ceiling Plan", run_read).params(
        "{region: [x0,y0,x1,y1] (the RCP on the sheet), level: name, offset?: [dx,dy] (RCP → floor plan; default: the walls' lower-left corners lined up), apply?: bool (replace the level's ceilings, soffits and items in the spec; rooms take their ceiling heights)} → {offset, ceilings: [{room?, outline, height_ft, grid?: {spacing_in, origin, angle_deg}}], soffits, items: [{kind, block, at, size_in}], applied}",
    )]
}

/// A height tag: a ceiling height note.
fn height_tag(t: &str) -> Option<f64> {
    let up = t.to_ascii_uppercase();
    let tagged = up.contains("A.F.F") || up.contains("AFF") || up.contains("CLG") || up.contains("CEILING") || up.contains("CEIL.");
    if !tagged {
        return None;
    }
    feet_inches(&up).map(|i| i / 12.0).filter(|f| (6.0..=40.0).contains(f))
}

fn item_kind(block: &str, on_fixture_layer: bool) -> Option<ItemKind> {
    let up = block.to_ascii_uppercase();
    let words: Vec<&str> = up.split(|c: char| !c.is_ascii_alphanumeric()).filter(|w| !w.is_empty()).collect();
    let starts = |p: &[&str]| words.iter().any(|w| p.iter().any(|x| w.starts_with(x)));
    if starts(&["DIFF", "GRILLE", "SUPPLY", "RETURN"]) || words.iter().any(|w| matches!(*w, "SD" | "RG" | "RA" | "SA" | "EG")) {
        Some(ItemKind::Diffuser)
    } else if on_fixture_layer || starts(&["LIGHT", "LITE", "LT", "FIX", "LUM", "TROFFER", "2X4", "2X2", "1X4", "DL", "CAN"]) {
        Some(ItemKind::Fixture)
    } else {
        None
    }
}

/// A plan of one family of parallel lines: direction (degrees, 0–180), the offsets across it.
fn families(segs: &[(Vec2, Vec2)]) -> Vec<(f64, Vec<f64>)> {
    let mut groups: std::collections::BTreeMap<i64, Vec<f64>> = std::collections::BTreeMap::new();
    for (a, b) in segs.iter().take(MAX_LINES) {
        let d = *b - *a;
        if d.len() <= 1e-9 {
            continue;
        }
        let ang = d.y.atan2(d.x).rem_euclid(std::f64::consts::PI);
        let key = (ang.to_degrees() * 2.0).round() as i64 % 360;
        let n = Vec2::new(-ang.sin(), ang.cos());
        groups.entry(key).or_default().push(n.dot(*a));
    }
    groups
        .into_iter()
        .map(|(k, mut o)| {
            o.sort_by(f64::total_cmp);
            o.dedup_by(|a, b| (*a - *b).abs() < 1e-6);
            (k as f64 / 2.0, o)
        })
        .collect()
}

/// The common spacing of sorted offsets (the median gap), if at least 3 lines are evenly spaced.
fn spacing(o: &[f64]) -> Option<f64> {
    if o.len() < 3 {
        return None;
    }
    let mut gaps: Vec<f64> = o.windows(2).map(|w| w[1] - w[0]).filter(|g| *g > 1e-6).collect();
    gaps.sort_by(f64::total_cmp);
    let g = *gaps.get(gaps.len() / 2)?;
    let even = gaps.iter().filter(|x| (*x / g - (*x / g).round()).abs() < 0.05).count();
    (even * 10 >= gaps.len() * 8).then_some(g)
}

fn run_read(s: &mut Session, p: &Value) -> Result<Value> {
    let (mut sp, _) = current(s, "rcp.read")?;
    let upf = if sp.units_per_foot.is_finite() && sp.units_per_foot > 0.0 { sp.units_per_foot } else { 12.0 };
    let level = str_param(p, "level").ok_or_else(|| bad("rcp.read", "`level` is required"))?;
    let level = sp.level(level).cloned().ok_or_else(|| bad("rcp.read", format!("there is no level named {level}")))?;
    let r: Vec<f64> = p.get("region").and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_f64).collect()).unwrap_or_default();
    let [x0, y0, x1, y1] = r[..] else { return Err(bad("rcp.read", "`region` must be [x0, y0, x1, y1]")) };
    if ![x0, y0, x1, y1].iter().all(|v| v.is_finite()) || (x1 - x0).abs() < 1e-9 || (y1 - y0).abs() < 1e-9 {
        return Err(bad("rcp.read", "`region` must be a rectangle with an area"));
    }
    let region = Bounds2::new(Vec2::new(x0.min(x1), y0.min(y1)), Vec2::new(x0.max(x1), y0.max(y1)));
    let plan = super::verify::plan_session(s)?;
    let d: &Drawing = plan.doc()?;
    let all = plan_objects(d);
    let centre = |e: &Entity| {
        let b = cadaddy_doc::entity_bounds(d, e, 0);
        (!b.is_empty()).then(|| b.center())
    };
    let ents: Vec<&Arc<Entity>> = all.iter().copied().filter(|e| centre(e).is_some_and(|c| region.contains(c))).take(MAX_LINES).collect();
    let is_wall = |e: &Entity| sp.role(&e.common.layer).is_some_and(|r| matches!(r.role, Role::Wall | Role::CurtainWall | Role::Column));
    // The offset onto the floor plan: given, else the walls' lower-left corners.
    let offset = match p.get("offset").and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_f64).collect::<Vec<_>>()) {
        Some(v) => match v[..] {
            [dx, dy] if dx.is_finite() && dy.is_finite() => Vec2::new(dx, dy),
            _ => return Err(bad("rcp.read", "`offset` must be [dx, dy]")),
        },
        None => {
            let walls_in =
                |es: &[&Arc<Entity>]| es.iter().filter(|e| is_wall(e)).fold(Bounds2::EMPTY, |b, e| b.union(&cadaddy_doc::entity_bounds(d, e, 0)));
            let rcp_walls = walls_in(&ents);
            let floor: Vec<&Arc<Entity>> = all
                .iter()
                .copied()
                .filter(|e| !centre(e).is_some_and(|c| region.contains(c)))
                .filter(|e| centre(e).and_then(|c| level_at(&sp.levels, c.x, c.y)).is_some_and(|l| l.name == level.name))
                .collect();
            let plan_walls = walls_in(&floor);
            if rcp_walls.is_empty() || plan_walls.is_empty() { Vec2::new(0.0, 0.0) } else { plan_walls.min - rcp_walls.min }
        }
    };
    let moved = |q: &[Vec2]| q.iter().map(|p| [p.x + offset.x, p.y + offset.y]).collect::<Vec<[f64; 2]>>();
    // Tracing on the walls alone.
    let walls_only = {
        let mut wd = (*d).clone();
        let keep: std::collections::HashSet<Handle> = ents.iter().filter(|e| is_wall(e)).map(|e| e.handle).collect();
        let drop: Vec<Handle> = wd.model.iter().filter(|e| !keep.contains(&e.handle)).map(|e| e.handle).collect();
        for h in drop {
            wd.model.remove(h);
        }
        let mut t = Session::empty();
        t.open_drawing(wd, "RCP walls", None);
        t
    };
    let notes = texts(&ents);
    let plan_notes = texts(&all.iter().copied().filter(|e| !centre(e).is_some_and(|c| region.contains(c))).collect::<Vec<_>>());
    let hatches: Vec<(Vec<Vec2>, Bounds2)> = ents
        .iter()
        .filter_map(|e| match &e.kind {
            EntityKind::Hatch(h) => {
                let l = h.loops.first()?;
                let q = cadaddy_geom::Polyline { vertices: l.vertices.clone(), closed: true }.tessellate(1e-3);
                (q.len() >= 3).then(|| (q.clone(), Bounds2::from_points(q.iter().copied())))
            }
            _ => None,
        })
        .collect();
    let in_soffit = |p: Vec2| hatches.iter().any(|(q, _)| cadaddy_geom::point_in_polygon(q, p));
    let lines: Vec<(Vec2, Vec2)> =
        ents.iter().filter(|e| !is_wall(e)).flat_map(|e| super::perceive::geometry::segments(&[*e]).into_iter().map(|sg| (sg.a, sg.b))).collect();
    let mut ceilings: Vec<(CeilingArea, Vec<Vec2>)> = Vec::new();
    for (_, t, at, _) in &notes {
        let Some(h) = height_tag(t) else { continue };
        if in_soffit(*at) {
            continue;
        }
        let Ok(loops) = super::hatch::loops_at(&walls_only, *at) else { continue };
        let Some(outer) = loops.iter().find(|l| l.outer).or(loops.first()) else { continue };
        let q = cadaddy_geom::Polyline { vertices: outer.vertices.clone(), closed: true }.tessellate(1e-3);
        if q.len() < 3
            || ceilings.iter().any(|(_, o)| o.len() == q.len() && Bounds2::from_points(o.iter().copied()) == Bounds2::from_points(q.iter().copied()))
        {
            continue;
        }
        // The grid: two families of evenly spaced lines inside the area.
        let inside: Vec<(Vec2, Vec2)> = lines.iter().copied().filter(|(a, b)| cadaddy_geom::point_in_polygon(&q, (*a + *b) * 0.5)).collect();
        let fams: Vec<(f64, f64, f64)> =
            families(&inside).into_iter().filter_map(|(ang, o)| spacing(&o).map(|g| (ang, g, o.first().copied().unwrap_or(0.0)))).collect();
        let grid = match fams.as_slice() {
            [a, b, ..] => {
                // `a` runs along `angle`; the crossing of a line of each family is the origin.
                let (ua, ub) = (Vec2::from_angle(a.0.to_radians()), Vec2::from_angle(b.0.to_radians()));
                let (na, nb) = (Vec2::new(-ua.y, ua.x), Vec2::new(-ub.y, ub.x));
                let det = na.cross(nb);
                let origin =
                    if det.abs() > 1e-9 { Vec2::new((a.2 * nb.y - b.2 * na.y) / det, (na.x * b.2 - nb.x * a.2) / det) } else { Vec2::new(0.0, 0.0) };
                let k = upf / 12.0;
                let r8 = |v: f64| (v / k * 8.0).round() / 8.0;
                // Spacing along the first family's lines is the second family's gap.
                Some(CeilingGrid { spacing_in: [r8(b.1), r8(a.1)], origin: [origin.x + offset.x, origin.y + offset.y], angle_deg: a.0 })
            }
            _ => None,
        };
        // The room: named inside the area on the RCP, else on the floor plan under it.
        let on_plan: Vec<Vec2> = q.iter().map(|p| *p + offset).collect();
        let room = sp
            .rooms
            .iter()
            .find(|r| notes.iter().any(|(_, n, nat, _)| cadaddy_geom::point_in_polygon(&q, *nat) && super::bspec::names_room(r, n)))
            .or_else(|| {
                sp.rooms.iter().find(|r| {
                    plan_notes.iter().any(|(_, n, nat, _)| cadaddy_geom::point_in_polygon(&on_plan, *nat) && super::bspec::names_room(r, n))
                })
            })
            .map(|r| r.name.clone());
        ceilings.push((CeilingArea { level: level.name.clone(), room, outline: moved(&q), height_ft: h, grid }, q));
    }
    // Soffits: hatched areas, at the note inside them (else a foot under the ceiling around).
    let mut soffits = Vec::new();
    for (q, _) in &hatches {
        let note = notes.iter().find(|(_, t, at, _)| {
            cadaddy_geom::point_in_polygon(q, *at) && (height_tag(t).is_some() || feet_inches(&t.to_ascii_uppercase()).is_some())
        });
        let around =
            ceilings.iter().find(|(_, o)| q.iter().all(|p| cadaddy_geom::point_in_polygon(o, *p) || o.contains(p))).map(|(c, _)| c.height_ft);
        let h = note
            .and_then(|(_, t, _, _)| feet_inches(&t.to_ascii_uppercase()))
            .map(|i| i / 12.0)
            .or(around.map(|a| a - 1.0))
            .unwrap_or(level.ceiling_ft - 1.0);
        soffits.push(Soffit { level: level.name.clone(), outline: moved(q), height_ft: h });
    }
    // Fixtures and diffusers, centred in their grid tile.
    let mut items = Vec::new();
    for e in ents.iter().take(MAX_ITEMS) {
        let EntityKind::Insert(ins) = &e.kind else { continue };
        let on_layer = sp.role(&e.common.layer).is_some_and(|r| matches!(r.role, Role::Fixture | Role::Device)) || {
            let up = e.common.layer.to_ascii_uppercase();
            up.contains("LITE") || up.contains("LIGHT")
        };
        let Some(kind) = item_kind(&ins.block, on_layer) else { continue };
        let b = cadaddy_doc::entity_bounds(d, e, 0);
        if b.is_empty() {
            continue;
        }
        let mut c = b.center();
        let in_area = ceilings.iter().find(|(_, q)| cadaddy_geom::point_in_polygon(q, c));
        if let Some(g) = in_area.and_then(|(a, _)| a.grid) {
            let k = upf / 12.0;
            let u = Vec2::from_angle(g.angle_deg.to_radians());
            let n = Vec2::new(-u.y, u.x);
            let o = Vec2::new(g.origin[0] - offset.x, g.origin[1] - offset.y);
            let (su, sn) = (g.spacing_in[0] * k, g.spacing_in[1] * k);
            let (cu, cn) = ((c - o).dot(u), (c - o).dot(n));
            let snap = |v: f64, s: f64| ((v / s - 0.5).round() + 0.5) * s;
            c = o + u * snap(cu, su) + n * snap(cn, sn);
        }
        let k = upf / 12.0;
        let r8 = |v: f64| (v / k * 8.0).round() / 8.0;
        items.push(CeilingItem {
            kind,
            block: ins.block.clone(),
            level: level.name.clone(),
            at: [c.x + offset.x, c.y + offset.y],
            size_in: [r8(b.width()), r8(b.height())],
            rotation_deg: ins.rotation.to_degrees(),
        });
    }
    let apply = bool_or(p, "apply", false);
    let result = json!({
        "level": level.name,
        "offset": [offset.x, offset.y],
        "ceilings": ceilings.iter().map(|(c, _)| serde_json::to_value(c).unwrap_or(Value::Null)).collect::<Vec<_>>(),
        "soffits": serde_json::to_value(&soffits).unwrap_or(Value::Null),
        "items": serde_json::to_value(&items).unwrap_or(Value::Null),
        "applied": apply,
    });
    if apply {
        let lv = level.name.clone();
        sp.rcp.ceilings.retain(|c| c.level != lv);
        sp.rcp.soffits.retain(|c| c.level != lv);
        sp.rcp.items.retain(|c| c.level != lv);
        for (c, _) in &ceilings {
            if let Some(r) = c.room.as_ref().and_then(|n| sp.rooms.iter_mut().find(|r| &r.name == n)) {
                r.ceiling_ft = Some(c.height_ft);
            }
        }
        sp.rcp.ceilings.extend(ceilings.into_iter().map(|(c, _)| c));
        sp.rcp.soffits.extend(soffits);
        sp.rcp.items.extend(items);
        s.doc_mut()?.building_spec = Some(sp.to_json());
    }
    Ok(result)
}

// ---------------- building ----------------

/// The pieces of the line through `p` along `u` that lie inside the polygon (even-odd).
fn clip_line(poly: &[Vec2], p: Vec2, u: Vec2) -> Vec<(Vec2, Vec2)> {
    let mut ts: Vec<f64> = Vec::new();
    for (a, b) in poly.iter().zip(poly.iter().cycle().skip(1)) {
        let e = *b - *a;
        let den = u.cross(e);
        if den.abs() < 1e-12 {
            continue;
        }
        let t = (*a - p).cross(e) / den;
        let s = (*a - p).cross(u) / den;
        if (0.0..1.0).contains(&s) {
            ts.push(t);
        }
    }
    ts.sort_by(f64::total_cmp);
    ts.chunks(2)
        .filter_map(|c| match c {
            [t0, t1] if t1 - t0 > 1e-9 => Some((p + u * *t0, p + u * *t1)),
            _ => None,
        })
        .collect()
}

/// A flat polygon at `z`, facing down.
fn panel(poly: &[Vec2], z: f64) -> Option<Mesh> {
    let tris = super::bspec::ear_clip(poly);
    if tris.is_empty() {
        return None;
    }
    let vertices = poly.iter().map(|q| Vec3::new(q.x, q.y, z)).collect();
    let id = |i: usize| u32::try_from(i).unwrap_or(0);
    let faces = tris.iter().map(|t| MeshFace { v: vec![id(t[0]), id(t[2]), id(t[1])], hidden: 0 }).collect();
    Some(Mesh { vertices, faces, kind: MeshKind::Polyface })
}

/// The RCP's ceilings, grids, soffits and items into model space (sheet units). On a level with
/// RCP ceilings they replace the level's and the rooms' ceilings. The counts.
pub(crate) fn build(d: &mut Drawing, sp: &BuildingSpec) -> Value {
    let upf = if sp.units_per_foot.is_finite() && sp.units_per_foot > 0.0 { sp.units_per_foot } else { 12.0 };
    let k = upf / 12.0;
    let levels: Vec<&str> = sp.rcp.ceilings.iter().map(|c| c.level.as_str()).collect();
    let replaced: Vec<Handle> = d
        .model
        .iter()
        .filter(|e| super::wallgen::made_as(e).as_deref() == Some("ceiling"))
        .filter(|e| super::bspec::made_level(e).is_some_and(|l| levels.iter().any(|x| x.eq_ignore_ascii_case(&l))))
        .map(|e| e.handle)
        .collect();
    for h in &replaced {
        d.model.remove(*h);
    }
    for (layer, color) in [("3D-Ceiling-Grid", 252), ("3D-Lights", 2), ("3D-Diffusers", 251)] {
        if d.layer(layer).is_none() {
            d.layers.push(cadaddy_doc::Layer { name: layer.into(), color: cadaddy_doc::color::Color::Index(color), ..cadaddy_doc::Layer::default() });
        }
    }
    let tag = |part: &str, level: &str, extra: String| {
        let t = |code: i32, v: String| RawTag { code, value: v };
        vec![
            t(1001, super::wallgen::APP.into()),
            t(1000, part.into()),
            t(1000, level.chars().take(255).collect()),
            t(1000, Handle(0).hex()),
            t(1000, extra),
        ]
    };
    let add = |d: &mut Drawing, layer: &str, xdata: Vec<RawTag>, m: Mesh| {
        !m.faces.is_empty() && d.add(&Space::Model, Common { layer: layer.into(), xdata, ..Common::default() }, EntityKind::Mesh(m)).is_ok()
    };
    let (mut ceilings, mut grids, mut soffits, mut lights, mut diffusers) = (0, 0, 0, 0, 0);
    for c in sp.rcp.ceilings.iter().take(MAX_ITEMS) {
        let Some(level) = sp.level(&c.level) else { continue };
        let z = (level.elevation_ft + c.height_ft) * upf;
        let poly: Vec<Vec2> = c.outline.iter().map(|p| Vec2::new(p[0], p[1])).collect();
        let extra = c.room.as_ref().map_or(String::new(), |r| format!("room:{r}"));
        if let Some(m) = panel(&poly, z)
            && add(d, "3D-Ceilings", tag("ceiling", &level.name, extra), m)
        {
            ceilings += 1;
        }
        if let Some(g) = c.grid {
            // Tees 1" wide and 1½" deep hanging from the ceiling, both ways across the area.
            let u = Vec2::from_angle(g.angle_deg.to_radians());
            let n = Vec2::new(-u.y, u.x);
            let o = Vec2::new(g.origin[0], g.origin[1]);
            let mut m = super::wallgen::empty_mesh();
            for (dir, across, step) in [(u, n, g.spacing_in[1] * k), (n, u, g.spacing_in[0] * k)] {
                let offs: Vec<f64> = poly.iter().map(|p| (*p - o).dot(across)).collect();
                let (lo, hi) = offs.iter().fold((f64::INFINITY, f64::NEG_INFINITY), |(l, h), v| (l.min(*v), h.max(*v)));
                if step.is_nan() || step <= 1e-9 || !lo.is_finite() || (hi - lo) / step > 2_000.0 {
                    continue;
                }
                let mut i = (lo / step).ceil();
                while i * step <= hi {
                    for (a, b) in clip_line(&poly, o + across * (i * step), dir) {
                        super::wallgen::add_box(&mut m, a, b, k, z - 1.5 * k, z);
                    }
                    i += 1.0;
                }
            }
            if add(d, "3D-Ceiling-Grid", tag("grid", &level.name, String::new()), m) {
                grids += 1;
            }
        }
    }
    for sf in sp.rcp.soffits.iter().take(MAX_ITEMS) {
        let Some(level) = sp.level(&sf.level) else { continue };
        let poly: Vec<Vec2> = sf.outline.iter().map(|p| Vec2::new(p[0], p[1])).collect();
        let z = (level.elevation_ft + sf.height_ft) * upf;
        let top = sp.rcp.ceilings.iter().filter(|c| c.level == sf.level).map(|c| c.height_ft).fold(level.ceiling_ft, f64::max);
        // Its underside and the drop round its edge up to the ceiling.
        if let Some(m) = super::bspec::prism(&poly, z, (level.elevation_ft + top) * upf)
            && add(d, "3D-Ceilings", tag("soffit", &level.name, String::new()), m)
        {
            soffits += 1;
        }
    }
    for it in sp.rcp.items.iter().take(MAX_ITEMS) {
        let Some(level) = sp.level(&it.level) else { continue };
        let at = Vec2::new(it.at[0], it.at[1]);
        let h = sp
            .rcp
            .ceilings
            .iter()
            .find(|c| c.level == it.level && cadaddy_geom::point_in_polygon(&c.outline.iter().map(|p| Vec2::new(p[0], p[1])).collect::<Vec<_>>(), at))
            .map_or(level.ceiling_ft, |c| c.height_ft);
        let z = (level.elevation_ft + h) * upf;
        let (w, dpt) = (it.size_in[0] * k, it.size_in[1] * k);
        let a = at - Vec2::new(w / 2.0, 0.0);
        let b = at + Vec2::new(w / 2.0, 0.0);
        let mut m = super::wallgen::empty_mesh();
        let depth = if it.kind == ItemKind::Fixture { 4.0 * k } else { 2.0 * k };
        super::wallgen::add_box(&mut m, a, b, dpt, z - depth, z);
        let (layer, part) = if it.kind == ItemKind::Fixture { ("3D-Lights", "light") } else { ("3D-Diffusers", "diffuser") };
        if add(d, layer, tag(part, &level.name, format!("block:{}", it.block)), m) {
            if it.kind == ItemKind::Fixture {
                lights += 1;
            } else {
                diffusers += 1;
            }
        }
    }
    json!({ "rcpCeilings": ceilings, "grids": grids, "soffits": soffits, "lights": lights, "diffusers": diffusers, "replacedCeilings": replaced.len() })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tags_kinds_and_spacing() {
        assert_eq!(height_tag("9'-0\" A.F.F."), Some(9.0));
        assert_eq!(height_tag("CLG 8'-6\""), Some(8.5));
        assert_eq!(height_tag("9'-0\""), None);
        assert_eq!(item_kind("2X4 TROFFER", false), Some(ItemKind::Fixture));
        assert_eq!(item_kind("SD-1", false), Some(ItemKind::Diffuser));
        assert_eq!(item_kind("CHAIR", false), None);
        assert_eq!(item_kind("A", true), Some(ItemKind::Fixture));
        assert_eq!(spacing(&[0.0, 24.0, 48.0, 72.0]), Some(24.0));
        assert_eq!(spacing(&[0.0, 24.0]), None);
        let poly = [Vec2::new(0.0, 0.0), Vec2::new(10.0, 0.0), Vec2::new(10.0, 10.0), Vec2::new(0.0, 10.0)];
        let pieces = clip_line(&poly, Vec2::new(-5.0, 4.0), Vec2::new(1.0, 0.0));
        assert_eq!(pieces.len(), 1);
        assert!((pieces[0].0.x - 0.0).abs() < 1e-9 && (pieces[0].1.x - 10.0).abs() < 1e-9);
    }
}
