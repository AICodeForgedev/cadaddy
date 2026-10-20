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

/// What an object is in IFC, from what WALLGEN or ROOMS tagged it as.
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
}

/// The tag `app`'s first strings on `e` (WALLGEN: part, level; ROOMS: name, number).
fn tag_strings(e: &Entity, app: &str) -> Option<Vec<String>> {
    let x = &e.common.xdata;
    let at = x.iter().position(|t| t.code == 1001 && t.value.trim().eq_ignore_ascii_case(app))?;
    Some(x.iter().skip(at + 1).take_while(|t| t.code != 1001).filter(|t| t.code == 1000).map(|t| t.value.clone()).collect())
}

fn class_of(e: &Entity) -> (Class, Option<String>, String, String) {
    if let Some(t) = tag_strings(e, "CADADDY_WALLGEN") {
        let part = t.first().map(String::as_str).unwrap_or_default();
        let class = match part {
            "wall" => Class::Wall,
            "floor" => Class::Slab,
            "ceiling" => Class::Ceiling,
            "door" => Class::Door,
            "window" => Class::Window,
            "stair" => Class::Stair,
            _ => Class::Proxy,
        };
        return (class, t.get(1).cloned(), part.to_string(), String::new());
    }
    if let Some(t) = tag_strings(e, "CADADDY_ROOM") {
        return (Class::Space, None, t.first().cloned().unwrap_or_default(), t.get(1).cloned().unwrap_or_default());
    }
    (Class::Proxy, None, e.common.layer.clone(), String::new())
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
            c => {
                out.push_str("\\X2\\");
                let mut buf = [0u16; 2];
                for u in c.encode_utf16(&mut buf) {
                    let _ = write!(out, "{u:04X}");
                }
                out.push_str("\\X0\\");
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

/// The drawing's model-space meshes as an IFC4 file (`name` goes into the header).
pub fn write_ifc(d: &Drawing, name: &str) -> Result<Vec<u8>> {
    let scale = cadaddy_doc::geo::metres_per_unit(d.header.i64("INSUNITS", 0)).max(1e-9);
    let unit_scale = if d.header.i64("INSUNITS", 0) == 0 { if d.header.i64("MEASUREMENT", 1) == 1 { 0.001 } else { 0.0254 } } else { scale };
    struct Item<'a> {
        e: &'a Entity,
        mesh: &'a Mesh,
        class: Class,
        level: Option<String>,
        name: String,
        long: String,
        zmin: f64,
    }
    let mut items: Vec<Item> = Vec::new();
    for e in d.model.iter() {
        let EntityKind::Mesh(m) = &e.kind else { continue };
        if m.faces.is_empty() || m.vertices.is_empty() {
            continue;
        }
        let (class, level, name, long) = class_of(e);
        let zmin = m.vertices.iter().map(|v| v.z).fold(f64::MAX, f64::min);
        items.push(Item { e, mesh: m, class, level, name, long, zmin });
    }
    if items.is_empty() {
        return Err(IoError::Format("nothing to write to IFC: no meshes in model space (make the building with WALLGEN or ROOMS first)".into()));
    }
    // Storeys: WALLGEN's levels, lowest first, at their lowest wall or slab top; one when none.
    let mut levels: Vec<(String, f64)> = Vec::new();
    for it in &items {
        if let Some(l) = &it.level {
            let z = if it.class == Class::Slab { it.mesh.vertices.iter().map(|v| v.z).fold(f64::MIN, f64::max) } else { it.zmin };
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
    let storey_of = |it: &Item| -> usize {
        if let Some(l) = &it.level
            && let Some(i) = levels.iter().position(|(n, _)| n == l)
        {
            return i;
        }
        // The highest storey at or below the object's foot.
        levels.iter().rposition(|(_, z)| *z <= it.zmin + 1e-6).unwrap_or(0)
    };

    let mut o = Out { lines: Vec::new() };
    let unit = match unit_scale {
        s if (s - 0.001).abs() < 1e-12 => o.add("IFCSIUNIT(*,.LENGTHUNIT.,.MILLI.,.METRE.)".into()),
        _ => o.add("IFCSIUNIT(*,.LENGTHUNIT.,$,.METRE.)".into()),
    };
    // Coordinates go out in the unit declared: millimetres for metric drawings, else metres.
    let k = if (unit_scale - 0.001).abs() < 1e-12 { 1.0 } else { scale };
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
    let project = o.add(format!("IFCPROJECT('{}',$,{},$,$,$,$,(#{context}),#{units})", guid(1, 1), step_str(&title)));
    let site_place = o.add(format!("IFCLOCALPLACEMENT($,#{world})"));
    let site = o.add(format!("IFCSITE('{}',$,'Site',$,$,#{site_place},$,$,.ELEMENT.,$,$,$,$,$)", guid(1, 2)));
    let bldg_place = o.add(format!("IFCLOCALPLACEMENT(#{site_place},#{world})"));
    let building = o.add(format!("IFCBUILDING('{}',$,'Building',$,$,#{bldg_place},$,$,.ELEMENT.,$,$,$)", guid(1, 3)));
    o.add(format!("IFCRELAGGREGATES('{}',$,$,$,#{project},(#{site}))", guid(1, 4)));
    o.add(format!("IFCRELAGGREGATES('{}',$,$,$,#{site},(#{building}))", guid(1, 5)));
    let mut storeys = Vec::new();
    for (i, (lname, z)) in levels.iter().enumerate() {
        let p = o.add(format!("IFCCARTESIANPOINT((0.,0.,{}))", real(z * k)));
        let a = o.add(format!("IFCAXIS2PLACEMENT3D(#{p},#{zdir},#{xdir})"));
        let place = o.add(format!("IFCLOCALPLACEMENT(#{bldg_place},#{a})"));
        let st = o.add(format!("IFCBUILDINGSTOREY('{}',$,{},$,$,#{place},$,$,.ELEMENT.,{})", guid(2, i as u64), step_str(lname), real(z * k)));
        storeys.push((st, place, *z));
    }
    o.add(format!("IFCRELAGGREGATES('{}',$,$,$,#{building},{})", guid(1, 6), refs(&storeys.iter().map(|s| s.0).collect::<Vec<_>>())));
    let mut contained: Vec<Vec<usize>> = vec![Vec::new(); storeys.len()];
    let mut spaces: Vec<Vec<usize>> = vec![Vec::new(); storeys.len()];
    let mut counts: HashMap<&'static str, usize> = HashMap::new();
    for it in &items {
        let si = storey_of(it);
        let (_, place_parent, sz) = storeys[si];
        // The body in the storey's coordinates (its placement is at the storey's elevation).
        let pts: Vec<String> = it.mesh.vertices.iter().map(|v| format!("({},{},{})", real(v.x * k), real(v.y * k), real((v.z - sz) * k))).collect();
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
        let g = guid(it.e.handle.0, 7);
        let nm = step_str(if it.name.is_empty() { "Element" } else { &it.name });
        let tag = step_str(&it.e.handle.hex());
        let (kind, line) = match it.class {
            Class::Wall => ("IfcWall", format!("IFCWALL('{g}',$,{nm},$,$,#{place},#{pds},{tag},.STANDARD.)")),
            Class::Slab => ("IfcSlab", format!("IFCSLAB('{g}',$,{nm},$,$,#{place},#{pds},{tag},.FLOOR.)")),
            Class::Ceiling => ("IfcCovering", format!("IFCCOVERING('{g}',$,{nm},$,$,#{place},#{pds},{tag},.CEILING.)")),
            Class::Door => ("IfcDoor", format!("IFCDOOR('{g}',$,{nm},$,$,#{place},#{pds},{tag},$,$,.DOOR.,$,$)")),
            Class::Window => ("IfcWindow", format!("IFCWINDOW('{g}',$,{nm},$,$,#{place},#{pds},{tag},$,$,.WINDOW.,$,$)")),
            Class::Stair => ("IfcStair", format!("IFCSTAIR('{g}',$,{nm},$,$,#{place},#{pds},{tag},.NOTDEFINED.)")),
            Class::Space => (
                "IfcSpace",
                format!(
                    "IFCSPACE('{g}',$,{},$,$,#{place},#{pds},{nm},.ELEMENT.,.INTERNAL.,$)",
                    step_str(if it.long.is_empty() { &it.name } else { &it.long })
                ),
            ),
            Class::Proxy => ("IfcBuildingElementProxy", format!("IFCBUILDINGELEMENTPROXY('{g}',$,{nm},$,$,#{place},#{pds},{tag},.NOTDEFINED.)")),
        };
        let id = o.add(line);
        *counts.entry(kind).or_default() += 1;
        if it.class == Class::Space { spaces[si].push(id) } else { contained[si].push(id) }
    }
    for (i, (st, _, _)) in storeys.iter().enumerate() {
        if !contained[i].is_empty() {
            o.add(format!("IFCRELCONTAINEDINSPATIALSTRUCTURE('{}',$,$,$,{},#{st})", guid(3, i as u64), refs(&contained[i])));
        }
        if !spaces[i].is_empty() {
            o.add(format!("IFCRELAGGREGATES('{}',$,$,$,#{st},{})", guid(4, i as u64), refs(&spaces[i])));
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
        } else if let Some(body) = tail.strip_prefix("\\X\\") {
            if let Some(b) = body.get(..2).and_then(|h| u8::from_str_radix(h, 16).ok()) {
                out.push(char::from(b));
            }
            rest = body.get(2..).unwrap_or("");
        } else if let Some(body) = tail.strip_prefix("\\\\") {
            out.push('\\');
            rest = body;
        } else if tail.starts_with("\\S\\") || tail.starts_with("\\P") {
            rest = tail.get(3..).unwrap_or("");
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
    const KNOWN: [&str; 24] = [
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
