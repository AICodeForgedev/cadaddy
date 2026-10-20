//! Drawing + rules → 3D scene.

use crate::math::{V3, v3};
use crate::rules::{LayerKind, Level, Rules};
use cadaddy_color::{Color, Rgb};
use cadaddy_doc::geom::{Polyline, Vec2};
use cadaddy_doc::{Drawing, Entity, EntityKind, Handle};

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
    /// The sheet point (drawing units) this level's plan places at the building origin: a scene
    /// point `p` (feet) is at `base + p * units_per_foot` on the sheet.
    pub base: Vec2,
}

/// What a triangle of the scene is part of.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Part {
    #[default]
    Wall,
    Floor,
    Ceiling,
    /// Window glass.
    Glass,
    /// An open door leaf.
    Door,
    /// A device box.
    Device,
    /// A stair's steps and landings, or a ramp.
    Stair,
}

/// Which drawing of the set a triangle or device came from (see [`Rules::sheets`]) and its
/// entity there. Floors and other generated faces carry handle 0.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Owner {
    pub src: u16,
    pub handle: Handle,
}

#[derive(Clone, Debug, PartialEq)]
pub struct DeviceInfo {
    pub handle: Handle,
    /// Source drawing (see [`Owner`]).
    pub src: u16,
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
    pub owners: Vec<Owner>,
    /// What each triangle is part of (as `owners`).
    pub parts: Vec<Part>,
    /// What the faces added next are part of.
    pub(crate) part: Part,
    /// Drawing units per foot of the sheets (see [`LevelInfo::base`]).
    pub units_per_foot: f64,
    /// Edge overlay, a line list.
    pub edges: Vec<Vertex>,
    pub walls: Vec<Seg>,
    pub devices: Vec<DeviceInfo>,
    pub levels: Vec<LevelInfo>,
    /// Entities the rules placed nowhere (outside every level's region).
    pub unplaced: usize,
    /// Wall face pairs closed into solid walls (top and end caps).
    pub solid_walls: usize,
    /// Openings filled ([`crate::Openings`]): doors, windows and cased openings.
    pub doors: usize,
    pub windows: usize,
    pub cased: usize,
    /// Door leaves, which open and close ([`Scene::toggle_door`]).
    pub door_leaves: Vec<crate::DoorLeaf>,
    /// Stairs, ramps and landings Walk climbs (feet).
    pub slopes: Vec<crate::stairs::Slope>,
}

/// A room name on a level's plan (TEXT or MTEXT), where it is in the building.
#[derive(Clone, Debug, PartialEq)]
pub struct Room {
    pub name: String,
    pub level: usize,
    pub at: Vec2,
}

/// Most room names collected.
const MAX_ROOMS: usize = 5_000;

/// The room names on the plans: text placed on a level, with letters, at most 40 characters.
pub fn rooms(sources: &[&Drawing], rules: &Rules) -> Vec<Room> {
    let upf = if rules.units_per_foot.is_finite() && rules.units_per_foot > 0.0 { rules.units_per_foot } else { 12.0 };
    let empty = Drawing::default();
    let src_of: Vec<usize> = (0..rules.levels.len()).map(|i| rules.level_source(i)).collect();
    let drawing_of = |lv: usize| -> &Drawing { src_of.get(lv).and_then(|s| sources.get(*s)).copied().unwrap_or(&empty) };
    let places: Vec<Place> =
        rules.levels.iter().enumerate().map(|(i, l)| Place { region: l.region, base: base_point(drawing_of(i), rules, l) }).collect();
    let mut out: Vec<Room> = Vec::new();
    for (lv, place) in places.iter().enumerate() {
        let d = drawing_of(lv);
        for e in d.model.iter() {
            let (text, at) = match &e.kind {
                EntityKind::Text(t) => (t.value.clone(), t.justify_point()),
                EntityKind::MText(t) => (cadaddy_fonts_plain(&t.contents), t.insert.xy()),
                _ => continue,
            };
            let name = text.split_whitespace().collect::<Vec<_>>().join(" ");
            if !room_name(&name) || !place.contains(at) {
                continue;
            }
            if out.iter().any(|r| r.level == lv && r.name == name) {
                continue;
            }
            out.push(Room { name, level: lv, at: to_world(at, place.base, upf) });
            if out.len() >= MAX_ROOMS {
                return out;
            }
        }
    }
    out
}

/// Whether `name` reads as a room name ("OFFICE 101", "Storage", "101 Lobby"), not a label or
/// a note ("PANEL LP-1 120/208V 3PH": battle test 0.7.8 #19): 2 to 40 characters with a letter,
/// at most four words, none of `/ : = " # % @ ;`, and digits in at most one word, the first or
/// the last (the room number).
fn room_name(name: &str) -> bool {
    let n = name.chars().count();
    if !(2..=40).contains(&n) || !name.chars().any(char::is_alphabetic) || name.contains(['/', ':', '=', '"', '#', '%', '@', ';']) {
        return false;
    }
    let words: Vec<&str> = name.split_whitespace().collect();
    let numbered: Vec<usize> = words.iter().enumerate().filter(|(_, w)| w.chars().any(|c| c.is_ascii_digit())).map(|(i, _)| i).collect();
    let number_at_an_end = match numbered[..] {
        [] => true,
        [i] => i == 0 || i + 1 == words.len(),
        _ => false,
    };
    words.len() <= 4 && number_at_an_end
}

/// MTEXT contents as plain text (format codes dropped).
fn cadaddy_fonts_plain(s: &str) -> String {
    let mut out = String::new();
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '{' | '}' => {}
            '\\' => match chars.next() {
                Some('P' | 'N') => out.push(' '),
                Some('\\') => out.push('\\'),
                Some('{') => out.push('{'),
                Some('}') => out.push('}'),
                Some(_) => {
                    // A code with a value runs to `;`.
                    for x in chars.by_ref() {
                        if x == ';' {
                            break;
                        }
                    }
                }
                None => {}
            },
            c => out.push(c),
        }
    }
    out
}

pub(crate) const WALL: Rgb = Rgb(222, 218, 210);
pub(crate) const WALL_TOP: Rgb = Rgb(118, 112, 104);
const FLOOR: Rgb = Rgb(150, 146, 140);
const CEILING: Rgb = Rgb(236, 234, 228);
const EDGE: Rgb = Rgb(40, 40, 44);

/// Floor-to-floor walls end this far (feet) below the next floor (its structure).
const FLOOR_GAP_FT: f64 = 0.1;
/// Two parallel faces this far apart (feet) are the two sides of one wall: 3½" partitions up to
/// thick exterior and rated walls.
const WALL_THICK_FT: (f64, f64) = (0.2, 1.6);
/// Faces count as parallel within this angle (radians).
const PARALLEL: f64 = 0.035;
/// Wall pairing is skipped above this many faces on one level (hostile input; quadratic).
const MAX_PAIR_FACES: usize = 8_000;

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

/// Build the 3D scene from one drawing. Never fails: what the rules don't place is counted in
/// [`Scene::unplaced`].
pub fn derive(d: &Drawing, rules: &Rules) -> Scene {
    derive_set(&[d], rules)
}

/// Build the 3D scene from a set of drawings: `sources[0]` is the drawing the rules were opened
/// with, then one per [`Rules::sheets`] entry. A level whose sheet is missing from the set is
/// empty.
pub fn derive_set(sources: &[&Drawing], rules: &Rules) -> Scene {
    let upf = if rules.units_per_foot.is_finite() && rules.units_per_foot > 0.0 { rules.units_per_foot } else { 12.0 };
    let empty = Drawing::default();
    let src_of: Vec<usize> = (0..rules.levels.len()).map(|i| rules.level_source(i)).collect();
    let drawing_of = |lv: usize| -> &Drawing { src_of.get(lv).and_then(|s| sources.get(*s)).copied().unwrap_or(&empty) };
    let places: Vec<Place> =
        rules.levels.iter().enumerate().map(|(i, l)| Place { region: l.region, base: base_point(drawing_of(i), rules, l) }).collect();
    // The level an entity of source `src` at plan point `p` belongs to.
    let level_at = |src: usize, p: Vec2| -> Option<usize> {
        (0..places.len()).find(|&i| src_of.get(i) == Some(&src) && places.get(i).is_some_and(|pl| pl.contains(p)))
    };
    let mut s = Scene { units_per_foot: upf, ..Scene::default() };
    let mut wall_pieces: Vec<(Vec<Vec2>, usize, f64, Owner, bool)> = Vec::new();
    // Curved runs the wall layers dropped: door swings among them (see [`crate::openings`]).
    let mut curves: Vec<(Vec<Vec2>, usize, Owner)> = Vec::new();

    for (src, d) in sources.iter().enumerate() {
        if !src_of.contains(&src) {
            continue;
        }
        let owner = |h: Handle| Owner { src: u16::try_from(src).unwrap_or(u16::MAX), handle: h };
        for e in d.model.iter() {
            let Some(kind) = rules.layer(&e.common.layer) else { continue };
            if matches!(kind, LayerKind::Device) || matches!(e.kind, EntityKind::Insert(_)) {
                continue;
            }
            let Some(pts) = plan_points(e) else { continue };
            let Some(lv) = pts.first().and_then(|p| level_at(src, *p)) else {
                s.unplaced += 1;
                continue;
            };
            let (Some(level), Some(place)) = (rules.levels.get(lv), places.get(lv)) else { continue };
            let world: Vec<Vec2> = pts.iter().map(|p| to_world(*p, place.base, upf)).collect();
            match kind {
                LayerKind::Wall { height_ft, skip_curves, paired_only } => {
                    // Floor-to-floor walls stop just under the next floor, so their dark top caps
                    // show when the levels above are cut away.
                    let h = height_ft.filter(|h| h.is_finite()).unwrap_or(level.height_ft - FLOOR_GAP_FT);
                    if *skip_curves {
                        let (runs, arcs) = split_runs(&world);
                        for run in runs {
                            wall_pieces.push((run, lv, h, owner(e.handle), *paired_only));
                        }
                        curves.extend(arcs.into_iter().map(|c| (c, lv, owner(e.handle))));
                    } else {
                        wall_pieces.push((world, lv, h, owner(e.handle), *paired_only));
                    }
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
    }

    // Level extents from their walls, then floors and ceilings.
    for (i, l) in rules.levels.iter().enumerate() {
        let mut min = Vec2 { x: f64::INFINITY, y: f64::INFINITY };
        let mut max = Vec2 { x: f64::NEG_INFINITY, y: f64::NEG_INFINITY };
        for (pts, lv, _, _, _) in &wall_pieces {
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
        let base = places.get(i).map_or(Vec2 { x: 0.0, y: 0.0 }, |p| p.base);
        s.levels.push(LevelInfo {
            name: l.name.clone(),
            elevation_ft: l.elevation_ft,
            height_ft: l.height_ft,
            ceiling_ft: l.ceiling_ft,
            min,
            max,
            base,
        });
    }
    for li in s.levels.clone() {
        if li.max.x > li.min.x && li.max.y > li.min.y {
            let pad = 2.0;
            let rect = |z: f64| {
                [
                    v3(li.min.x - pad, li.min.y - pad, z),
                    v3(li.max.x + pad, li.min.y - pad, z),
                    v3(li.max.x + pad, li.max.y + pad, z),
                    v3(li.min.x - pad, li.max.y + pad, z),
                ]
            };
            s.part = Part::Floor;
            quad(&mut s, rect(li.elevation_ft), FLOOR, Owner::default());
            // The ceiling faces down (drawn from below; a cut at the ceiling line hides it).
            let [a, b, c, d] = rect(li.elevation_ft + li.ceiling_ft);
            if li.ceiling_ft > 0.0 && li.ceiling_ft < li.height_ft {
                s.part = Part::Ceiling;
                quad(&mut s, [a, d, c, b], CEILING, Owner::default());
            }
        }
    }

    // Walls: the faces of each piece (corners marked for vertical edges), paired-only layers
    // cut down to where a partner face runs alongside, then one vertical quad per face.
    let mut faces: Vec<Face> = Vec::new();
    for (pts, lv, h, owner, paired_only) in &wall_pieces {
        let Some(level) = s.levels.get(*lv) else { continue };
        let (z0, z1) = (level.elevation_ft, level.elevation_ft + h.max(0.0));
        let n = pts.len();
        let corner = |at: usize| -> bool {
            if at == 0 || at + 1 >= n {
                return true;
            }
            match (pts.get(at - 1), pts.get(at), pts.get(at + 1)) {
                (Some(p), Some(q), Some(r)) => {
                    let (u, v) = (*q - *p, *r - *q);
                    (u.x * v.x + u.y * v.y) / (u.len() * v.len()).max(1e-12) < 0.9
                }
                _ => true,
            }
        };
        for (i, w) in pts.windows(2).enumerate() {
            let [a, b] = w else { continue };
            if (*b - *a).len() < MIN_WALL_FT || faces.len() >= MAX_SEGS {
                continue;
            }
            faces.push(Face { a: *a, b: *b, lv: *lv, z0, z1, owner: *owner, paired_only: *paired_only, edge_a: corner(i), edge_b: i + 2 == n });
        }
    }
    let faces = keep_paired(faces);
    // Openings are found on the paired walls before they are built: lines of a window unit inside a
    // window opening are its frame, not wall.
    let face_segs: Vec<Seg> = faces.iter().map(|f| Seg { a: f.a, b: f.b, level: f.lv }).collect();
    let plans = crate::openings::plan(&face_segs, &pair_pieces(&faces), &curves, &rules.openings);
    let faces: Vec<Face> = faces.into_iter().filter(|f| !crate::openings::in_window(&plans, f.lv, f.a, f.b)).collect();
    s.part = Part::Wall;
    for f in &faces {
        let (a, b, z0, z1) = (f.a, f.b, f.z0, f.z1);
        s.walls.push(Seg { a, b, level: f.lv });
        quad(&mut s, [v3(a.x, a.y, z0), v3(b.x, b.y, z0), v3(b.x, b.y, z1), v3(a.x, a.y, z1)], wall_rgb(sources, f.owner), f.owner);
        edge(&mut s, v3(a.x, a.y, z1), v3(b.x, b.y, z1));
        edge(&mut s, v3(a.x, a.y, z0), v3(b.x, b.y, z0));
        if f.edge_a {
            edge(&mut s, v3(a.x, a.y, z0), v3(a.x, a.y, z1));
        }
        if f.edge_b {
            edge(&mut s, v3(b.x, b.y, z0), v3(b.x, b.y, z1));
        }
    }
    close_walls(&mut s, &faces);
    crate::openings::build(&mut s, &plans, &rules.openings);
    // Stairs and ramps: their steps, landings and handrails, and the slopes Walk climbs.
    s.part = Part::Stair;
    for st in rules.stairs.iter().take(1_000) {
        let Some(lv) = rules.levels.iter().position(|l| l.name.eq_ignore_ascii_case(&st.level)) else { continue };
        let Some(place) = places.get(lv) else { continue };
        let g = st.geometry(place.base, upf);
        let owner = Owner { src: 0, handle: Handle(0) };
        for b in &g.blocks {
            let len = b.a.dist(b.b);
            if len > 1e-9 && b.z1 > b.z0 {
                let dir = (b.b - b.a) * (1.0 / len);
                let c = (b.a + b.b) * 0.5;
                boxed(&mut s, v3(c.x, c.y, (b.z0 + b.z1) / 2.0), dir, len, b.width, b.z1 - b.z0, FLOOR, owner);
            }
        }
        for w in &g.wedges {
            let len = w.a.dist(w.b);
            if len <= 1e-9 {
                continue;
            }
            let n = Vec2 { x: -(w.b - w.a).y, y: (w.b - w.a).x } * (w.width / 2.0 / len);
            let (a0, a1, b0, b1) = (w.a - n, w.a + n, w.b - n, w.b + n);
            let top = [v3(a0.x, a0.y, w.za), v3(b0.x, b0.y, w.zb), v3(b1.x, b1.y, w.zb), v3(a1.x, a1.y, w.za)];
            quad(&mut s, top, FLOOR, owner);
            quad(&mut s, [v3(a0.x, a0.y, w.z0), v3(b0.x, b0.y, w.z0), v3(b0.x, b0.y, w.zb), v3(a0.x, a0.y, w.za)], WALL, owner);
            quad(&mut s, [v3(a1.x, a1.y, w.za), v3(b1.x, b1.y, w.zb), v3(b1.x, b1.y, w.z0), v3(a1.x, a1.y, w.z0)], WALL, owner);
        }
        for (a, b) in &g.rails {
            edge(&mut s, v3(a[0], a[1], a[2]), v3(b[0], b[1], b[2]));
        }
        s.slopes.extend(g.slopes);
    }
    s.part = Part::Device;

    // Devices: from the levels' own drawings, and from device sheets (any level, by attribute).
    let device_srcs = rules.device_sources();
    for (src, d) in sources.iter().enumerate() {
        let device_sheet = device_srcs.contains(&src);
        if !src_of.contains(&src) && !device_sheet {
            continue;
        }
        for e in d.model.iter() {
            let EntityKind::Insert(ins) = &e.kind else { continue };
            if rules.layer(&e.common.layer) != Some(&LayerKind::Device) {
                continue;
            }
            let attrs: Vec<(String, String)> = ins.attribs.iter().map(|a| (a.tag.clone(), a.text.value.clone())).collect();
            let attr = |tag: &str| attrs.iter().find(|(t, _)| t.eq_ignore_ascii_case(tag)).map(|(_, v)| v.as_str());
            let at = ins.insert.xy();
            let lv = attr(&rules.devices.level_attr)
                .and_then(|v| {
                    (0..rules.levels.len())
                        .find(|&i| (device_sheet || src_of.get(i) == Some(&src)) && rules.levels.get(i).and_then(|l| l.attr.as_deref()) == Some(v))
                })
                .or_else(|| if device_sheet { None } else { level_at(src, at) });
            let Some(lv) = lv else {
                s.unplaced += 1;
                continue;
            };
            let (Some(level), Some(place)) = (s.levels.get(lv).cloned(), places.get(lv)) else { continue };
            let ty = attr(&rules.devices.type_attr).unwrap_or("");
            let mount = rules.mount(ty).clone();
            let aff_override =
                attr(&rules.devices.aff_attr).and_then(|v| v.trim().trim_end_matches('"').parse::<f64>().ok()).filter(|v| v.is_finite());
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
            let src16 = u16::try_from(src).unwrap_or(u16::MAX);
            boxed(&mut s, pos, dir, w, across, h, c, Owner { src: src16, handle: e.handle });
            s.devices.push(DeviceInfo {
                handle: e.handle,
                src: src16,
                block: ins.block.clone(),
                level: lv,
                pos,
                aff_ft: aff,
                ceiling: mount.ceiling && aff_override.is_none(),
                attrs,
            });
        }
    }
    s
}

/// One wall face in plan: a straight piece of a wall layer's linework on a level.
#[derive(Clone, Copy, Debug)]
struct Face {
    a: Vec2,
    b: Vec2,
    lv: usize,
    z0: f64,
    z1: f64,
    owner: Owner,
    paired_only: bool,
    /// Draw a vertical edge at this end (a corner or the end of a run).
    edge_a: bool,
    edge_b: bool,
}

/// A face parallel to `f` a wall's thickness away: its signed offset (along `f`'s left normal)
/// and the overlap with `f`, in feet along `f` from `a`.
fn partner(f: &Face, g: &Face) -> Option<(f64, f64, f64)> {
    let (a, b) = (f.a, f.b);
    let len = (b - a).len();
    let ul = (g.b - g.a).len();
    if len < 1e-9 || ul < 1e-9 {
        return None;
    }
    let t = (b - a) * (1.0 / len);
    let n = Vec2 { x: -t.y, y: t.x };
    let u = g.b - g.a;
    if (t.x * u.y - t.y * u.x).abs() / ul > PARALLEL {
        return None;
    }
    let off = (g.a - a).x * n.x + (g.a - a).y * n.y;
    let off_b = (g.b - a).x * n.x + (g.b - a).y * n.y;
    if (off - off_b).abs() > 0.05 || off.abs() < WALL_THICK_FT.0 || off.abs() > WALL_THICK_FT.1 {
        return None;
    }
    let (sc, sd) = ((g.a - a).x * t.x + (g.a - a).y * t.y, (g.b - a).x * t.x + (g.b - a).y * t.y);
    let (lo, hi) = (sc.min(sd).max(0.0), sc.max(sd).min(len));
    (hi - lo >= 0.25).then_some((off, lo, hi))
}

/// Faces of paired-only layers cut down to the stretches where a partner runs alongside (see
/// [`LayerKind::Wall`]); a level with more faces than [`MAX_PAIR_FACES`] keeps them all.
fn keep_paired(faces: Vec<Face>) -> Vec<Face> {
    if !faces.iter().any(|f| f.paired_only) {
        return faces;
    }
    let mut by_level: std::collections::BTreeMap<usize, Vec<usize>> = std::collections::BTreeMap::new();
    for (i, f) in faces.iter().enumerate() {
        by_level.entry(f.lv).or_default().push(i);
    }
    let mut out = Vec::with_capacity(faces.len());
    for (i, f) in faces.iter().enumerate() {
        let on = by_level.get(&f.lv).map(Vec::as_slice).unwrap_or(&[]);
        if !f.paired_only || on.len() > MAX_PAIR_FACES {
            out.push(*f);
            continue;
        }
        let mut spans: Vec<(f64, f64)> =
            on.iter().filter(|&&j| j != i).filter_map(|&j| faces.get(j)).filter_map(|g| partner(f, g)).map(|(_, lo, hi)| (lo, hi)).collect();
        spans.sort_by(|x, y| x.0.total_cmp(&y.0));
        let mut merged: Vec<(f64, f64)> = Vec::new();
        for (lo, hi) in spans {
            match merged.last_mut() {
                Some(m) if lo <= m.1 + 0.01 => m.1 = m.1.max(hi),
                _ => merged.push((lo, hi)),
            }
        }
        let len = (f.b - f.a).len();
        let t = (f.b - f.a) * (1.0 / len.max(1e-12));
        for (lo, hi) in merged {
            let at_a = lo < 0.01;
            let at_b = hi > len - 0.01;
            out.push(Face {
                a: f.a + t * lo,
                b: f.a + t * hi,
                edge_a: if at_a { f.edge_a } else { true },
                edge_b: if at_b { f.edge_b } else { true },
                ..*f
            });
        }
    }
    out
}

/// Pair each wall face with the nearest parallel face a wall's thickness away and close the strip
/// between them: a top cap over the overlap and end caps where the overlap stops (door and window
/// openings, wall ends). Plans draw both sides of a wall; this makes them read as one solid.
fn close_walls(s: &mut Scene, faces: &[Face]) {
    let mut by_level: std::collections::BTreeMap<usize, Vec<&Face>> = std::collections::BTreeMap::new();
    for f in faces {
        by_level.entry(f.lv).or_default().push(f);
    }
    for on in by_level.values() {
        if on.len() > MAX_PAIR_FACES {
            continue;
        }
        for (i, f) in on.iter().enumerate() {
            let (a, b) = (f.a, f.b);
            let len = (b - a).len();
            if len < 1e-9 {
                continue;
            }
            let t = (b - a) * (1.0 / len);
            let n = Vec2 { x: -t.y, y: t.x };
            // Nearest partner on the +n side only, so each pair closes once.
            let best = on
                .iter()
                .enumerate()
                .filter(|(j, _)| *j != i)
                .filter_map(|(_, g)| partner(f, g))
                .filter(|(off, _, _)| *off > 0.0)
                .min_by(|x, y| x.0.total_cmp(&y.0));
            let Some((off, lo, hi)) = best else { continue };
            let (z0, z1) = (f.z0, f.z1);
            let p = |along: f64, out: f64, z: f64| {
                let q = a + t * along + n * out;
                v3(q.x, q.y, z)
            };
            quad(s, [p(lo, 0.0, z1), p(hi, 0.0, z1), p(hi, off, z1), p(lo, off, z1)], WALL_TOP, f.owner);
            // End caps only where the overlap stops at this face's own end (an opening or a wall
            // end), not where a longer partner runs past a junction.
            if lo < 0.05 {
                quad(s, [p(lo, off, z0), p(lo, 0.0, z0), p(lo, 0.0, z1), p(lo, off, z1)], WALL, f.owner);
                edge(s, p(lo, 0.0, z1), p(lo, off, z1));
            }
            if hi > len - 0.05 {
                quad(s, [p(hi, 0.0, z0), p(hi, off, z0), p(hi, off, z1), p(hi, 0.0, z1)], WALL, f.owner);
                edge(s, p(hi, 0.0, z1), p(hi, off, z1));
            }
            s.solid_walls += 1;
        }
    }
}

/// The solid stretches of wall: each face with its nearest partner on its +n side, over their
/// overlap (as [`close_walls`] closes them).
fn pair_pieces(faces: &[Face]) -> Vec<Piece> {
    let mut by_level: std::collections::BTreeMap<usize, Vec<&Face>> = std::collections::BTreeMap::new();
    for f in faces {
        by_level.entry(f.lv).or_default().push(f);
    }
    let mut pieces = Vec::new();
    for on in by_level.values() {
        if on.len() > MAX_PAIR_FACES {
            continue;
        }
        for (i, f) in on.iter().enumerate() {
            let len = (f.b - f.a).len();
            if len < 1e-9 {
                continue;
            }
            let t = (f.b - f.a) * (1.0 / len);
            let n = Vec2 { x: -t.y, y: t.x };
            let best = on
                .iter()
                .enumerate()
                .filter(|(j, _)| *j != i)
                .filter_map(|(_, g)| partner(f, g))
                .filter(|(off, _, _)| *off > 0.0)
                .min_by(|x, y| x.0.total_cmp(&y.0));
            if let Some((off, lo, hi)) = best {
                pieces.push(Piece { p0: f.a + t * lo, p1: f.a + t * hi, n, off, lv: f.lv, z0: f.z0, z1: f.z1, owner: f.owner });
            }
        }
    }
    pieces
}

/// A solid stretch of wall: along one face from `p0` to `p1`, `off` thick on the face's `n` side.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Piece {
    pub(crate) p0: Vec2,
    pub(crate) p1: Vec2,
    pub(crate) n: Vec2,
    pub(crate) off: f64,
    pub(crate) lv: usize,
    pub(crate) z0: f64,
    pub(crate) z1: f64,
    pub(crate) owner: Owner,
}

/// A polyline without its curved parts: a segment is dropped when either end turns gently
/// (2°–50°) into its neighbour, as chords of a flattened arc do. What remains is split into runs.
#[cfg(test)]
fn straight_runs(pts: &[Vec2]) -> Vec<Vec<Vec2>> {
    split_runs(pts).0
}

/// A polyline split into its straight runs and its curved runs (see [`straight_runs`]).
fn split_runs(pts: &[Vec2]) -> (Vec<Vec<Vec2>>, Vec<Vec<Vec2>>) {
    let turn = |p: Vec2, q: Vec2, r: Vec2| -> f64 {
        let (u, v) = (q - p, r - q);
        let (lu, lv) = (u.len(), v.len());
        if lu < 1e-12 || lv < 1e-12 {
            return 0.0;
        }
        ((u.x * v.x + u.y * v.y) / (lu * lv)).clamp(-1.0, 1.0).acos().to_degrees()
    };
    let gentle = |deg: f64| (2.0..=50.0).contains(&deg);
    let n = pts.len();
    let mut runs: Vec<Vec<Vec2>> = Vec::new();
    let mut cur: Vec<Vec2> = Vec::new();
    let mut arcs: Vec<Vec<Vec2>> = Vec::new();
    let mut arc: Vec<Vec2> = Vec::new();
    for i in 0..n.saturating_sub(1) {
        let (Some(&a), Some(&b)) = (pts.get(i), pts.get(i + 1)) else { break };
        let before = i.checked_sub(1).and_then(|j| pts.get(j)).map(|&p| turn(p, a, b));
        let after = pts.get(i + 2).map(|&r| turn(a, b, r));
        let curved = before.is_some_and(gentle) || after.is_some_and(gentle);
        if curved {
            if cur.len() >= 2 {
                runs.push(std::mem::take(&mut cur));
            }
            cur.clear();
            if arc.is_empty() {
                arc.push(a);
            }
            arc.push(b);
            continue;
        }
        if arc.len() >= 3 {
            arcs.push(std::mem::take(&mut arc));
        }
        arc.clear();
        if cur.is_empty() {
            cur.push(a);
        }
        cur.push(b);
    }
    if cur.len() >= 2 {
        runs.push(cur);
    }
    if arc.len() >= 3 {
        arcs.push(arc);
    }
    (runs, arcs)
}

/// Each level's name and base point: the sheet point that maps to the building origin (its
/// `base`, else the lower-left corner of its walls). Exports place each level's objects by it, so
/// plans drawn side by side on one sheet stack as storeys.
pub fn level_bases(d: &Drawing, rules: &Rules) -> Vec<(String, Vec2)> {
    rules.levels.iter().map(|l| (l.name.clone(), base_point(d, rules, l))).collect()
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

/// A wall face's colour: the base colour of the material its line shows (its own or its
/// layer's, MATERIALATTACH), else the plain wall colour.
fn wall_rgb(sources: &[&Drawing], owner: Owner) -> Rgb {
    let Some(d) = sources.get(usize::from(owner.src)) else { return WALL };
    d.entity(owner.handle).and_then(|e| d.material_of(e, None)).map_or(WALL, |m| m.color)
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

pub(crate) fn edge(s: &mut Scene, a: V3, b: V3) {
    s.edges.push(vtx(a, V3::default(), EDGE));
    s.edges.push(vtx(b, V3::default(), EDGE));
}

/// Two triangles for a planar quad (corners in order).
pub(crate) fn quad(s: &mut Scene, q: [V3; 4], c: Rgb, owner: Owner) {
    let [a, b, cc, d] = q;
    let n = (b - a).cross(d - a).norm();
    for p in [a, b, cc, a, cc, d] {
        s.tris.push(vtx(p, n, c));
    }
    s.parts.push(s.part);
    s.parts.push(s.part);
    s.owners.push(owner);
    s.owners.push(owner);
}

/// A box centred on `c`, `along` wide in direction `dir`, `across` deep, `h` high.
#[allow(clippy::too_many_arguments)]
pub(crate) fn boxed(s: &mut Scene, c: V3, dir: Vec2, along: f64, across: f64, h: f64, col: Rgb, owner: Owner) {
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
    use cadaddy_doc::{Insert, Line};
    use cadaddy_geom::Vec3;

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
            sheet: None,
        };
        let mut devices = DeviceRules::default();
        devices.by_type.insert("REC".into(), Mount { ceiling: false, aff_ft: 1.5, size_ft: [0.3, 0.15, 0.45] });
        let rules = Rules {
            units_per_foot: 12.0,
            levels: vec![lvl("L3", 20.0, -100.0, "3"), lvl("L4", 30.0, 900.0, "4")],
            layers: vec![
                LayerRule { layer: "A-WALL".into(), kind: LayerKind::Wall { height_ft: None, skip_curves: false, paired_only: false } },
                LayerRule { layer: "E-DEV*".into(), kind: LayerKind::Device },
            ],
            devices,
            device_sheets: Vec::new(),
            openings: crate::rules::Openings::default(),
            stairs: Vec::new(),
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
        assert!(zs.contains(&20.0) && zs.contains(&29.9) && zs.contains(&39.9), "walls stop just under the next floor");
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
    fn the_built_in_rules_place_blocks_on_electrical_layers() {
        // A plan with no rules file: receptacles and lights on the layers people name them by,
        // a door on its architectural layer (battle test 0.7.8 #19: 0 devices).
        let mut d = Drawing::default();
        for (a, b) in [((0.0, 0.0), (240.0, 0.0)), ((240.0, 0.0), (240.0, 120.0)), ((240.0, 120.0), (0.0, 120.0)), ((0.0, 120.0), (0.0, 0.0))] {
            add(&mut d, "A-WALL", line(a.0, a.1, b.0, b.1));
        }
        for (i, layer) in ["E-POWR", "E-LITE", "POWER", "LIGHTING", "DEVICES"].iter().enumerate() {
            add(&mut d, layer, device(20.0 + 40.0 * i as f64, 60.0, "A", "1"));
        }
        add(&mut d, "A-DOOR", device(100.0, 0.0, "DOOR", "1"));
        let s = derive(&d, &Rules::fallback());
        assert_eq!(s.devices.len(), 5, "{:?}", s.devices.iter().map(|v| &v.block).collect::<Vec<_>>());
        assert_eq!(s.walls.len(), 4, "a wall layer stays a wall");
    }

    #[test]
    fn room_names_not_labels() {
        for name in ["OFFICE 101", "Storage", "101 Lobby", "MECH & ELEC", "CONF. RM 2", "B12 Stair"] {
            assert!(room_name(name), "{name}");
        }
        for name in
            ["PANEL LP-1 120/208V 3PH", "10'-0\" AFF", "GENERAL NOTES:", "12", "X", "SEE NOTE #3", "LEVEL 2 CORRIDOR 204", "ONE TWO THREE FOUR FIVE"]
        {
            assert!(!room_name(name), "{name}");
        }
    }

    #[test]
    fn walls_show_their_layers_material_colour() {
        let (mut d, rules) = two_floor_sheet();
        let plain = derive(&d, &rules);
        assert!(plain.tris.iter().any(|v| v.c[..3] == [WALL.0, WALL.1, WALL.2]));
        let concrete = cadaddy_doc::material::recipe("concrete").unwrap().material(0.0254);
        let c = concrete.color;
        d.materials.push(concrete.clone());
        d.layers.retain(|l| l.name != "A-WALL");
        d.layers.push(cadaddy_doc::Layer { name: "A-WALL".into(), material: Some(concrete.name.clone()), ..cadaddy_doc::Layer::default() });
        let s = derive(&d, &rules);
        assert!(s.tris.iter().any(|v| v.c[..3] == [c.0, c.1, c.2]), "walls take the material's colour");
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
    fn floors_from_separate_sheets_stack_and_remember_their_sheet() {
        let room = || {
            let mut d = Drawing::default();
            add(&mut d, "A-WALL", line(0.0, 0.0, 240.0, 0.0));
            add(&mut d, "A-WALL", line(240.0, 0.0, 240.0, 120.0));
            d
        };
        let (a, b) = (room(), room());
        let lvl = |name: &str, elev: f64, sheet: &str| Level {
            name: name.into(),
            elevation_ft: elev,
            height_ft: 10.0,
            ceiling_ft: 8.0,
            region: None,
            base: Some([0.0, 0.0]),
            attr: None,
            sheet: Some(sheet.into()),
        };
        let rules = Rules {
            units_per_foot: 12.0,
            levels: vec![lvl("L1", 0.0, "A111.dxf"), lvl("L2", 10.0, "A112.dxf"), lvl("L2 again", 20.0, "A112.dxf")],
            layers: vec![LayerRule { layer: "A-WALL".into(), kind: LayerKind::Wall { height_ft: None, skip_curves: false, paired_only: false } }],
            devices: DeviceRules::default(),
            device_sheets: Vec::new(),
            openings: crate::rules::Openings::default(),
            stairs: Vec::new(),
        };
        assert_eq!(rules.sheets(), ["A111.dxf", "A112.dxf"]);
        assert_eq!((rules.level_source(0), rules.level_source(1), rules.level_source(2)), (1, 2, 2));
        let s = derive_set(&[&Drawing::default(), &a, &b], &rules);
        // Each sheet feeds its own levels; a sheet's entity lands on the first level that takes it.
        assert_eq!(s.walls.iter().filter(|w| w.level == 0).count(), 2);
        assert_eq!(s.walls.iter().filter(|w| w.level == 1).count(), 2);
        assert!(s.owners.iter().any(|o| o.src == 1) && s.owners.iter().any(|o| o.src == 2));
        assert_eq!(s.levels.first().map(|l| (l.min, l.max)), s.levels.get(1).map(|l| (l.min, l.max)));
        // A missing sheet leaves its levels empty instead of failing.
        let s = derive_set(&[&Drawing::default()], &rules);
        assert!(s.walls.is_empty());
    }

    #[test]
    fn both_faces_of_a_wall_close_into_a_solid() {
        let (mut d, rules) = two_floor_sheet();
        // A 6" partition, 10' long, inside the lower room: two faces 6" apart.
        add(&mut d, "A-WALL", line(60.0, 30.0, 180.0, 30.0));
        add(&mut d, "A-WALL", line(180.0, 36.0, 60.0, 36.0));
        let s = derive(&d, &rules);
        assert!(s.solid_walls >= 1);
        let top = [WALL_TOP.0, WALL_TOP.1, WALL_TOP.2, 255];
        let cap: Vec<&Vertex> = s.tris.iter().filter(|v| v.c == top).collect();
        assert!(!cap.is_empty() && cap.iter().all(|v| (v.p[2] - 29.9).abs() < 1e-4), "caps sit on top of the wall");
        assert!(cap.iter().all(|v| (2.5 - 1e-3..=3.0 + 1e-3).contains(&f64::from(v.p[1]))), "between the faces");
        // The room's walls are 10' apart: too far to be one wall.
        assert!(s.solid_walls < 4);
    }

    #[test]
    fn door_swings_drop_out_of_walls_but_corners_stay() {
        let p = |x: f64, y: f64| Vec2 { x, y };
        // An L-shaped wall, then a quarter-circle swing in 9 chords.
        let wall = [p(0.0, 0.0), p(10.0, 0.0), p(10.0, 5.0)];
        assert_eq!(straight_runs(&wall), vec![wall.to_vec()]);
        let swing: Vec<Vec2> = (0..=9).map(|i| f64::from(i) * std::f64::consts::FRAC_PI_2 / 9.0).map(|a| p(3.0 * a.cos(), 3.0 * a.sin())).collect();
        assert!(straight_runs(&swing).is_empty());
        // A leaf line joined square to the end of its swing keeps only the leaf.
        let mut leaf = vec![p(0.0, 0.0), p(3.0, 0.0)];
        leaf.extend(swing.iter().skip(1));
        assert_eq!(straight_runs(&leaf), vec![vec![p(0.0, 0.0), p(3.0, 0.0)]], "the leaf stays, its swing goes");
    }

    #[test]
    fn paired_only_keeps_walls_and_drops_lone_lines() {
        let (mut d, mut rules) = two_floor_sheet();
        for r in &mut rules.layers {
            if let LayerKind::Wall { paired_only, .. } = &mut r.kind {
                *paired_only = true;
            }
        }
        // A 6" partition 10' long with a 3' doorway in one face, plus a lone dimension line.
        add(&mut d, "A-WALL", line(60.0, 30.0, 180.0, 30.0));
        add(&mut d, "A-WALL", line(60.0, 36.0, 96.0, 36.0));
        add(&mut d, "A-WALL", line(132.0, 36.0, 180.0, 36.0));
        add(&mut d, "A-WALL", line(10.0, 80.0, 200.0, 80.0));
        // Only the faces are checked here: the doorway is not filled (see `openings`).
        rules.openings.enabled = false;
        let s = derive(&d, &rules);
        let lv0: Vec<&Seg> = s.walls.iter().filter(|w| w.level == 0).collect();
        // The room's own walls are 10' apart: none of them pair, so the room's lone outline goes.
        assert!(lv0.iter().all(|w| (w.a.y - 2.5).abs() < 1e-9 || (w.a.y - 3.0).abs() < 1e-9), "{lv0:?}");
        // The full face is cut to the two stretches its partner runs along: the doorway opens.
        let along_full: Vec<(f64, f64)> = lv0.iter().filter(|w| (w.a.y - 2.5).abs() < 1e-9).map(|w| (w.a.x, w.b.x)).collect();
        assert_eq!(along_full.len(), 2, "{along_full:?}");
        assert!(along_full.iter().any(|(a, b)| (a - 5.0).abs() < 1e-9 && (b - 8.0).abs() < 1e-9));
        assert!(along_full.iter().any(|(a, b)| (a - 11.0).abs() < 1e-9 && (b - 15.0).abs() < 1e-9));
    }

    #[test]
    fn a_device_sheet_places_devices_on_their_floor() {
        let (d, mut rules) = two_floor_sheet();
        let mut devs = Drawing::default();
        // Floor 4 by attribute, placed with level L4's base (the lower-left of its walls).
        add(&mut devs, "E-DEVC", device(1120.0, 1060.0, "A", "4"));
        add(&mut devs, "E-DEVC", device(9999.0, 9999.0, "A", "7"));
        rules.device_sheets = vec!["Hotel devices.dxf".into()];
        assert_eq!(rules.sheets(), ["Hotel devices.dxf"]);
        assert_eq!(rules.device_sources(), [1]);
        let s = derive_set(&[&d, &devs], &rules);
        let from_sheet: Vec<&DeviceInfo> = s.devices.iter().filter(|v| v.src == 1).collect();
        assert_eq!(from_sheet.len(), 1, "a floor no level has is not placed");
        assert!(from_sheet.iter().all(|v| v.level == 1 && (v.pos.x - 1120.0 / 12.0).abs() < 1e-9 && (v.pos.y - 5.0).abs() < 1e-9), "{from_sheet:?}");
        assert!(s.unplaced >= 1);
        // Walls in a device sheet are not walls.
        assert_eq!(s.walls.len(), derive(&d, &rules).walls.len());
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
