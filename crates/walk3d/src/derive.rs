//! Drawing + rules → 3D scene.

use crate::math::{V3, v3};
use crate::rules::{LayerKind, Level, Rules};
use cadcraft_color::{Color, Rgb};
use cadcraft_doc::geom::{Polyline, Vec2};
use cadcraft_doc::{Drawing, Entity, EntityKind, Handle};

/// Arcs are cut into chords no further than this from the curve, in drawing units.
const ARC_TOL: f64 = 0.5;
/// Walls this short (feet) are dropped: PDF-converted linework is full of dust.
const MIN_WALL_FT: f64 = 0.05;
/// A wall-mounted device snaps to a wall this close (feet).
const SNAP_FT: f64 = 2.0;
/// Hard cap on wall segments taken from one drawing (hostile input).
const MAX_SEGS: usize = 2_000_000;

/// One triangle-list vertex: position (feet), face normal, sRGB colour.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Vertex {
    pub p: [f32; 3],
    pub n: [f32; 3],
    pub c: [u8; 4],
}

/// A wall segment in plan on one level (collision).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Seg {
    pub a: Vec2,
    pub b: Vec2,
    pub level: usize,
}

#[derive(Clone, Debug, PartialEq)]
pub struct LevelInfo {
    pub name: String,
    pub elevation_ft: f64,
    pub height_ft: f64,
    pub ceiling_ft: f64,
    /// Plan extents of this level's walls, feet.
    pub min: Vec2,
    pub max: Vec2,
}

#[derive(Clone, Debug, PartialEq)]
pub struct DeviceInfo {
    pub handle: Handle,
    pub block: String,
    pub level: usize,
    /// Centre of the device box, feet.
    pub pos: V3,
    /// Mounting height above the floor, feet.
    pub aff_ft: f64,
    pub ceiling: bool,
    pub attrs: Vec<(String, String)>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Scene {
    /// Triangle list.
    pub tris: Vec<Vertex>,
    /// Owner of each triangle (`tris.len() / 3` entries).
    pub owners: Vec<Handle>,
    /// Edge overlay, a line list.
    pub edges: Vec<Vertex>,
    pub walls: Vec<Seg>,
    pub devices: Vec<DeviceInfo>,
    pub levels: Vec<LevelInfo>,
    /// Entities the rules placed nowhere (outside every level's region).
    pub unplaced: usize,
}

const WALL: Rgb = Rgb(222, 218, 210);
const FLOOR: Rgb = Rgb(150, 146, 140);
const EDGE: Rgb = Rgb(40, 40, 44);

/// Where one level's plan sits: sheet region + base point.
struct Place {
    region: Option<[f64; 4]>,
    base: Vec2,
}

impl Place {
    fn contains(&self, p: Vec2) -> bool {
        match self.region {
            Some([x0, y0, x1, y1]) => p.x >= x0.min(x1) && p.x <= x0.max(x1) && p.y >= y0.min(y1) && p.y <= y0.max(y1),
            None => true,
        }
    }
}

/// Build the 3D scene. Never fails: what the rules don't place is counted in [`Scene::unplaced`].
pub fn derive(d: &Drawing, rules: &Rules) -> Scene {
    let upf = if rules.units_per_foot.is_finite() && rules.units_per_foot > 0.0 { rules.units_per_foot } else { 12.0 };
    let places: Vec<Place> = rules.levels.iter().map(|l| Place { region: l.region, base: base_point(d, rules, l) }).collect();
    let mut s = Scene::default();
    let mut wall_pieces: Vec<(Vec<Vec2>, usize, f64, Handle)> = Vec::new();

    for e in d.model.iter() {
        let Some(kind) = rules.layer(&e.common.layer) else { continue };
        if matches!(kind, LayerKind::Device) || matches!(e.kind, EntityKind::Insert(_)) {
            continue;
        }
        let Some(pts) = plan_points(e) else { continue };
        let Some(lv) = pts.first().and_then(|p| places.iter().position(|pl| pl.contains(*p))) else {
            s.unplaced += 1;
            continue;
        };
        let (Some(level), Some(place)) = (rules.levels.get(lv), places.get(lv)) else { continue };
        let world: Vec<Vec2> = pts.iter().map(|p| to_world(*p, place.base, upf)).collect();
        match kind {
            LayerKind::Wall { height_ft } => {
                let h = height_ft.filter(|h| h.is_finite()).unwrap_or(level.height_ft);
                wall_pieces.push((world, lv, h, e.handle));
            }
            LayerKind::Run { height_ft } => {
                let z = level.elevation_ft + height_ft;
                let c = entity_rgb(d, e);
                for w in world.windows(2) {
                    if let [a, b] = w {
                        s.edges.push(vtx(v3(a.x, a.y, z), V3::default(), c));
                        s.edges.push(vtx(v3(b.x, b.y, z), V3::default(), c));
                    }
                }
            }
            LayerKind::Device => {}
        }
    }

    // Level extents from their walls, then floors.
    for (i, l) in rules.levels.iter().enumerate() {
        let mut min = Vec2 { x: f64::INFINITY, y: f64::INFINITY };
        let mut max = Vec2 { x: f64::NEG_INFINITY, y: f64::NEG_INFINITY };
        for (pts, lv, _, _) in &wall_pieces {
            if *lv == i {
                for p in pts {
                    min = Vec2 { x: min.x.min(p.x), y: min.y.min(p.y) };
                    max = Vec2 { x: max.x.max(p.x), y: max.y.max(p.y) };
                }
            }
        }
        if !min.x.is_finite() {
            min = Vec2 { x: 0.0, y: 0.0 };
            max = Vec2 { x: 0.0, y: 0.0 };
        }
        s.levels.push(LevelInfo { name: l.name.clone(), elevation_ft: l.elevation_ft, height_ft: l.height_ft, ceiling_ft: l.ceiling_ft, min, max });
    }
    for li in s.levels.clone() {
        if li.max.x > li.min.x && li.max.y > li.min.y {
            let z = li.elevation_ft;
            let pad = 2.0;
            let (a, b) = (v3(li.min.x - pad, li.min.y - pad, z), v3(li.max.x + pad, li.max.y + pad, z));
            quad(&mut s, [a, v3(b.x, a.y, z), b, v3(a.x, b.y, z)], FLOOR, Handle(0));
        }
    }

    // Walls: one vertical quad per segment, edges along top and bottom and at corners.
    let mut segs = 0usize;
    for (pts, lv, h, handle) in &wall_pieces {
        let Some(level) = s.levels.get(*lv) else { continue };
        let (z0, z1) = (level.elevation_ft, level.elevation_ft + h.max(0.0));
        let n = pts.len();
        for (i, w) in pts.windows(2).enumerate() {
            let [a, b] = w else { continue };
            if (*b - *a).len() < MIN_WALL_FT || segs >= MAX_SEGS {
                continue;
            }
            segs += 1;
            s.walls.push(Seg { a: *a, b: *b, level: *lv });
            quad(&mut s, [v3(a.x, a.y, z0), v3(b.x, b.y, z0), v3(b.x, b.y, z1), v3(a.x, a.y, z1)], WALL, *handle);
            edge(&mut s, v3(a.x, a.y, z1), v3(b.x, b.y, z1));
            edge(&mut s, v3(a.x, a.y, z0), v3(b.x, b.y, z0));
            let corner = |at: usize| -> bool {
                if at == 0 || at + 1 >= n {
                    return true;
                }
                match (pts.get(at - 1), pts.get(at), pts.get(at + 1)) {
                    (Some(p), Some(q), Some(r)) => {
                        let (u, v) = (*q - *p, *r - *q);
                        let c = (u.x * v.x + u.y * v.y) / (u.len() * v.len()).max(1e-12);
                        c < 0.9
                    }
                    _ => true,
                }
            };
            if corner(i) {
                edge(&mut s, v3(a.x, a.y, z0), v3(a.x, a.y, z1));
            }
            if i + 2 == n {
                edge(&mut s, v3(b.x, b.y, z0), v3(b.x, b.y, z1));
            }
        }
    }

    // Devices.
    for e in d.model.iter() {
        let EntityKind::Insert(ins) = &e.kind else { continue };
        if rules.layer(&e.common.layer) != Some(&LayerKind::Device) {
            continue;
        }
        let attrs: Vec<(String, String)> = ins.attribs.iter().map(|a| (a.tag.clone(), a.text.value.clone())).collect();
        let attr = |tag: &str| attrs.iter().find(|(t, _)| t.eq_ignore_ascii_case(tag)).map(|(_, v)| v.as_str());
        let at = ins.insert.xy();
        let lv = attr(&rules.devices.level_attr)
            .and_then(|v| rules.levels.iter().position(|l| l.attr.as_deref() == Some(v)))
            .or_else(|| places.iter().position(|p| p.contains(at)));
        let Some(lv) = lv else {
            s.unplaced += 1;
            continue;
        };
        let (Some(level), Some(place)) = (s.levels.get(lv).cloned(), places.get(lv)) else { continue };
        let ty = attr(&rules.devices.type_attr).unwrap_or("");
        let mount = rules.mount(ty).clone();
        let aff_override = attr(&rules.devices.aff_attr).and_then(|v| v.trim().trim_end_matches('"').parse::<f64>().ok()).filter(|v| v.is_finite());
        let p = to_world(at, place.base, upf);
        let c = entity_rgb(d, e);
        let [w, dp, h] = mount.size_ft.map(|v| if v.is_finite() { v.clamp(0.01, 20.0) } else { 0.5 });
        let (pos, aff, dir) = if mount.ceiling && aff_override.is_none() {
            let aff = level.ceiling_ft - h / 2.0;
            (v3(p.x, p.y, level.elevation_ft + aff), aff, Vec2 { x: 1.0, y: 0.0 })
        } else {
            let aff = aff_override.map(|i| i / 12.0).unwrap_or(mount.aff_ft);
            // Snap onto the nearest wall face of this level, facing out of it.
            let mut best: Option<(f64, Vec2, Vec2)> = None;
            for sg in s.walls.iter().filter(|sg| sg.level == lv) {
                let (q, dist) = closest(sg.a, sg.b, p);
                if dist < SNAP_FT && best.is_none_or(|(bd, _, _)| dist < bd) {
                    let t = (sg.b - sg.a) * (1.0 / (sg.b - sg.a).len().max(1e-12));
                    let mut nrm = Vec2 { x: -t.y, y: t.x };
                    if (p - q).x * nrm.x + (p - q).y * nrm.y < 0.0 {
                        nrm = nrm * -1.0;
                    }
                    best = Some((dist, q + nrm * (dp / 2.0), t));
                }
            }
            match best {
                Some((_, q, t)) => (v3(q.x, q.y, level.elevation_ft + aff), aff, t),
                None => (v3(p.x, p.y, level.elevation_ft + aff), aff, Vec2 { x: 1.0, y: 0.0 }),
            }
        };
        let across = if mount.ceiling { w } else { dp };
        boxed(&mut s, pos, dir, w, across, h, c, e.handle);
        s.devices.push(DeviceInfo {
            handle: e.handle,
            block: ins.block.clone(),
            level: lv,
            pos,
            aff_ft: aff,
            ceiling: mount.ceiling && aff_override.is_none(),
            attrs,
        });
    }
    s
}

/// The sheet point mapped to the building origin for a level.
fn base_point(d: &Drawing, rules: &Rules, l: &Level) -> Vec2 {
    if let Some([x, y]) = l.base {
        return Vec2 { x, y };
    }
    let place = Place { region: l.region, base: Vec2 { x: 0.0, y: 0.0 } };
    let mut min = Vec2 { x: f64::INFINITY, y: f64::INFINITY };
    for e in d.model.iter() {
        if !matches!(rules.layer(&e.common.layer), Some(LayerKind::Wall { .. })) {
            continue;
        }
        let Some(pts) = plan_points(e) else { continue };
        if !pts.first().is_some_and(|p| place.contains(*p)) {
            continue;
        }
        for p in pts {
            min = Vec2 { x: min.x.min(p.x), y: min.y.min(p.y) };
        }
    }
    if min.x.is_finite() { min } else { Vec2 { x: 0.0, y: 0.0 } }
}

/// Lines and polylines as plan points (arcs chorded); none for anything else.
fn plan_points(e: &Entity) -> Option<Vec<Vec2>> {
    let pts = match &e.kind {
        EntityKind::Line(l) => vec![l.a.xy(), l.b.xy()],
        EntityKind::LwPolyline(p) => Polyline { vertices: p.vertices.clone(), closed: p.closed }.tessellate(ARC_TOL),
        EntityKind::Polyline3d(p) => {
            let mut v: Vec<Vec2> = p.points.iter().map(|q| q.xy()).collect();
            if p.closed
                && let Some(f) = v.first().copied()
            {
                v.push(f);
            }
            v
        }
        _ => return None,
    };
    let pts: Vec<Vec2> = pts.into_iter().filter(|p| p.x.is_finite() && p.y.is_finite()).collect();
    if pts.len() >= 2 { Some(pts) } else { None }
}

fn to_world(p: Vec2, base: Vec2, upf: f64) -> Vec2 {
    Vec2 { x: (p.x - base.x) / upf, y: (p.y - base.y) / upf }
}

fn entity_rgb(d: &Drawing, e: &Entity) -> Rgb {
    let layer = d.layer(&e.common.layer).map(|l| l.color).unwrap_or(Color::Index(7));
    let c = e.common.color.resolve(layer, Color::Index(7));
    // White/black ACI 7 reads as nothing in a lit scene; give devices a visible colour.
    if c == Rgb(255, 255, 255) || c == Rgb(0, 0, 0) { Rgb(230, 120, 30) } else { c }
}

fn vtx(p: V3, n: V3, c: Rgb) -> Vertex {
    Vertex { p: p.f32(), n: n.f32(), c: [c.0, c.1, c.2, 255] }
}

fn edge(s: &mut Scene, a: V3, b: V3) {
    s.edges.push(vtx(a, V3::default(), EDGE));
    s.edges.push(vtx(b, V3::default(), EDGE));
}

/// Two triangles for a planar quad (corners in order).
fn quad(s: &mut Scene, q: [V3; 4], c: Rgb, owner: Handle) {
    let [a, b, cc, d] = q;
    let n = (b - a).cross(d - a).norm();
    for p in [a, b, cc, a, cc, d] {
        s.tris.push(vtx(p, n, c));
    }
    s.owners.push(owner);
    s.owners.push(owner);
}

/// A box centred on `c`, `along` wide in direction `dir`, `across` deep, `h` high.
#[allow(clippy::too_many_arguments)]
fn boxed(s: &mut Scene, c: V3, dir: Vec2, along: f64, across: f64, h: f64, col: Rgb, owner: Handle) {
    let t = v3(dir.x, dir.y, 0.0).norm() * (along / 2.0);
    let n = v3(-dir.y, dir.x, 0.0).norm() * (across / 2.0);
    let z = v3(0.0, 0.0, h / 2.0);
    let p = |i: f64, j: f64, k: f64| c + t * i + n * j + z * k;
    let faces = [
        [p(-1., -1., -1.), p(1., -1., -1.), p(1., 1., -1.), p(-1., 1., -1.)],
        [p(-1., -1., 1.), p(-1., 1., 1.), p(1., 1., 1.), p(1., -1., 1.)],
        [p(-1., -1., -1.), p(-1., -1., 1.), p(1., -1., 1.), p(1., -1., -1.)],
        [p(-1., 1., -1.), p(1., 1., -1.), p(1., 1., 1.), p(-1., 1., 1.)],
        [p(-1., -1., -1.), p(-1., 1., -1.), p(-1., 1., 1.), p(-1., -1., 1.)],
        [p(1., -1., -1.), p(1., -1., 1.), p(1., 1., 1.), p(1., 1., -1.)],
    ];
    for f in faces {
        quad(s, f, col, owner);
    }
}

/// Closest point on segment ab to p, and its distance.
pub(crate) fn closest(a: Vec2, b: Vec2, p: Vec2) -> (Vec2, f64) {
    let ab = b - a;
    let l2 = ab.x * ab.x + ab.y * ab.y;
    let t = if l2 > 1e-18 { (((p - a).x * ab.x + (p - a).y * ab.y) / l2).clamp(0.0, 1.0) } else { 0.0 };
    let q = a + ab * t;
    (q, (p - q).len())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rules::{DeviceRules, LayerRule, Mount};
    use cadcraft_doc::{Insert, Line};
    use cadcraft_geom::Vec3;

    fn add(d: &mut Drawing, layer: &str, kind: EntityKind) {
        let h = Handle(d.handseed.max(1));
        d.handseed = h.0 + 1;
        let mut e = Entity::new(h, kind);
        e.common.layer = layer.into();
        d.model.push(e);
    }

    fn line(x0: f64, y0: f64, x1: f64, y1: f64) -> EntityKind {
        EntityKind::Line(Line { a: Vec3 { x: x0, y: y0, z: 0.0 }, b: Vec3 { x: x1, y: y1, z: 0.0 } })
    }

    fn device(x: f64, y: f64, ty: &str, floor: &str) -> EntityKind {
        let attr = |tag: &str, v: &str| serde_json::json!({"tag": tag, "text": {"insert": Vec3 { x, y, z: 0.0 }, "height": 6.0, "value": v}});
        let json =
            serde_json::json!({"block": format!("E-DEV-{ty}"), "insert": Vec3 { x, y, z: 0.0 }, "attribs": [attr("TYPE", ty), attr("FLOOR", floor)]});
        EntityKind::Insert(serde_json::from_value::<Insert>(json).unwrap_or_else(|e| panic!("{e}")))
    }

    /// Two copies of a 10' × 20' room drawn one above the other on the sheet (inches), as
    /// stacked floor plans are; a ceiling light and a receptacle on each.
    fn two_floor_sheet() -> (Drawing, Rules) {
        let mut d = Drawing::default();
        for oy in [0.0, 1000.0] {
            add(&mut d, "A-WALL", line(0.0, oy, 240.0, oy));
            add(&mut d, "A-WALL", line(240.0, oy, 240.0, oy + 120.0));
            add(&mut d, "A-WALL", line(240.0, oy + 120.0, 0.0, oy + 120.0));
            add(&mut d, "A-WALL", line(0.0, oy + 120.0, 0.0, oy));
            add(&mut d, "NOTES", line(10.0, oy + 10.0, 50.0, oy + 10.0));
        }
        add(&mut d, "E-DEVC", device(120.0, 60.0, "A", "3"));
        add(&mut d, "E-DEVC", device(230.0, 60.0, "REC", "3"));
        // Drawn on the lower plan but tagged floor 4: the attribute wins.
        add(&mut d, "E-DEVC", device(120.0, 30.0, "A", "4"));
        let lvl = |name: &str, elev: f64, y0: f64, attr: &str| Level {
            name: name.into(),
            elevation_ft: elev,
            height_ft: 10.0,
            ceiling_ft: 8.0,
            region: Some([-100.0, y0, 400.0, y0 + 500.0]),
            base: None,
            attr: Some(attr.into()),
        };
        let mut devices = DeviceRules::default();
        devices.by_type.insert("REC".into(), Mount { ceiling: false, aff_ft: 1.5, size_ft: [0.3, 0.15, 0.45] });
        let rules = Rules {
            units_per_foot: 12.0,
            levels: vec![lvl("L3", 20.0, -100.0, "3"), lvl("L4", 30.0, 900.0, "4")],
            layers: vec![
                LayerRule { layer: "A-WALL".into(), kind: LayerKind::Wall { height_ft: None } },
                LayerRule { layer: "E-DEV*".into(), kind: LayerKind::Device },
            ],
            devices,
        };
        (d, rules)
    }

    #[test]
    fn stacked_plans_line_up() {
        let (d, rules) = two_floor_sheet();
        let s = derive(&d, &rules);
        assert_eq!(s.walls.len(), 8);
        assert_eq!(s.levels.len(), 2);
        for l in &s.levels {
            assert!((l.min.x - 0.0).abs() < 1e-9 && (l.min.y - 0.0).abs() < 1e-9, "{l:?}");
            assert!((l.max.x - 20.0).abs() < 1e-9 && (l.max.y - 10.0).abs() < 1e-9, "{l:?}");
        }
        // Walls stand from the floor to the next floor.
        let zs: Vec<f32> = s.tris.iter().map(|v| v.p[2]).collect();
        assert!(zs.contains(&20.0) && zs.contains(&30.0) && zs.contains(&40.0));
        assert_eq!(s.owners.len() * 3, s.tris.len());
    }

    #[test]
    fn devices_mount_by_type_and_level_attribute() {
        let (d, rules) = two_floor_sheet();
        let s = derive(&d, &rules);
        assert_eq!(s.devices.len(), 3);
        let light = s.devices.iter().find(|v| v.block == "E-DEV-A" && v.level == 0).unwrap_or_else(|| panic!("no light"));
        assert!(light.ceiling);
        assert!((light.pos.z - (20.0 + 8.0 - 0.125)).abs() < 1e-9, "{:?}", light.pos);
        assert!((light.pos.x - 10.0).abs() < 1e-9 && (light.pos.y - 5.0).abs() < 1e-9);
        let rec = s.devices.iter().find(|v| v.block == "E-DEV-REC").unwrap_or_else(|| panic!("no rec"));
        assert!(!rec.ceiling);
        assert!((rec.pos.z - 21.5).abs() < 1e-9);
        // Snapped onto the east wall's inside face (x = 20 minus half the box depth).
        assert!((rec.pos.x - (20.0 - 0.075)).abs() < 1e-9, "{:?}", rec.pos);
        let up = s.devices.iter().find(|v| v.level == 1).unwrap_or_else(|| panic!("no floor 4 device"));
        assert!((up.pos.z - (30.0 + 8.0 - 0.125)).abs() < 1e-9);
    }

    #[test]
    fn unmatched_layers_are_ignored_and_outside_regions_counted() {
        let (mut d, rules) = two_floor_sheet();
        add(&mut d, "A-WALL", line(5000.0, 5000.0, 5100.0, 5000.0));
        let s = derive(&d, &rules);
        assert_eq!(s.walls.len(), 8);
        assert_eq!(s.unplaced, 1);
    }

    #[test]
    fn hostile_numbers_do_not_panic() {
        let (mut d, mut rules) = two_floor_sheet();
        add(&mut d, "A-WALL", line(f64::NAN, 0.0, f64::INFINITY, 1.0));
        rules.units_per_foot = 0.0;
        if let Some(m) = rules.devices.by_type.get_mut("REC") {
            m.size_ft = [f64::NAN, -1.0, 1e300];
        }
        let s = derive(&d, &rules);
        assert!(s.tris.iter().all(|v| v.p.iter().all(|c| c.is_finite())));
    }
}
