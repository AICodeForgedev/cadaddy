//! IFC4 exchange, written from the public IFC schema and ISO 10303-21 (the STEP physical file);
//! no IFC library is used.
//!
//! Out: the building WALLGEN made — walls (IfcWall), floor slabs (IfcSlab), ceilings
//! (IfcCovering), doors (IfcDoor), windows (IfcWindow), room volumes (IfcSpace) and other meshes
//! (IfcBuildingElementProxy) — in a project, site, building and one storey per level, each body a
//! tessellated IfcPolygonalFaceSet in metres. GlobalIds come from the objects' handles, so the
//! same drawing gives the same ids.
//!
//! In: every product with a body (walls, slabs, spaces, doors… whatever the file has) as a MESH on
//! a layer per IFC class (`IFC-Wall`, `IFC-Space`…), tagged with its class, GlobalId and name. The
//! common body kinds are read: polygonal and triangulated face sets, faceted BReps and shell
//! models, extruded rectangle, circle and polyline profiles, mapped items and the first operand of
//! booleans (openings aren't cut), placed by their local placement chains, in the file's length unit.

use std::collections::HashMap;
use std::fmt::Write as _;

use cadaddy_doc::{Common, Drawing, Entity, EntityKind, Layer, Mesh, MeshFace, MeshKind, RawTag, Space};
use cadaddy_geom::Vec3;

use crate::{IoError, Result};

/// The extended data application tagging objects read from IFC (class, GlobalId, name).
pub const APP: &str = "CADADDY_IFC";
/// Most instances one file holds.
const MAX_INSTANCES: usize = 5_000_000;
/// Deepest placement chain, mapped item or boolean followed.
const MAX_DEPTH: usize = 64;
/// Most points one imported object has.
const MAX_POINTS: usize = 2_000_000;
/// Most faces all imported objects have together.
const MAX_FACES: usize = 10_000_000;

// ---------------- what goes out ----------------

/// What an object is in IFC, from what WALLGEN, model.build or ROOMS tagged it as (or the class
/// it was read as from an IFC file).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Class {
    Wall,
    Slab,
    Ceiling,
    Door,
    Window,
    Stair,
    Space,
    Proxy,
    Column,
    CurtainWall,
    Railing,
    Ramp,
    Roof,
    Beam,
    Terrain,
    Duct,
    Pipe,
    Conduit,
}

impl Class {
    /// The IFC4 entity and the predefined type this class is written with.
    fn ifc(self) -> (&'static str, &'static str) {
        match self {
            Class::Wall => ("IfcWall", "STANDARD"),
            Class::Slab => ("IfcSlab", "FLOOR"),
            Class::Ceiling => ("IfcCovering", "CEILING"),
            Class::Door => ("IfcDoor", "DOOR"),
            Class::Window => ("IfcWindow", "WINDOW"),
            Class::Stair => ("IfcStair", "NOTDEFINED"),
            Class::Space => ("IfcSpace", "INTERNAL"),
            Class::Proxy => ("IfcBuildingElementProxy", "NOTDEFINED"),
            Class::Column => ("IfcColumn", "COLUMN"),
            Class::CurtainWall => ("IfcCurtainWall", "NOTDEFINED"),
            Class::Railing => ("IfcRailing", "HANDRAIL"),
            Class::Ramp => ("IfcRamp", "NOTDEFINED"),
            Class::Roof => ("IfcRoof", "NOTDEFINED"),
            Class::Beam => ("IfcBeam", "BEAM"),
            Class::Terrain => ("IfcGeographicElement", "TERRAIN"),
            Class::Duct => ("IfcDuctSegment", "RIGIDSEGMENT"),
            Class::Pipe => ("IfcPipeSegment", "RIGIDSEGMENT"),
            Class::Conduit => ("IfcCableCarrierSegment", "CONDUITSEGMENT"),
        }
    }

    /// The class an object read from IFC as `name` (the reader's class name) is written back as.
    fn of_read(name: &str) -> Option<Class> {
        Some(match name {
            "Wall" | "WallStandardCase" => Class::Wall,
            "Slab" => Class::Slab,
            "Covering" => Class::Ceiling,
            "Door" => Class::Door,
            "Window" => Class::Window,
            "Stair" => Class::Stair,
            "Space" => Class::Space,
            "BuildingElementProxy" => Class::Proxy,
            "Column" => Class::Column,
            "CurtainWall" => Class::CurtainWall,
            "Railing" => Class::Railing,
            "Ramp" => Class::Ramp,
            "Roof" => Class::Roof,
            "Beam" => Class::Beam,
            "GeographicElement" => Class::Terrain,
            "DuctSegment" => Class::Duct,
            "PipeSegment" => Class::Pipe,
            "CableCarrierSegment" => Class::Conduit,
            _ => return None,
        })
    }
}

/// What an object is written as: its class, predefined type, storey (WALLGEN's level), name,
/// long name, and the GlobalId it was read with from an IFC file.
struct Kind {
    class: Class,
    predefined: &'static str,
    level: Option<String>,
    name: String,
    long: String,
    read_id: Option<String>,
}

/// The tag `app`'s first strings on `e` (WALLGEN: part, level; ROOMS: name, number).
fn tag_strings(e: &Entity, app: &str) -> Option<Vec<String>> {
    let x = &e.common.xdata;
    let at = x.iter().position(|t| t.code == 1001 && t.value.trim().eq_ignore_ascii_case(app))?;
    Some(x.iter().skip(at + 1).take_while(|t| t.code != 1001).filter(|t| t.code == 1000).map(|t| t.value.clone()).collect())
}

fn class_of(e: &Entity) -> Kind {
    let kind =
        |class: Class, level: Option<String>, name: String, long: String| Kind { class, predefined: class.ifc().1, level, name, long, read_id: None };
    if let Some(t) = tag_strings(e, "CADADDY_WALLGEN") {
        let part = t.first().map(String::as_str).unwrap_or_default();
        // model.build's MEP runs: `system:<name>` and `kind:duct|pipe|conduit` after the source.
        let extra = |key: &str| t.iter().skip(3).find_map(|v| v.strip_prefix(key)).map(str::to_string);
        let class = match part {
            "wall" | "parapet" => Class::Wall,
            "floor" => Class::Slab,
            "ceiling" | "soffit" => Class::Ceiling,
            "door" => Class::Door,
            "window" => Class::Window,
            "stair" => Class::Stair,
            "column" => Class::Column,
            "curtainwall" => Class::CurtainWall,
            "railing" => Class::Railing,
            "ramp" => Class::Ramp,
            "roof" => Class::Roof,
            "ground" => Class::Terrain,
            "mep" => match extra("kind:").as_deref() {
                Some("duct") => Class::Duct,
                Some("pipe") => Class::Pipe,
                Some("conduit") => Class::Conduit,
                _ => Class::Proxy,
            },
            _ => Class::Proxy,
        };
        let mut k = kind(class, t.get(1).cloned(), extra("system:").unwrap_or_else(|| part.to_string()), String::new());
        match part {
            "parapet" => k.predefined = "PARAPET",
            "soffit" => k.predefined = "CLADDING",
            _ => {}
        }
        return k;
    }
    if let Some(t) = tag_strings(e, "CADADDY_ROOM") {
        return kind(Class::Space, None, t.first().cloned().unwrap_or_default(), t.get(1).cloned().unwrap_or_default());
    }
    // Read from an IFC file: written back as its class, with its GlobalId and name.
    if let Some(t) = tag_strings(e, APP)
        && let Some(class) = t.first().and_then(|c| Class::of_read(c))
    {
        let name = t.get(2).cloned().filter(|n| !n.is_empty()).unwrap_or_else(|| e.common.layer.clone());
        let mut k = kind(class, None, name, String::new());
        // Its predefined type wasn't read: not defined, rather than a guess.
        k.predefined = if class == Class::Space { "SPACE" } else { "NOTDEFINED" };
        k.read_id = t.get(1).filter(|g| is_guid(g)).cloned();
        return k;
    }
    kind(Class::Proxy, None, e.common.layer.clone(), String::new())
}

/// Whether `g` is an IFC GlobalId: 22 characters of IFC's base-64 alphabet, the first 0–3.
fn is_guid(g: &str) -> bool {
    g.len() == 22
        && g.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'$')
        && g.as_bytes().first().is_some_and(|b| (b'0'..=b'3').contains(b))
}

/// A number standing for `text` (FNV-1a), for GlobalIds made from names.
fn hash(text: &str) -> u64 {
    text.bytes().fold(0xCBF2_9CE4_8422_2325, |h, b| (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01B3))
}

/// What makes this drawing's GlobalIds its own: its FINGERPRINTGUID (every AutoCAD drawing has one,
/// kept when it is saved), so two drawings with the same handles don't give the same ids; 0 for a
/// drawing without one (its ids are then the handles' alone, as before).
fn drawing_seed(d: &Drawing) -> u64 {
    match d.header.get("FINGERPRINTGUID").and_then(cadaddy_doc::HVal::as_str).map(str::trim) {
        Some(g) if !g.is_empty() => hash(&g.to_ascii_uppercase()),
        _ => 0,
    }
}

/// An IFC GlobalId: 128 bits from `seed`, in IFC's 22-character base-64 form.
fn guid(seed: u64, salt: u64) -> String {
    const CHARS: &[u8; 64] = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz_$";
    let mix = |mut z: u64| {
        z = z.wrapping_add(0x9E37_79B9_7F4A_7C15);
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    };
    let hi = mix(seed ^ salt.rotate_left(17));
    let lo = mix(hi ^ seed.rotate_left(29) ^ salt);
    let v = (u128::from(hi) << 64) | u128::from(lo);
    let mut s = String::with_capacity(22);
    s.push(CHARS[(v >> 126) as usize & 3] as char);
    for i in (0..21).rev() {
        s.push(CHARS[(v >> (i * 6)) as usize & 63] as char);
    }
    s
}

/// A STEP string: `'` doubled, `\` doubled, anything outside printable ASCII as `\X2\…\X0\`.
fn step_str(s: &str) -> String {
    let mut out = String::from("'");
    for c in s.chars() {
        match c {
            '\'' => out.push_str("''"),
            '\\' => out.push_str("\\\\"),
            ' '..='~' => out.push(c),
            // ISO 10303-21: \X2\ holds 16-bit characters, \X4\ the ones above U+FFFF.
            c if u32::from(c) > 0xFFFF => {
                let _ = write!(out, "\\X4\\{:08X}\\X0\\", u32::from(c));
            }
            c => {
                let _ = write!(out, "\\X2\\{:04X}\\X0\\", u32::from(c));
            }
        }
    }
    out.push('\'');
    out
}

/// A STEP real: always with a decimal point, no exponent for ordinary values.
fn real(v: f64) -> String {
    if !v.is_finite() {
        return "0.".into();
    }
    let s = format!("{v:.6}");
    let s = s.trim_end_matches('0');
    if s == "-0." { "0.".into() } else { s.to_string() }
}

/// Instances written one per line, numbered from 1.
struct Out {
    lines: Vec<String>,
}

impl Out {
    fn add(&mut self, body: String) -> usize {
        self.lines.push(body);
        self.lines.len()
    }
}

fn refs(ids: &[usize]) -> String {
    let v: Vec<String> = ids.iter().map(|i| format!("#{i}")).collect();
    format!("({})", v.join(","))
}

/// A wall, door or window as written: its class, product and placement instances, storey, and
/// its extents in the storey's coordinates.
struct Placed {
    class: Class,
    product: usize,
    place: usize,
    storey: usize,
    lo: [f64; 3],
    hi: [f64; 3],
    handle: u64,
}

/// A mesh's measures for the base quantities, in drawing units: its volume (when it is closed),
/// its footprint (the area of its downward faces seen from above) and its height.
struct Measures {
    volume: Option<f64>,
    footprint: f64,
    height: f64,
}

fn measures(m: &Mesh) -> Measures {
    let p = |i: &u32| m.vertices.get(*i as usize).copied();
    let mut volume = 0.0;
    let mut footprint = 0.0;
    let mut edges: HashMap<(u32, u32), i64> = HashMap::new();
    let mut closed = true;
    for f in &m.faces {
        let pts: Vec<Vec3> = f.v.iter().filter_map(p).collect();
        if pts.len() < 3 || pts.len() != f.v.len() {
            closed = false;
            continue;
        }
        // Fan triangles: signed volume against the origin, and the area seen from above.
        let mut normal_z = 0.0;
        let mut area_xy = 0.0;
        if let Some(&a) = pts.first() {
            for w in pts.get(1..).unwrap_or_default().windows(2) {
                if let [b, c] = w {
                    volume += a.dot(b.cross(*c)) / 6.0;
                    normal_z += (*b - a).cross(*c - a).z;
                    area_xy += ((b.x - a.x) * (c.y - a.y) - (c.x - a.x) * (b.y - a.y)) / 2.0;
                }
            }
        }
        if normal_z < 0.0 {
            footprint += area_xy.abs();
        }
        // Each edge one way in one face and the other way in another: a closed, oriented surface.
        for (i, &a) in f.v.iter().enumerate() {
            let b = f.v.get((i + 1) % f.v.len()).copied().unwrap_or(a);
            *edges.entry((a.min(b), a.max(b))).or_default() += if a < b { 1 } else { -1 };
        }
    }
    closed &= !edges.is_empty() && edges.values().all(|n| *n == 0);
    let (lo, hi) = m.vertices.iter().fold((f64::MAX, f64::MIN), |(lo, hi), v| (lo.min(v.z), hi.max(v.z)));
    Measures { volume: closed.then_some(volume.abs()), footprint, height: (hi - lo).max(0.0) }
}

/// The drawing's model-space meshes as an IFC4 file (`name` goes into the header).
pub fn write_ifc(d: &Drawing, name: &str) -> Result<Vec<u8>> {
    let insunits = d.header.i64("INSUNITS", 0);
    // Metres per drawing unit: a drawing without units is in millimetres if metric, else inches.
    let unit_scale = match insunits {
        0 if d.header.i64("MEASUREMENT", 1) == 1 => 0.001,
        0 => 0.0254,
        u => cadaddy_doc::geo::metres_per_unit(u).max(1e-9),
    };
    let mm = (unit_scale - 0.001).abs() < 1e-12;
    struct Item<'a> {
        e: &'a Entity,
        mesh: &'a Mesh,
        kind: Kind,
        zmin: f64,
    }
    let mut items: Vec<Item> = Vec::new();
    for e in d.model.iter() {
        let EntityKind::Mesh(m) = &e.kind else { continue };
        if m.faces.is_empty() || m.vertices.is_empty() {
            continue;
        }
        let zmin = m.vertices.iter().map(|v| v.z).fold(f64::MAX, f64::min);
        items.push(Item { e, mesh: m, kind: class_of(e), zmin });
    }
    if items.is_empty() {
        return Err(IoError::Format("nothing to write to IFC: no meshes in model space (make the building with WALLGEN or ROOMS first)".into()));
    }
    // Storeys: WALLGEN's levels, lowest first, at their lowest wall or slab top; one when none.
    let mut levels: Vec<(String, f64)> = Vec::new();
    for it in &items {
        if let Some(l) = &it.kind.level {
            let z = if it.kind.class == Class::Slab { it.mesh.vertices.iter().map(|v| v.z).fold(f64::MIN, f64::max) } else { it.zmin };
            match levels.iter_mut().find(|(n, _)| n == l) {
                Some(x) => x.1 = x.1.min(z),
                None => levels.push((l.clone(), z)),
            }
        }
    }
    if levels.is_empty() {
        levels.push(("Level 1".into(), items.iter().map(|i| i.zmin).fold(f64::MAX, f64::min).min(0.0)));
    }
    levels.sort_by(|a, b| a.1.total_cmp(&b.1));
    // Where each storey's objects are on the sheet: plans drawn side by side (L2's beside L1's)
    // are built over their own plans, and their building spec's base points stack them.
    let bases: Vec<(String, cadaddy_geom::Vec2)> = d
        .building_spec
        .as_deref()
        .and_then(|t| cadaddy_walk3d::BuildingSpec::from_json(t).ok())
        .map(|sp| cadaddy_walk3d::level_bases(d, &sp.to_rules()))
        .unwrap_or_default();
    let base_of = |level: &str| bases.iter().find(|(n, _)| n == level).map_or(cadaddy_geom::Vec2::ZERO, |(_, b)| *b);
    let storey_of = |it: &Item| -> usize {
        if let Some(l) = &it.kind.level
            && let Some(i) = levels.iter().position(|(n, _)| n == l)
        {
            return i;
        }
        // The highest storey at or below the object's foot.
        levels.iter().rposition(|(_, z)| *z <= it.zmin + 1e-6).unwrap_or(0)
    };
    // GlobalIds: from the drawing's own seed, so they stay the same each time it is exported (and
    // differ from another drawing's); an object read from IFC keeps the id it was read with
    // (the first one, when copies of it carry the same id).
    let seed = drawing_seed(d);
    let id = |n: u64, salt: u64| guid(n, salt ^ seed);
    let mut read_ids: HashMap<&str, u64> = HashMap::new();
    for it in &items {
        if let Some(g) = &it.kind.read_id {
            let h = read_ids.entry(g.as_str()).or_insert(it.e.handle.0);
            *h = (*h).min(it.e.handle.0);
        }
    }
    let element_id = |it: &Item| -> String {
        match &it.kind.read_id {
            Some(g) if read_ids.get(g.as_str()) == Some(&it.e.handle.0) => g.clone(),
            _ => id(it.e.handle.0, 7),
        }
    };

    let mut o = Out { lines: Vec::new() };
    let unit = if mm { o.add("IFCSIUNIT(*,.LENGTHUNIT.,.MILLI.,.METRE.)".into()) } else { o.add("IFCSIUNIT(*,.LENGTHUNIT.,$,.METRE.)".into()) };
    // Coordinates go out in the unit declared: millimetres for metric drawings, else metres.
    let k = if mm { 1.0 } else { unit_scale };
    let area = o.add("IFCSIUNIT(*,.AREAUNIT.,$,.SQUARE_METRE.)".into());
    let volume = o.add("IFCSIUNIT(*,.VOLUMEUNIT.,$,.CUBIC_METRE.)".into());
    let units = o.add(format!("IFCUNITASSIGNMENT({})", refs(&[unit, area, volume])));
    let origin = o.add("IFCCARTESIANPOINT((0.,0.,0.))".into());
    let zdir = o.add("IFCDIRECTION((0.,0.,1.))".into());
    let xdir = o.add("IFCDIRECTION((1.,0.,0.))".into());
    let world = o.add(format!("IFCAXIS2PLACEMENT3D(#{origin},#{zdir},#{xdir})"));
    let context = o.add(format!("IFCGEOMETRICREPRESENTATIONCONTEXT($,'Model',3,1.E-05,#{world},$)"));
    let body_ctx = o.add(format!("IFCGEOMETRICREPRESENTATIONSUBCONTEXT('Body','Model',*,*,*,*,#{context},$,.MODEL_VIEW.,$)"));
    let title = std::path::Path::new(name).file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "CADaddy".into());
    let project = o.add(format!("IFCPROJECT('{}',$,{},$,$,$,$,(#{context}),#{units})", id(1, 1), step_str(&title)));
    let site_place = o.add(format!("IFCLOCALPLACEMENT($,#{world})"));
    let site = o.add(format!("IFCSITE('{}',$,'Site',$,$,#{site_place},$,$,.ELEMENT.,$,$,$,$,$)", id(1, 2)));
    let bldg_place = o.add(format!("IFCLOCALPLACEMENT(#{site_place},#{world})"));
    let building = o.add(format!("IFCBUILDING('{}',$,'Building',$,$,#{bldg_place},$,$,.ELEMENT.,$,$,$)", id(1, 3)));
    o.add(format!("IFCRELAGGREGATES('{}',$,$,$,#{project},(#{site}))", id(1, 4)));
    o.add(format!("IFCRELAGGREGATES('{}',$,$,$,#{site},(#{building}))", id(1, 5)));
    // A storey's ids come from its name, so adding a level doesn't change the others'.
    let mut storeys = Vec::new();
    for (lname, z) in &levels {
        let p = o.add(format!("IFCCARTESIANPOINT((0.,0.,{}))", real(z * k)));
        let a = o.add(format!("IFCAXIS2PLACEMENT3D(#{p},#{zdir},#{xdir})"));
        let place = o.add(format!("IFCLOCALPLACEMENT(#{bldg_place},#{a})"));
        let st = o.add(format!("IFCBUILDINGSTOREY('{}',$,{},$,$,#{place},$,$,.ELEMENT.,{})", id(hash(lname), 2), step_str(lname), real(z * k)));
        storeys.push((st, place, *z, hash(lname), base_of(lname)));
    }
    o.add(format!("IFCRELAGGREGATES('{}',$,$,$,#{building},{})", id(1, 6), refs(&storeys.iter().map(|s| s.0).collect::<Vec<_>>())));
    let mut contained: Vec<Vec<usize>> = vec![Vec::new(); storeys.len()];
    let mut spaces: Vec<Vec<usize>> = vec![Vec::new(); storeys.len()];
    let mut quantities: Vec<(usize, String, Vec<String>, u64)> = Vec::new();
    // Walls, doors and windows as written: for the openings the doors and windows fill.
    let mut placed: Vec<Placed> = Vec::new();
    for it in &items {
        let si = storey_of(it);
        let Some(&(_, place_parent, sz, _, base)) = storeys.get(si) else { continue };
        // The body in the storey's coordinates (its placement is at the storey's elevation; its
        // plan's base point at the building origin).
        let pts: Vec<String> = it
            .mesh
            .vertices
            .iter()
            .map(|v| format!("({},{},{})", real((v.x - base.x) * k), real((v.y - base.y) * k), real((v.z - sz) * k)))
            .collect();
        let list = o.add(format!("IFCCARTESIANPOINTLIST3D(({}))", pts.join(",")));
        let nv = it.mesh.vertices.len();
        let mut faces = Vec::new();
        for f in &it.mesh.faces {
            if f.v.len() < 3 || f.v.iter().any(|i| *i as usize >= nv) {
                continue;
            }
            let idx: Vec<String> = f.v.iter().map(|i| (i + 1).to_string()).collect();
            faces.push(o.add(format!("IFCINDEXEDPOLYGONALFACE(({}))", idx.join(","))));
        }
        if faces.is_empty() {
            continue;
        }
        let set = o.add(format!("IFCPOLYGONALFACESET(#{list},$,{},$)", refs(&faces)));
        let shape = o.add(format!("IFCSHAPEREPRESENTATION(#{body_ctx},'Body','Tessellation',(#{set}))"));
        let pds = o.add(format!("IFCPRODUCTDEFINITIONSHAPE($,$,(#{shape}))"));
        let place = o.add(format!("IFCLOCALPLACEMENT(#{place_parent},#{world})"));
        let g = element_id(it);
        let kd = &it.kind;
        let nm = step_str(if kd.name.is_empty() { "Element" } else { &kd.name });
        let tag = step_str(&it.e.handle.hex());
        let (entity, _) = kd.class.ifc();
        let pre = kd.predefined;
        let upper = entity.to_ascii_uppercase();
        let line = match kd.class {
            Class::Door | Class::Window => format!("{upper}('{g}',$,{nm},$,$,#{place},#{pds},{tag},$,$,.{pre}.,$,$)"),
            Class::Space => format!(
                "IFCSPACE('{g}',$,{},$,$,#{place},#{pds},{nm},.ELEMENT.,.{pre}.,$)",
                step_str(if kd.long.is_empty() { &kd.name } else { &kd.long })
            ),
            _ => format!("{upper}('{g}',$,{nm},$,$,#{place},#{pds},{tag},.{pre}.)"),
        };
        let product = o.add(line);
        if matches!(kd.class, Class::Wall | Class::Door | Class::Window) {
            let (lo, hi) = it.mesh.vertices.iter().fold(([f64::MAX; 3], [f64::MIN; 3]), |(lo, hi), v| {
                let p = [(v.x - base.x) * k, (v.y - base.y) * k, (v.z - sz) * k];
                (std::array::from_fn(|i| lo[i].min(p[i])), std::array::from_fn(|i| hi[i].max(p[i])))
            });
            placed.push(Placed { class: kd.class, product, place, storey: si, lo, hi, handle: it.e.handle.0 });
        }
        if let Some(list) = if kd.class == Class::Space { spaces.get_mut(si) } else { contained.get_mut(si) } {
            list.push(product);
        }
        // Base quantities (IFC4's Qto_…BaseQuantities): lengths in the length unit, areas in m², volumes in m³.
        let m = measures(it.mesh);
        let (len, a, v) = (|x: f64| real(x * k), |x: f64| real(x * unit_scale * unit_scale), |x: f64| real(x * unit_scale.powi(3)));
        let q_len = |n: &str, x: f64| format!("IFCQUANTITYLENGTH('{n}',$,$,{},$)", len(x));
        let q_area = |n: &str, x: f64| format!("IFCQUANTITYAREA('{n}',$,$,{},$)", a(x));
        let q_vol = |n: &str, x: Option<f64>| x.map(|x| format!("IFCQUANTITYVOLUME('{n}',$,$,{},$)", v(x)));
        let (set_name, qs): (&str, Vec<String>) = match kd.class {
            Class::Wall => {
                ("Qto_WallBaseQuantities", [Some(q_len("Height", m.height)), q_vol("GrossVolume", m.volume)].into_iter().flatten().collect())
            }
            Class::Slab => {
                ("Qto_SlabBaseQuantities", [Some(q_area("GrossArea", m.footprint)), q_vol("GrossVolume", m.volume)].into_iter().flatten().collect())
            }
            Class::Ceiling => ("Qto_CoveringBaseQuantities", vec![q_area("GrossArea", m.footprint)]),
            Class::Column => {
                ("Qto_ColumnBaseQuantities", [Some(q_len("Length", m.height)), q_vol("GrossVolume", m.volume)].into_iter().flatten().collect())
            }
            Class::Door => ("Qto_DoorBaseQuantities", vec![q_len("Height", m.height)]),
            Class::Window => ("Qto_WindowBaseQuantities", vec![q_len("Height", m.height)]),
            Class::Space => (
                "Qto_SpaceBaseQuantities",
                [
                    Some(q_len("Height", m.height)),
                    Some(q_area("GrossFloorArea", m.footprint)),
                    Some(q_area("NetFloorArea", m.footprint)),
                    q_vol("GrossVolume", m.volume),
                ]
                .into_iter()
                .flatten()
                .collect(),
            ),
            _ => ("", Vec::new()),
        };
        if !qs.is_empty() {
            quantities.push((product, set_name.to_string(), qs, it.e.handle.0));
        }
    }
    // Each door and window fills an opening in its wall (IfcOpeningElement, voiding the wall): a
    // box through the wall's thickness over the door's extents, placed relative to the wall. The
    // wall's body already has the hole.
    let near = 0.01 / unit_scale * k;
    for fill in placed.iter().filter(|p| matches!(p.class, Class::Door | Class::Window)) {
        let overlap = |w: &Placed| -> f64 { (0..3).map(|i| (fill.hi[i].min(w.hi[i] + near) - fill.lo[i].max(w.lo[i] - near)).max(0.0)).product() };
        let host = placed
            .iter()
            .filter(|w| w.class == Class::Wall && w.storey == fill.storey)
            .map(|w| (w, overlap(w)))
            .filter(|(_, v)| *v > 0.0)
            .max_by(|a, b| a.1.total_cmp(&b.1))
            .map(|(w, _)| w);
        let Some(wall) = host else { continue };
        // The wall's thin side: the opening goes through all of it.
        let thin = if wall.hi[0] - wall.lo[0] <= wall.hi[1] - wall.lo[1] { 0 } else { 1 };
        let mut lo = fill.lo;
        let mut hi = fill.hi;
        lo[thin] = wall.lo[thin].min(fill.lo[thin]);
        hi[thin] = wall.hi[thin].max(fill.hi[thin]);
        let corner = |i: usize| -> String {
            let pick = |axis: usize, bit: usize| if i >> bit & 1 == 1 { hi[axis] } else { lo[axis] };
            format!("({},{},{})", real(pick(0, 0)), real(pick(1, 1)), real(pick(2, 2)))
        };
        let list = o.add(format!("IFCCARTESIANPOINTLIST3D(({}))", (0..8).map(corner).collect::<Vec<_>>().join(",")));
        // The box's faces, facing out (corners numbered by their x, y, z bits).
        let faces: Vec<usize> = [[1, 3, 4, 2], [5, 6, 8, 7], [1, 2, 6, 5], [3, 7, 8, 4], [1, 5, 7, 3], [2, 4, 8, 6]]
            .iter()
            .map(|f| o.add(format!("IFCINDEXEDPOLYGONALFACE(({},{},{},{}))", f[0], f[1], f[2], f[3])))
            .collect();
        let set = o.add(format!("IFCPOLYGONALFACESET(#{list},.T.,{},$)", refs(&faces)));
        let shape = o.add(format!("IFCSHAPEREPRESENTATION(#{body_ctx},'Body','Tessellation',(#{set}))"));
        let pds = o.add(format!("IFCPRODUCTDEFINITIONSHAPE($,$,(#{shape}))"));
        let place = o.add(format!("IFCLOCALPLACEMENT(#{},#{world})", wall.place));
        let opening = o.add(format!("IFCOPENINGELEMENT('{}',$,'Opening',$,$,#{place},#{pds},$,.OPENING.)", id(fill.handle, 10)));
        o.add(format!("IFCRELVOIDSELEMENT('{}',$,$,$,#{},#{opening})", id(fill.handle, 11), wall.product));
        o.add(format!("IFCRELFILLSELEMENT('{}',$,$,$,#{opening},#{})", id(fill.handle, 12), fill.product));
    }
    for (product, set_name, qs, handle) in quantities {
        let ids: Vec<usize> = qs.into_iter().map(|q| o.add(q)).collect();
        let set = o.add(format!("IFCELEMENTQUANTITY('{}',$,'{set_name}',$,'BaseQuantities',{})", id(handle, 8), refs(&ids)));
        o.add(format!("IFCRELDEFINESBYPROPERTIES('{}',$,$,$,(#{product}),#{set})", id(handle, 9)));
    }
    for ((st, _, _, h, _), (inside, rooms)) in storeys.iter().zip(contained.iter().zip(&spaces)) {
        if !inside.is_empty() {
            o.add(format!("IFCRELCONTAINEDINSPATIALSTRUCTURE('{}',$,$,$,{},#{st})", id(*h, 3), refs(inside)));
        }
        if !rooms.is_empty() {
            o.add(format!("IFCRELAGGREGATES('{}',$,$,$,#{st},{})", id(*h, 4), refs(rooms)));
        }
    }
    let mut s = String::with_capacity(o.lines.iter().map(|l| l.len() + 10).sum::<usize>() + 512);
    s.push_str("ISO-10303-21;\nHEADER;\nFILE_DESCRIPTION(('ViewDefinition [ReferenceView]'),'2;1');\n");
    let _ = writeln!(s, "FILE_NAME({},'',(''),(''),'CADaddy','CADaddy','');", step_str(&format!("{title}.ifc")));
    s.push_str("FILE_SCHEMA(('IFC4'));\nENDSEC;\nDATA;\n");
    for (i, l) in o.lines.iter().enumerate() {
        let _ = writeln!(s, "#{}={l};", i + 1);
    }
    s.push_str("ENDSEC;\nEND-ISO-10303-21;\n");
    Ok(s.into_bytes())
}

// ---------------- what comes in ----------------

/// One parameter of an instance.
#[derive(Clone, Debug, PartialEq)]
enum P {
    Ref(u64),
    Num(f64),
    Str(String),
    Enum(String),
    List(Vec<P>),
    Typed(String, Vec<P>),
    Null,
}

impl P {
    fn num(&self) -> Option<f64> {
        match self {
            P::Num(v) => Some(*v),
            P::Typed(_, a) => a.first().and_then(P::num),
            _ => None,
        }
    }
    fn list(&self) -> &[P] {
        match self {
            P::List(v) => v,
            _ => &[],
        }
    }
    fn reference(&self) -> Option<u64> {
        match self {
            P::Ref(r) => Some(*r),
            _ => None,
        }
    }
    fn text(&self) -> Option<&str> {
        match self {
            P::Str(s) => Some(s),
            _ => None,
        }
    }
}

/// The file's instances: number → (TYPE in upper case, parameters).
type Model = HashMap<u64, (String, Vec<P>)>;

struct Lexer<'a> {
    b: &'a [u8],
    at: usize,
}

impl Lexer<'_> {
    fn skip(&mut self) {
        loop {
            while self.b.get(self.at).is_some_and(u8::is_ascii_whitespace) {
                self.at += 1;
            }
            if self.b.get(self.at..self.at + 2) == Some(b"/*") {
                match self.b.get(self.at + 2..).and_then(|r| r.windows(2).position(|w| w == b"*/")) {
                    Some(e) => self.at += e + 4,
                    None => self.at = self.b.len(),
                }
                continue;
            }
            break;
        }
    }

    /// A STEP string after its opening `'`: `''` is a quote, `\X2\hhhh…\X0\` UTF-16, `\X\hh` a byte.
    fn string(&mut self) -> Option<String> {
        let mut raw = Vec::new();
        loop {
            let c = *self.b.get(self.at)?;
            self.at += 1;
            if c == b'\'' {
                if self.b.get(self.at) == Some(&b'\'') {
                    raw.push(b'\'');
                    self.at += 1;
                    continue;
                }
                break;
            }
            raw.push(c);
            if raw.len() > 1 << 20 {
                return None;
            }
        }
        Some(decode_step_string(&raw))
    }

    fn word(&mut self) -> String {
        let start = self.at;
        while self.b.get(self.at).is_some_and(|c| c.is_ascii_alphanumeric() || *c == b'_') {
            self.at += 1;
        }
        String::from_utf8_lossy(&self.b[start..self.at]).to_ascii_uppercase()
    }

    fn param(&mut self, depth: usize) -> Option<P> {
        if depth > 64 {
            return None;
        }
        self.skip();
        let c = *self.b.get(self.at)?;
        match c {
            b'#' => {
                self.at += 1;
                let start = self.at;
                while self.b.get(self.at).is_some_and(u8::is_ascii_digit) {
                    self.at += 1;
                }
                std::str::from_utf8(&self.b[start..self.at]).ok()?.parse().ok().map(P::Ref)
            }
            b'\'' => {
                self.at += 1;
                self.string().map(P::Str)
            }
            b'.' if self.b.get(self.at + 1).is_some_and(u8::is_ascii_alphabetic) => {
                self.at += 1;
                let w = self.word();
                (self.b.get(self.at) == Some(&b'.')).then(|| {
                    self.at += 1;
                    P::Enum(w)
                })
            }
            b'$' | b'*' => {
                self.at += 1;
                Some(P::Null)
            }
            b'(' => {
                self.at += 1;
                self.list(depth).map(P::List)
            }
            b'"' => {
                // Binary: hex digits.
                self.at += 1;
                while self.b.get(self.at).is_some_and(|c| *c != b'"') {
                    self.at += 1;
                }
                self.at += 1;
                Some(P::Null)
            }
            c if c.is_ascii_digit() || c == b'-' || c == b'+' || c == b'.' => {
                let start = self.at;
                self.at += 1;
                while self.b.get(self.at).is_some_and(|c| c.is_ascii_digit() || matches!(c, b'.' | b'E' | b'e' | b'-' | b'+')) {
                    self.at += 1;
                }
                let t = std::str::from_utf8(&self.b[start..self.at]).ok()?;
                // STEP allows `1.E-05`; Rust parses it as written.
                t.parse::<f64>().ok().filter(|v| v.is_finite()).map(P::Num).or(Some(P::Num(0.0)))
            }
            c if c.is_ascii_alphabetic() => {
                let w = self.word();
                self.skip();
                if self.b.get(self.at) == Some(&b'(') {
                    self.at += 1;
                    self.list(depth).map(|a| P::Typed(w, a))
                } else {
                    Some(P::Enum(w))
                }
            }
            _ => None,
        }
    }

    /// Parameters after `(` up to the matching `)`.
    fn list(&mut self, depth: usize) -> Option<Vec<P>> {
        let mut out = Vec::new();
        loop {
            self.skip();
            match self.b.get(self.at)? {
                b')' => {
                    self.at += 1;
                    return Some(out);
                }
                b',' => self.at += 1,
                _ => {
                    out.push(self.param(depth + 1)?);
                    if out.len() > 50_000_000 {
                        return None;
                    }
                }
            }
        }
    }
}

fn decode_step_string(raw: &[u8]) -> String {
    let t = String::from_utf8_lossy(raw);
    let mut out = String::new();
    let mut rest: &str = &t;
    while let Some(i) = rest.find('\\') {
        out.push_str(&rest[..i]);
        let tail = &rest[i..];
        if let Some(body) = tail.strip_prefix("\\X2\\") {
            let end = body.find("\\X0\\").unwrap_or(body.len());
            let units: Vec<u16> = (0..end / 4).filter_map(|k| u16::from_str_radix(body.get(k * 4..k * 4 + 4)?, 16).ok()).collect();
            out.push_str(&String::from_utf16_lossy(&units));
            rest = body.get(end + 4..).unwrap_or("");
        } else if let Some(body) = tail.strip_prefix("\\X4\\") {
            let end = body.find("\\X0\\").unwrap_or(body.len());
            let chars = (0..end / 8).filter_map(|k| u32::from_str_radix(body.get(k * 8..k * 8 + 8)?, 16).ok()).filter_map(char::from_u32);
            out.extend(chars);
            rest = body.get(end + 4..).unwrap_or("");
        } else if let Some(body) = tail.strip_prefix("\\S\\") {
            // ISO 8859's upper half: the next character + 128.
            let mut cs = body.chars();
            if let Some(c) = cs.next().and_then(|c| char::from_u32(u32::from(c) + 128)) {
                out.push(c);
            }
            rest = cs.as_str();
        } else if tail.starts_with("\\P") && tail.get(3..4) == Some("\\") {
            // A code page directive (\PA\): Latin-1 is read as Latin-1.
            rest = tail.get(4..).unwrap_or("");
        } else if let Some(body) = tail.strip_prefix("\\X\\") {
            if let Some(b) = body.get(..2).and_then(|h| u8::from_str_radix(h, 16).ok()) {
                out.push(char::from(b));
            }
            rest = body.get(2..).unwrap_or("");
        } else if let Some(body) = tail.strip_prefix("\\\\") {
            out.push('\\');
            rest = body;
        } else {
            out.push('\\');
            rest = &tail[1..];
        }
    }
    out.push_str(rest);
    out
}

/// The DATA section's instances.
fn parse(bytes: &[u8]) -> Result<Model> {
    let start = find(bytes, b"DATA;").ok_or_else(|| IoError::Format("not an IFC (STEP) file: no DATA section".into()))? + 5;
    if !bytes.starts_with(b"ISO-10303-21") && find(bytes.get(..bytes.len().min(256)).unwrap_or_default(), b"ISO-10303-21").is_none() {
        return Err(IoError::Format("not an IFC (STEP) file".into()));
    }
    let mut lx = Lexer { b: bytes, at: start };
    let mut m: Model = HashMap::new();
    loop {
        lx.skip();
        match lx.b.get(lx.at) {
            Some(b'#') => {}
            _ => break,
        }
        lx.at += 1;
        let s = lx.at;
        while lx.b.get(lx.at).is_some_and(u8::is_ascii_digit) {
            lx.at += 1;
        }
        let id: Option<u64> = std::str::from_utf8(&lx.b[s..lx.at]).ok().and_then(|t| t.parse().ok());
        lx.skip();
        let ok = lx.b.get(lx.at) == Some(&b'=');
        lx.at += 1;
        lx.skip();
        let name = lx.word();
        lx.skip();
        let params = if ok && lx.b.get(lx.at) == Some(&b'(') {
            lx.at += 1;
            lx.list(0)
        } else {
            None
        };
        // A damaged instance is skipped to the next `;`.
        match (id, params) {
            (Some(id), Some(p)) if !name.is_empty() => {
                m.insert(id, (name, p));
            }
            _ => {}
        }
        match lx.b.get(lx.at..).and_then(|r| r.iter().position(|c| *c == b';')) {
            Some(e) => lx.at += e + 1,
            None => break,
        }
        if m.len() > MAX_INSTANCES {
            return Err(IoError::Format(format!("more than {MAX_INSTANCES} instances")));
        }
    }
    if m.is_empty() {
        return Err(IoError::Format("the IFC file has no instances".into()));
    }
    Ok(m)
}

fn find(h: &[u8], n: &[u8]) -> Option<usize> {
    h.windows(n.len()).position(|w| w == n)
}

/// An affine transform: three rows of [x y z t].
type M = [[f64; 4]; 3];
const ID: M = [[1.0, 0.0, 0.0, 0.0], [0.0, 1.0, 0.0, 0.0], [0.0, 0.0, 1.0, 0.0]];

fn mul(a: &M, b: &M) -> M {
    let mut r = [[0.0; 4]; 3];
    for i in 0..3 {
        for j in 0..4 {
            r[i][j] = a[i][0] * b[0][j] + a[i][1] * b[1][j] + a[i][2] * b[2][j] + if j == 3 { a[i][3] } else { 0.0 };
        }
    }
    r
}

fn apply(m: &M, p: Vec3) -> Vec3 {
    Vec3::new(
        m[0][0] * p.x + m[0][1] * p.y + m[0][2] * p.z + m[0][3],
        m[1][0] * p.x + m[1][1] * p.y + m[1][2] * p.z + m[1][3],
        m[2][0] * p.x + m[2][1] * p.y + m[2][2] * p.z + m[2][3],
    )
}

struct Reader<'a> {
    m: &'a Model,
}

impl Reader<'_> {
    fn get(&self, p: &P) -> Option<&(String, Vec<P>)> {
        self.m.get(&p.reference()?)
    }

    fn coords(&self, p: &P) -> Vec<f64> {
        match self.get(p) {
            Some((_, a)) => a.first().map(|l| l.list().iter().filter_map(P::num).collect()).unwrap_or_default(),
            None => Vec::new(),
        }
    }

    fn point(&self, p: &P) -> Option<Vec3> {
        let c = self.coords(p);
        Some(Vec3::new(*c.first()?, *c.get(1)?, c.get(2).copied().unwrap_or(0.0)))
    }

    fn direction(&self, p: &P) -> Option<Vec3> {
        let v = self.point(p)?;
        let l = v.len();
        (l > 1e-12 && l.is_finite()).then(|| v * (1.0 / l))
    }

    /// An IfcAxis2Placement3D (or 2D) as a transform.
    fn axis(&self, p: &P) -> M {
        let Some((name, a)) = self.get(p) else { return ID };
        let o = a.first().and_then(|x| self.point(x)).unwrap_or_default();
        let (z, xr) = if name == "IFCAXIS2PLACEMENT2D" {
            (Vec3::new(0.0, 0.0, 1.0), a.get(1).and_then(|x| self.direction(x)).unwrap_or(Vec3::new(1.0, 0.0, 0.0)))
        } else {
            (
                a.get(1).and_then(|x| self.direction(x)).unwrap_or(Vec3::new(0.0, 0.0, 1.0)),
                a.get(2).and_then(|x| self.direction(x)).unwrap_or(Vec3::new(1.0, 0.0, 0.0)),
            )
        };
        let mut x = xr - z * xr.dot(z);
        if x.len() < 1e-9 {
            x = if z.x.abs() < 0.9 { Vec3::new(1.0, 0.0, 0.0) } else { Vec3::new(0.0, 1.0, 0.0) };
            x = x - z * x.dot(z);
        }
        let x = x * (1.0 / x.len());
        let y = z.cross(x);
        [[x.x, y.x, z.x, o.x], [x.y, y.y, z.y, o.y], [x.z, y.z, z.z, o.z]]
    }

    /// An IfcLocalPlacement chain as a transform to world.
    fn placement(&self, p: &P, depth: usize) -> M {
        if depth > MAX_DEPTH {
            return ID;
        }
        let Some((name, a)) = self.get(p) else { return ID };
        if name != "IFCLOCALPLACEMENT" {
            return ID;
        }
        let parent = a.first().filter(|x| **x != P::Null).map_or(ID, |x| self.placement(x, depth + 1));
        let local = a.get(1).map_or(ID, |x| self.axis(x));
        mul(&parent, &local)
    }

    /// The faces (outer loops, local coordinates) of geometry item `p`, transformed by `t`.
    fn item(&self, p: &P, t: &M, depth: usize, out: &mut Vec<Vec<Vec3>>) {
        if depth > MAX_DEPTH || out.len() > MAX_FACES {
            return;
        }
        let Some((name, a)) = self.get(p) else { return };
        match name.as_str() {
            "IFCPOLYGONALFACESET" => {
                let pts = self.point_list(a.first());
                let pn: Vec<usize> = a.get(3).map(|l| l.list().iter().filter_map(|x| x.num().map(|v| v as usize)).collect()).unwrap_or_default();
                let at = |i: usize| -> Option<Vec3> {
                    let j = if pn.is_empty() { i } else { *pn.get(i.checked_sub(1)?)? };
                    pts.get(j.checked_sub(1)?).map(|v| apply(t, *v))
                };
                for f in a.get(2).map(P::list).unwrap_or_default() {
                    let Some((_, fa)) = self.get(f) else { continue };
                    let face: Option<Vec<Vec3>> = fa.first().map(P::list).unwrap_or_default().iter().map(|i| at(i.num()? as usize)).collect();
                    if let Some(face) = face.filter(|f| f.len() >= 3) {
                        out.push(face);
                    }
                }
            }
            "IFCTRIANGULATEDFACESET" => {
                let pts = self.point_list(a.first());
                let pn: Vec<usize> = a.get(4).map(|l| l.list().iter().filter_map(|x| x.num().map(|v| v as usize)).collect()).unwrap_or_default();
                let at = |i: usize| -> Option<Vec3> {
                    let j = if pn.is_empty() { i } else { *pn.get(i.checked_sub(1)?)? };
                    pts.get(j.checked_sub(1)?).map(|v| apply(t, *v))
                };
                for tri in a.get(3).map(P::list).unwrap_or_default() {
                    let face: Option<Vec<Vec3>> = tri.list().iter().map(|i| at(i.num()? as usize)).collect();
                    if let Some(face) = face.filter(|f| f.len() == 3) {
                        out.push(face);
                    }
                }
            }
            "IFCFACETEDBREP"
            | "IFCFACEBASEDSURFACEMODEL"
            | "IFCSHELLBASEDSURFACEMODEL"
            | "IFCCLOSEDSHELL"
            | "IFCOPENSHELL"
            | "IFCCONNECTEDFACESET" => {
                let children: Vec<&P> = match name.as_str() {
                    "IFCFACETEDBREP" => a.first().into_iter().collect(),
                    "IFCCLOSEDSHELL" | "IFCOPENSHELL" | "IFCCONNECTEDFACESET" => {
                        // Faces directly.
                        for f in a.first().map(P::list).unwrap_or_default() {
                            if let Some(face) = self.face(f, t) {
                                out.push(face);
                            }
                            if out.len() > MAX_FACES {
                                return;
                            }
                        }
                        Vec::new()
                    }
                    _ => a.first().map(P::list).unwrap_or_default().iter().collect(),
                };
                for c in children {
                    self.item(c, t, depth + 1, out);
                }
            }
            "IFCEXTRUDEDAREASOLID" => {
                let profile = a.first().map(|x| self.profile(x)).unwrap_or_default();
                if profile.len() < 3 {
                    return;
                }
                let pos = a.get(1).map_or(ID, |x| self.axis(x));
                let dir = a.get(2).and_then(|x| self.direction(x)).unwrap_or(Vec3::new(0.0, 0.0, 1.0));
                let depth_v = a.get(3).and_then(P::num).unwrap_or(0.0);
                if !depth_v.is_finite() || depth_v.abs() < 1e-12 {
                    return;
                }
                let m = mul(t, &pos);
                let off = dir * depth_v;
                let bottom: Vec<Vec3> = profile.iter().map(|q| apply(&m, *q)).collect();
                let top: Vec<Vec3> = profile.iter().map(|q| apply(&m, *q + off)).collect();
                out.push(bottom.iter().rev().copied().collect());
                out.push(top.clone());
                let n = profile.len();
                for i in 0..n {
                    let j = (i + 1) % n;
                    out.push(vec![bottom[i], bottom[j], top[j], top[i]]);
                }
            }
            "IFCMAPPEDITEM" => {
                let Some((_, src)) = a.first().and_then(|x| self.get(x)) else { return };
                let origin = src.first().map_or(ID, |x| self.axis(x));
                let target = a.get(1).map_or(ID, |x| self.operator(x));
                let m = mul(&mul(t, &target), &origin);
                if let Some((_, rep)) = src.get(1).and_then(|x| self.get(x)) {
                    for it in rep.get(3).map(P::list).unwrap_or_default() {
                        self.item(it, &m, depth + 1, out);
                    }
                }
            }
            "IFCBOOLEANRESULT" | "IFCBOOLEANCLIPPINGRESULT" => {
                // The body before openings and clips are taken away.
                if let Some(first) = a.get(1) {
                    self.item(first, t, depth + 1, out);
                }
            }
            _ => {}
        }
    }

    fn point_list(&self, p: Option<&P>) -> Vec<Vec3> {
        let Some((_, a)) = p.and_then(|x| self.get(x)) else { return Vec::new() };
        a.first()
            .map(P::list)
            .unwrap_or_default()
            .iter()
            .take(MAX_POINTS)
            .filter_map(|c| {
                let v: Vec<f64> = c.list().iter().filter_map(P::num).collect();
                Some(Vec3::new(*v.first()?, *v.get(1)?, v.get(2).copied().unwrap_or(0.0)))
            })
            .collect()
    }

    /// An IfcFace's outer loop.
    fn face(&self, f: &P, t: &M) -> Option<Vec<Vec3>> {
        let (_, fa) = self.get(f)?;
        let bounds = fa.first()?.list();
        let outer = bounds.iter().find(|b| self.get(b).is_some_and(|(n, _)| n == "IFCFACEOUTERBOUND")).or_else(|| bounds.first())?;
        let (_, ba) = self.get(outer)?;
        let (lname, la) = self.get(ba.first()?)?;
        if lname != "IFCPOLYLOOP" {
            return None;
        }
        let pts: Vec<Vec3> = la.first()?.list().iter().filter_map(|p| self.point(p)).map(|v| apply(t, v)).collect();
        (pts.len() >= 3).then_some(pts)
    }

    /// A profile's outline in its own plane (z = 0).
    fn profile(&self, p: &P) -> Vec<Vec3> {
        let Some((name, a)) = self.get(p) else { return Vec::new() };
        match name.as_str() {
            "IFCRECTANGLEPROFILEDEF" => {
                let pos = a.get(2).map_or(ID, |x| self.axis(x));
                let (w, h) = (a.get(3).and_then(P::num).unwrap_or(0.0) / 2.0, a.get(4).and_then(P::num).unwrap_or(0.0) / 2.0);
                [(-w, -h), (w, -h), (w, h), (-w, h)].iter().map(|(x, y)| apply(&pos, Vec3::new(*x, *y, 0.0))).collect()
            }
            "IFCCIRCLEPROFILEDEF" => {
                let pos = a.get(2).map_or(ID, |x| self.axis(x));
                let r = a.get(3).and_then(P::num).unwrap_or(0.0);
                (0..24)
                    .map(|i| {
                        let t = std::f64::consts::TAU * f64::from(i) / 24.0;
                        apply(&pos, Vec3::new(r * t.cos(), r * t.sin(), 0.0))
                    })
                    .collect()
            }
            "IFCARBITRARYCLOSEDPROFILEDEF" | "IFCARBITRARYPROFILEDEFWITHVOIDS" => {
                let mut pts = a.get(2).map(|c| self.curve(c)).unwrap_or_default();
                if pts.len() > 1 && (pts[0] - pts[pts.len() - 1]).len() < 1e-9 {
                    pts.pop();
                }
                pts
            }
            _ => Vec::new(),
        }
    }

    fn curve(&self, p: &P) -> Vec<Vec3> {
        let Some((name, a)) = self.get(p) else { return Vec::new() };
        match name.as_str() {
            "IFCPOLYLINE" => a.first().map(P::list).unwrap_or_default().iter().take(MAX_POINTS).filter_map(|x| self.point(x)).collect(),
            "IFCINDEXEDPOLYCURVE" => self.point_list(a.first()),
            _ => Vec::new(),
        }
    }

    /// An IfcCartesianTransformationOperator3D (axes, origin, scale) as a transform.
    fn operator(&self, p: &P) -> M {
        let Some((_, a)) = self.get(p) else { return ID };
        let x = a.first().and_then(|v| self.direction(v)).unwrap_or(Vec3::new(1.0, 0.0, 0.0));
        let y = a.get(1).and_then(|v| self.direction(v)).unwrap_or(Vec3::new(0.0, 1.0, 0.0));
        let o = a.get(2).and_then(|v| self.point(v)).unwrap_or_default();
        let s = a.get(3).and_then(P::num).filter(|s| s.is_finite() && *s > 0.0).unwrap_or(1.0);
        let z = a.get(4).and_then(|v| self.direction(v)).unwrap_or_else(|| x.cross(y));
        [[x.x * s, y.x * s, z.x * s, o.x], [x.y * s, y.y * s, z.y * s, o.y], [x.z * s, y.z * s, z.z * s, o.z]]
    }

    /// Metres per length unit of the project (1 when the file says none).
    fn unit(&self) -> f64 {
        let Some((_, project)) = self.m.values().find(|(n, _)| n == "IFCPROJECT") else { return 1.0 };
        let Some((_, ua)) = project.get(8).and_then(|x| self.get(x)) else { return 1.0 };
        for u in ua.first().map(P::list).unwrap_or_default() {
            let Some((name, a)) = self.get(u) else { continue };
            if a.get(1) != Some(&P::Enum("LENGTHUNIT".into())) {
                continue;
            }
            match name.as_str() {
                "IFCSIUNIT" => {
                    let prefix = match a.get(2) {
                        Some(P::Enum(p)) => match p.as_str() {
                            "MILLI" => 0.001,
                            "CENTI" => 0.01,
                            "DECI" => 0.1,
                            "KILO" => 1000.0,
                            "MICRO" => 1e-6,
                            _ => 1.0,
                        },
                        _ => 1.0,
                    };
                    return prefix;
                }
                "IFCCONVERSIONBASEDUNIT" => {
                    let factor = a.get(3).and_then(|x| self.get(x)).and_then(|(_, m)| m.first()?.num());
                    if let Some(f) = factor.filter(|f| f.is_finite() && *f > 0.0) {
                        // The factor is in the conversion's own unit, usually metres or millimetres.
                        return if f > 1.0 { f * 0.001 } else { f };
                    }
                    let name = a.get(2).and_then(P::text).unwrap_or_default().to_ascii_uppercase();
                    return if name.contains("INCH") {
                        0.0254
                    } else if name.contains("FOOT") || name.contains("FEET") {
                        0.3048
                    } else {
                        1.0
                    };
                }
                _ => {}
            }
        }
        1.0
    }
}

/// Spatial structure and openings: not objects of their own.
const NOT_OBJECTS: [&str; 6] = ["IFCPROJECT", "IFCSITE", "IFCBUILDING", "IFCBUILDINGSTOREY", "IFCOPENINGELEMENT", "IFCANNOTATION"];

/// `IFCWALLSTANDARDCASE` → `WallStandardCase`, as the IFC schema writes it.
fn class_name(upper: &str) -> String {
    const KNOWN: [&str; 28] = [
        "WallStandardCase",
        "Wall",
        "Slab",
        "Space",
        "Door",
        "Window",
        "Covering",
        "Roof",
        "Stair",
        "StairFlight",
        "Railing",
        "Column",
        "Beam",
        "Member",
        "Plate",
        "CurtainWall",
        "Footing",
        "Pile",
        "Ramp",
        "BuildingElementProxy",
        "FurnishingElement",
        "FlowTerminal",
        "FlowSegment",
        "DistributionElement",
        "DuctSegment",
        "PipeSegment",
        "CableCarrierSegment",
        "GeographicElement",
    ];
    let bare = upper.strip_prefix("IFC").unwrap_or(upper);
    KNOWN.iter().find(|k| k.eq_ignore_ascii_case(bare)).map_or_else(
        || {
            let mut s = bare.to_ascii_lowercase();
            if let Some(f) = s.get_mut(..1) {
                f.make_ascii_uppercase();
            }
            s
        },
        |k| (*k).to_string(),
    )
}

/// A mesh of `faces` with equal points welded.
fn mesh_of(faces: &[Vec<Vec3>]) -> Option<Mesh> {
    let mut index: HashMap<[u64; 3], u32> = HashMap::new();
    let mut m = Mesh { kind: MeshKind::Polyface, ..Mesh::default() };
    for f in faces {
        let mut v = Vec::with_capacity(f.len());
        for p in f {
            if !(p.x.is_finite() && p.y.is_finite() && p.z.is_finite()) {
                continue;
            }
            let key = [p.x.to_bits(), p.y.to_bits(), p.z.to_bits()];
            let next = u32::try_from(m.vertices.len()).ok()?;
            let i = *index.entry(key).or_insert_with(|| {
                m.vertices.push(*p);
                next
            });
            if v.last() != Some(&i) {
                v.push(i);
            }
        }
        if v.len() > 2 && v.first() == v.last() {
            v.pop();
        }
        if v.len() >= 3 {
            m.faces.push(MeshFace { v, hidden: 0 });
        }
    }
    (!m.faces.is_empty()).then_some(m)
}

/// An IFC file as a drawing: a mesh per product with a body, on a layer per class, in metres.
pub fn read_ifc(bytes: &[u8]) -> Result<Drawing> {
    let model = parse(bytes)?;
    let r = Reader { m: &model };
    let unit = r.unit();
    let scale: M = [[unit, 0.0, 0.0, 0.0], [0.0, unit, 0.0, 0.0], [0.0, 0.0, unit, 0.0]];
    let mut d = Drawing::new_metric();
    d.header.set_i64("INSUNITS", 6);
    let mut ids: Vec<&u64> = model.keys().collect();
    ids.sort_unstable();
    let mut total_faces = 0usize;
    let mut n = 0usize;
    for id in ids {
        let Some((name, a)) = model.get(id) else { continue };
        if NOT_OBJECTS.contains(&name.as_str()) || a.len() < 7 {
            continue;
        }
        // An IfcProduct: GlobalId, OwnerHistory, Name, Description, ObjectType, ObjectPlacement, Representation…
        let Some((pds_name, pds)) = a.get(6).and_then(|x| r.get(x)) else { continue };
        if pds_name != "IFCPRODUCTDEFINITIONSHAPE" {
            continue;
        }
        let place = mul(&scale, &a.get(5).map_or(ID, |x| r.placement(x, 0)));
        let mut faces = Vec::new();
        for rep in pds.get(2).map(P::list).unwrap_or_default() {
            let Some((_, ra)) = r.get(rep) else { continue };
            // The body; an axis, box or footprint representation would draw a second outline.
            let id = ra.get(1).and_then(P::text).unwrap_or_default();
            if !id.is_empty() && !id.eq_ignore_ascii_case("Body") && !id.eq_ignore_ascii_case("Facetation") {
                continue;
            }
            for it in ra.get(3).map(P::list).unwrap_or_default() {
                r.item(it, &place, 0, &mut faces);
            }
        }
        total_faces = total_faces.saturating_add(faces.len());
        if total_faces > MAX_FACES {
            return Err(IoError::Format(format!("more than {MAX_FACES} faces")));
        }
        let Some(mesh) = mesh_of(&faces) else { continue };
        if mesh.vertices.len() > MAX_POINTS {
            continue;
        }
        let class = class_name(name);
        let layer = format!("IFC-{class}");
        if d.layer(&layer).is_none() {
            d.layers.push(Layer { name: layer.clone(), ..Layer::default() });
        }
        let guid = a.first().and_then(P::text).unwrap_or_default();
        let label = a.get(2).and_then(P::text).unwrap_or_default();
        let xdata = vec![
            RawTag { code: 1001, value: APP.into() },
            RawTag { code: 1000, value: class.clone() },
            RawTag { code: 1000, value: guid.chars().take(64).collect() },
            RawTag { code: 1000, value: label.chars().take(250).collect() },
        ];
        let mut c = Common { layer, xdata, ..Common::default() };
        if class == "Space" {
            c.transparency = cadaddy_doc::Transparency::Percent(80);
        }
        d.add(&Space::Model, c, EntityKind::Mesh(mesh)).map_err(|e| IoError::Format(e.to_string()))?;
        n += 1;
    }
    if n == 0 {
        return Err(IoError::Format("no building elements with a body in the IFC file".into()));
    }
    Ok(d)
}

/// The IFC class and GlobalId of an object read from IFC.
pub fn ifc_tag(e: &Entity) -> Option<(String, String)> {
    let t = tag_strings(e, APP)?;
    Some((t.first()?.clone(), t.get(1).cloned().unwrap_or_default()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strings_and_reals_are_valid_step() {
        assert_eq!(step_str("It's a \\ test \u{e9}"), "'It''s a \\\\ test \\X2\\00E9\\X0\\'");
        assert_eq!(decode_step_string(b"It's a \\\\ test \\X2\\00E9\\X0\\"), "It's a \\ test \u{e9}");
        assert_eq!(real(1.0), "1.");
        assert_eq!(real(-0.0), "0.");
        assert_eq!(real(2.5), "2.5");
        // Above U+FFFF: \X4\ (ISO 10303-21), not UTF-16 surrogates in \X2\.
        assert_eq!(step_str("Room \u{1F3E0}"), "'Room \\X4\\0001F3E0\\X0\\'");
        assert_eq!(decode_step_string(b"Room \\X4\\0001F3E0\\X0\\"), "Room \u{1F3E0}");
        // \S\ shifts the next character into ISO 8859's upper half; \PA\ picks a code page.
        assert_eq!(decode_step_string(b"Caf\\S\\i \\PA\\ok"), "Caf\u{e9} ok");
        assert!(is_guid(&guid(7, 9)) && !is_guid("4zzzzzzzzzzzzzzzzzzzzz") && !is_guid("short"));
        assert_eq!(guid(1, 2).len(), 22);
        assert_eq!(guid(1, 2), guid(1, 2));
        assert_ne!(guid(1, 2), guid(1, 3));
    }

    /// A hand-written IFC: an extruded rectangle wall in millimetres, placed through a chain, and
    /// a triangulated slab.
    #[test]
    fn extrusions_face_sets_placements_and_units_are_read() {
        let ifc = b"ISO-10303-21;\nHEADER;\nFILE_SCHEMA(('IFC4'));\nENDSEC;\nDATA;\n\
            #1=IFCSIUNIT(*,.LENGTHUNIT.,.MILLI.,.METRE.);\n#2=IFCUNITASSIGNMENT((#1));\n\
            #3=IFCPROJECT('p',$,'P',$,$,$,$,$,#2);\n\
            #10=IFCCARTESIANPOINT((1000.,2000.,0.));\n#11=IFCAXIS2PLACEMENT3D(#10,$,$);\n#12=IFCLOCALPLACEMENT($,#11);\n\
            #13=IFCCARTESIANPOINT((0.,0.,3000.));\n#14=IFCAXIS2PLACEMENT3D(#13,$,$);\n#15=IFCLOCALPLACEMENT(#12,#14);\n\
            #20=IFCCARTESIANPOINT((0.,0.));\n#21=IFCAXIS2PLACEMENT2D(#20,$);\n\
            #22=IFCRECTANGLEPROFILEDEF(.AREA.,$,#21,4000.,200.);\n#23=IFCDIRECTION((0.,0.,1.));\n\
            #24=IFCEXTRUDEDAREASOLID(#22,$,#23,2500.);\n#25=IFCSHAPEREPRESENTATION($,'Body','SweptSolid',(#24));\n\
            #26=IFCPRODUCTDEFINITIONSHAPE($,$,(#25));\n#27=IFCWALL('w1',$,'Wall /* not a comment */',$,$,#15,#26,$,$);\n\
            /* a comment */\n\
            #30=IFCCARTESIANPOINTLIST3D(((0.,0.,0.),(1000.,0.,0.),(0.,1000.,0.)));\n\
            #31=IFCTRIANGULATEDFACESET(#30,$,.T.,((1,2,3)),$);\n#32=IFCSHAPEREPRESENTATION($,'Body','Tessellation',(#31));\n\
            #33=IFCPRODUCTDEFINITIONSHAPE($,$,(#32));\n#34=IFCSLAB('s1',$,'Slab',$,$,$,#33,$,.FLOOR.);\n\
            ENDSEC;\nEND-ISO-10303-21;\n";
        let d = read_ifc(ifc).unwrap();
        let walls: Vec<&Entity> = d.model.iter().filter(|e| e.common.layer == "IFC-Wall").map(|e| e.as_ref()).collect();
        assert_eq!(walls.len(), 1);
        let EntityKind::Mesh(m) = &walls[0].kind else { panic!("a mesh") };
        assert_eq!(m.vertices.len(), 8);
        assert_eq!(m.faces.len(), 6);
        let (lo, hi) = m.vertices.iter().fold((Vec3::new(f64::MAX, f64::MAX, f64::MAX), Vec3::new(f64::MIN, f64::MIN, f64::MIN)), |(lo, hi), v| {
            (Vec3::new(lo.x.min(v.x), lo.y.min(v.y), lo.z.min(v.z)), Vec3::new(hi.x.max(v.x), hi.y.max(v.y), hi.z.max(v.z)))
        });
        // Metres: x 1±2, y 2±0.1, z 3..5.5.
        assert!((lo.x + 1.0).abs() < 1e-9 && (hi.x - 3.0).abs() < 1e-9, "{lo:?} {hi:?}");
        assert!((lo.y - 1.9).abs() < 1e-9 && (hi.z - 5.5).abs() < 1e-9, "{lo:?} {hi:?}");
        assert_eq!(ifc_tag(walls[0]), Some(("Wall".into(), "w1".into())));
        assert_eq!(d.model.iter().filter(|e| e.common.layer == "IFC-Slab").count(), 1);
    }

    #[test]
    fn garbage_and_hostile_files_are_refused() {
        for bad in [&b""[..], b"hello", b"ISO-10303-21;\nDATA;\nENDSEC;\n", b"ISO-10303-21;\nDATA;\n#1=IFCWALL((((((((("] {
            assert!(read_ifc(bad).is_err());
        }
        // A placement that refers to itself ends.
        let ifc = b"ISO-10303-21;\nDATA;\n#1=IFCLOCALPLACEMENT(#1,$);\n#2=IFCCARTESIANPOINTLIST3D(((0.,0.,0.),(1.,0.,0.),(0.,1.,0.)));\n\
            #3=IFCTRIANGULATEDFACESET(#2,$,.T.,((1,2,3)),$);\n#4=IFCSHAPEREPRESENTATION($,'Body','T',(#3));\n\
            #5=IFCPRODUCTDEFINITIONSHAPE($,$,(#4));\n#6=IFCWALL('w',$,$,$,$,#1,#5,$,$);\nENDSEC;\n";
        assert_eq!(read_ifc(ifc).unwrap().model.len(), 1);
        // A mapped item that maps itself ends too.
        let ifc = b"ISO-10303-21;\nDATA;\n#1=IFCREPRESENTATIONMAP($,#2);\n#2=IFCSHAPEREPRESENTATION($,'Body','M',(#3));\n\
            #3=IFCMAPPEDITEM(#1,$);\n#5=IFCPRODUCTDEFINITIONSHAPE($,$,(#2));\n#6=IFCWALL('w',$,$,$,$,$,#5,$,$);\nENDSEC;\n";
        assert!(read_ifc(ifc).is_err(), "no faces");
    }
}
