//! 3D exchange: glTF 2.0 (`.glb` binary and `.gltf` with embedded data), Wavefront OBJ with its
//! MTL materials, and binary STL out; OBJ, STL (binary or ASCII) and glTF in, as MESH objects
//! with materials.
//!
//! What goes out is what the shaded views draw (`render::shade::scene`): every surface as
//! triangles, with its material (base colour, roughness, metalness, opacity, base colour texture)
//! and texture coordinates. glTF is Y-up in metres: the drawing's Z becomes Y and its units become
//! metres (INSUNITS). A block's surfaces are written once and each reference to it is a node
//! placing them (instancing). OBJ is written Y-up in drawing units; STL Z-up in drawing units.

use cadaddy_color::Rgb;
use cadaddy_doc::material::{Material, Texture, metres_per_unit};
use cadaddy_doc::{Common, Drawing, EntityKind, EntityStore, Mesh, MeshFace, MeshKind, Space};
use cadaddy_geom::Vec3;
use cadaddy_render::shade::{NO_SURFACE, Scene, Tri, scene};
use serde_json::{Value, json};

use crate::{IoError, Result};

/// Most triangles one file brings in, and nodes a glTF scene walks.
const MAX_TRIS: usize = 5_000_000;
const MAX_NODES: usize = 100_000;
const MAX_DEPTH: usize = 64;

// ---------------- base64 (data URIs) ----------------

const B64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

fn b64_encode(data: &[u8]) -> String {
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for c in data.chunks(3) {
        let n = (u32::from(c[0]) << 16) | (u32::from(*c.get(1).unwrap_or(&0)) << 8) | u32::from(*c.get(2).unwrap_or(&0));
        for (i, shift) in [18, 12, 6, 0].into_iter().enumerate() {
            if i <= c.len() {
                out.push(char::from(B64[((n >> shift) & 63) as usize]));
            } else {
                out.push('=');
            }
        }
    }
    out
}

fn b64_decode(s: &str) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(s.len() / 4 * 3);
    let (mut acc, mut bits) = (0u32, 0u32);
    for ch in s.bytes() {
        let v = match ch {
            b'A'..=b'Z' => ch - b'A',
            b'a'..=b'z' => ch - b'a' + 26,
            b'0'..=b'9' => ch - b'0' + 52,
            b'+' | b'-' => 62,
            b'/' | b'_' => 63,
            b'=' | b'\n' | b'\r' | b' ' => continue,
            _ => return None,
        };
        acc = (acc << 6) | u32::from(v);
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
        }
    }
    Some(out)
}

// ---------------- what goes out ----------------

/// A material as files carry it.
#[derive(Clone, Debug, PartialEq)]
struct OutMat {
    name: String,
    /// Linear base colour and opacity.
    base: [f32; 4],
    roughness: f32,
    metallic: f32,
    /// Index into the scene's textures.
    texture: Option<u32>,
}

/// Triangles grouped by material: (material index, positions, uvs).
type Groups = Vec<(usize, Vec<[Vec3; 3]>, Vec<[[f32; 2]; 3]>)>;

/// The materials and per-material triangles of a scene, materials shared through `mats`.
fn group(sc: &Scene, mats: &mut Vec<OutMat>, offset: Vec3) -> Groups {
    let lin = |v: u8| (f32::from(v) / 255.0).powf(2.2);
    let mut groups: Groups = Vec::new();
    for t in &sc.tris {
        let m = match sc.surfaces.get(t.mat as usize).filter(|_| t.mat != NO_SURFACE) {
            Some(s) => OutMat {
                name: s.name.clone(),
                base: [s.base[0], s.base[1], s.base[2], s.opacity],
                roughness: s.roughness,
                metallic: s.metallic,
                texture: s.texture,
            },
            None => OutMat {
                name: format!("Color {:02X}{:02X}{:02X}", t.color.0, t.color.1, t.color.2),
                base: [lin(t.color.0), lin(t.color.1), lin(t.color.2), 1.0],
                roughness: 0.6,
                metallic: 0.0,
                texture: None,
            },
        };
        let mi = match mats.iter().position(|x| x.name == m.name) {
            Some(i) => i,
            None => {
                mats.push(m);
                mats.len() - 1
            }
        };
        let gi = match groups.iter().position(|g| g.0 == mi) {
            Some(i) => i,
            None => {
                groups.push((mi, Vec::new(), Vec::new()));
                groups.len() - 1
            }
        };
        if let Some(g) = groups.get_mut(gi) {
            g.1.push(t.p.map(|p| p - offset));
            g.2.push(t.uv);
        }
    }
    groups
}

/// A drawing whose model space is `entities` (the rest of `d` as it is: layers, materials).
fn with_model(d: &Drawing, entities: impl Iterator<Item = cadaddy_doc::Entity>) -> Drawing {
    let mut t = d.clone();
    let mut store = EntityStore::new();
    for e in entities {
        store.push(e);
    }
    t.model = store;
    t
}

/// The scene of a drawing (curves to a thousandth of its size).
fn scene_of(d: &Drawing) -> Scene {
    let ext = d.extents(&Space::Model);
    let size = ext.width().max(ext.height());
    let tol = if size.is_finite() && size > 0.0 { size / 1000.0 } else { 0.01 };
    scene(d, &Space::Model, tol, Rgb(255, 255, 255))
}

/// Model space's surfaces outside block references, and each referenced block's own surfaces
/// with the placements of its references.
struct Parts {
    main: Scene,
    blocks: Vec<(String, Scene, Vec<[f64; 16]>)>,
}

/// A reference's placement (column-major 4 × 4): the block's base to the insertion point, scaled,
/// turned about Z.
fn insert_matrix(base: Vec3, at: Vec3, scale: Vec3, rot: f64) -> [f64; 16] {
    let (s, c) = rot.sin_cos();
    // M = T(at) · Rz · S · T(−base)
    let (sx, sy, sz) = (scale.x, scale.y, scale.z);
    let t = Vec3::new(-base.x * sx, -base.y * sy, -base.z * sz);
    let tx = c * t.x - s * t.y + at.x;
    let ty = s * t.x + c * t.y + at.y;
    let tz = t.z + at.z;
    [c * sx, s * sx, 0.0, 0.0, -s * sy, c * sy, 0.0, 0.0, 0.0, 0.0, sz, 0.0, tx, ty, tz, 1.0]
}

fn parts(d: &Drawing) -> Parts {
    let mut blocks: Vec<(String, Scene, Vec<[f64; 16]>)> = Vec::new();
    let mut plain = Vec::new();
    for e in d.model.iter() {
        if !d.is_visible(e) {
            continue;
        }
        if let EntityKind::Insert(i) = &e.kind
            && let Some(b) = d.block(&i.block)
            && b.entities.iter().next().is_some()
            && b.xref_path.is_none()
        {
            let at = i.insert;
            let mut placements = Vec::new();
            let (cols, rows) = (i.cols.clamp(1, 1000), i.rows.clamp(1, 1000));
            for r in 0..rows {
                for cc in 0..cols {
                    let (s, co) = i.rotation.sin_cos();
                    let (dx, dy) = (f64::from(cc) * i.col_spacing, f64::from(r) * i.row_spacing);
                    let off = Vec3::new(co * dx - s * dy, s * dx + co * dy, 0.0);
                    placements.push(insert_matrix(b.base, at + off, i.scale, i.rotation));
                }
            }
            match blocks.iter_mut().find(|(n, _, _)| n.eq_ignore_ascii_case(&i.block)) {
                Some(entry) => entry.2.extend(placements),
                None => {
                    let sc = scene_of(&with_model(d, b.entities.iter().map(|x| (**x).clone())));
                    blocks.push((i.block.clone(), sc, placements));
                }
            }
            continue;
        }
        plain.push((**e).clone());
    }
    Parts { main: scene_of(&with_model(d, plain.into_iter())), blocks }
}

/// One glTF document's binary buffer and JSON, built up.
struct Gltf {
    bin: Vec<u8>,
    views: Vec<Value>,
    accessors: Vec<Value>,
}

impl Gltf {
    fn align(&mut self) {
        while !self.bin.len().is_multiple_of(4) {
            self.bin.push(0);
        }
    }
    fn view(&mut self, bytes: &[u8], target: Option<u32>) -> usize {
        self.align();
        let start = self.bin.len();
        self.bin.extend_from_slice(bytes);
        let mut v = json!({ "buffer": 0, "byteOffset": start, "byteLength": bytes.len() });
        if let (Some(t), Some(o)) = (target, v.as_object_mut()) {
            o.insert("target".into(), json!(t));
        }
        self.views.push(v);
        self.views.len() - 1
    }
    fn floats(&mut self, data: &[f32], kind: &str, n: usize, minmax: bool) -> usize {
        let bytes: Vec<u8> = data.iter().flat_map(|f| f.to_le_bytes()).collect();
        let view = self.view(&bytes, Some(34962));
        let count = data.len() / n.max(1);
        let mut a = json!({ "bufferView": view, "componentType": 5126, "count": count, "type": kind });
        if minmax && count > 0 {
            let mut lo = vec![f32::MAX; n];
            let mut hi = vec![f32::MIN; n];
            for c in data.chunks(n) {
                for k in 0..n {
                    lo[k] = lo[k].min(c[k]);
                    hi[k] = hi[k].max(c[k]);
                }
            }
            if let Some(o) = a.as_object_mut() {
                o.insert("min".into(), json!(lo));
                o.insert("max".into(), json!(hi));
            }
        }
        self.accessors.push(a);
        self.accessors.len() - 1
    }
    fn indices(&mut self, count: usize) -> usize {
        let bytes: Vec<u8> = (0..count as u32).flat_map(|i| i.to_le_bytes()).collect();
        let view = self.view(&bytes, Some(34963));
        self.accessors.push(json!({ "bufferView": view, "componentType": 5125, "count": count, "type": "SCALAR" }));
        self.accessors.len() - 1
    }
}

/// A mesh (glTF primitives per material) of `groups`, positions converted by `to`.
fn gltf_mesh(g: &mut Gltf, groups: &Groups, to: &dyn Fn(Vec3) -> [f32; 3], name: &str) -> Value {
    let mut prims = Vec::new();
    for (mi, tris, uvs) in groups {
        let mut pos = Vec::with_capacity(tris.len() * 9);
        let mut nrm = Vec::with_capacity(tris.len() * 9);
        let mut tex = Vec::with_capacity(tris.len() * 6);
        for (t, uv) in tris.iter().zip(uvs) {
            let p = t.map(to);
            let a = Vec3::new(f64::from(p[0][0]), f64::from(p[0][1]), f64::from(p[0][2]));
            let b = Vec3::new(f64::from(p[1][0]), f64::from(p[1][1]), f64::from(p[1][2]));
            let c = Vec3::new(f64::from(p[2][0]), f64::from(p[2][1]), f64::from(p[2][2]));
            let n = (b - a).cross(c - a).normalized();
            let n = if n.len() > 0.5 { n } else { Vec3::new(0.0, 1.0, 0.0) };
            for k in 0..3 {
                pos.extend(p[k]);
                nrm.extend([n.x as f32, n.y as f32, n.z as f32]);
                // glTF's texture origin is the image's top left.
                tex.extend([uv[k][0], 1.0 - uv[k][1]]);
            }
        }
        let count = tris.len() * 3;
        let pa = g.floats(&pos, "VEC3", 3, true);
        let na = g.floats(&nrm, "VEC3", 3, false);
        let ta = g.floats(&tex, "VEC2", 2, false);
        let ia = g.indices(count);
        prims.push(json!({ "attributes": { "POSITION": pa, "NORMAL": na, "TEXCOORD_0": ta }, "indices": ia, "material": mi, "mode": 4 }));
    }
    json!({ "name": name, "primitives": prims })
}

/// The drawing as glTF: (JSON, binary buffer).
fn gltf(d: &Drawing) -> (Value, Vec<u8>) {
    let parts = parts(d);
    let unit = metres_per_unit(d.header.i64("INSUNITS", 0), d.header.i64("MEASUREMENT", 1) == 1) as f32;
    let mut mats: Vec<OutMat> = Vec::new();
    let mut g = Gltf { bin: Vec::new(), views: Vec::new(), accessors: Vec::new() };
    let mut meshes = Vec::new();
    let mut nodes: Vec<Value> = Vec::new();
    let mut children: Vec<usize> = Vec::new();
    // Positions about the model's middle stay precise in f32.
    let centre = {
        let e = d.extents(&Space::Model);
        if e.is_empty() { Vec3::ZERO } else { Vec3::new(e.center().x, e.center().y, 0.0) }
    };
    let local = |p: Vec3| [p.x as f32, p.y as f32, p.z as f32];
    let main = group(&parts.main, &mut mats, centre);
    if !main.is_empty() {
        meshes.push(gltf_mesh(&mut g, &main, &local, "Model"));
        nodes.push(json!({ "name": "Model", "mesh": meshes.len() - 1 }));
        children.push(nodes.len() - 1);
    }
    for (name, sc, placements) in &parts.blocks {
        let groups = group(sc, &mut mats, Vec3::ZERO);
        if groups.is_empty() {
            continue;
        }
        meshes.push(gltf_mesh(&mut g, &groups, &local, name));
        let mesh = meshes.len() - 1;
        for m in placements {
            let mut m = *m;
            m[12] -= centre.x;
            m[13] -= centre.y;
            m[14] -= centre.z;
            nodes.push(json!({ "name": name, "mesh": mesh, "matrix": m }));
            children.push(nodes.len() - 1);
        }
    }
    // Textures: the scene's images, as PNG in the buffer. Each block's scene numbers its own
    // textures, so a material's texture is looked up in the scene it came from.
    let mut images: Vec<Value> = Vec::new();
    let mut textures: Vec<Value> = Vec::new();
    let mut tex_of: Vec<(String, usize)> = Vec::new();
    let scenes: Vec<&Scene> = std::iter::once(&parts.main).chain(parts.blocks.iter().map(|b| &b.1)).collect();
    let mut materials = Vec::new();
    for m in &mats {
        let mut pbr = json!({ "baseColorFactor": m.base, "metallicFactor": m.metallic, "roughnessFactor": m.roughness });
        if let Some(t) = m.texture {
            let key = m.name.clone();
            let ti = match tex_of.iter().find(|(k, _)| *k == key) {
                Some((_, i)) => Some(*i),
                None => scenes
                    .iter()
                    .find_map(|sc| {
                        let owns = sc.surfaces.iter().any(|s| s.name == m.name && s.texture == Some(t));
                        owns.then(|| sc.textures.get(t as usize)).flatten()
                    })
                    .and_then(|r| r.encode_png())
                    .map(|png| {
                        let view = g.view(&png, None);
                        images.push(json!({ "bufferView": view, "mimeType": "image/png" }));
                        textures.push(json!({ "source": images.len() - 1, "sampler": 0 }));
                        tex_of.push((key, textures.len() - 1));
                        textures.len() - 1
                    }),
            };
            if let (Some(ti), Some(o)) = (ti, pbr.as_object_mut()) {
                o.insert("baseColorTexture".into(), json!({ "index": ti }));
            }
        }
        let mut mat = json!({ "name": m.name, "pbrMetallicRoughness": pbr, "doubleSided": true });
        if m.base[3] < 0.999
            && let Some(o) = mat.as_object_mut()
        {
            o.insert("alphaMode".into(), json!("BLEND"));
        }
        materials.push(mat);
    }
    // The root turns Z up into Y up and drawing units into metres.
    // (x, y, z) → (x, z, −y), after putting the model's middle back.
    let (cx, cy, cz) = (centre.x as f32 * unit, centre.y as f32 * unit, centre.z as f32 * unit);
    let root = json!({ "name": "Drawing", "matrix": [unit, 0.0, 0.0, 0.0, 0.0, 0.0, -unit, 0.0, 0.0, unit, 0.0, 0.0, cx, cz, -cy, 1.0], "children": children });
    nodes.push(root);
    let root_i = nodes.len() - 1;
    g.align();
    let mut doc = json!({
        "asset": { "version": "2.0", "generator": "CADaddy" },
        "scene": 0,
        "scenes": [{ "nodes": [root_i] }],
        "nodes": nodes,
        "meshes": meshes,
        "materials": materials,
        "accessors": g.accessors,
        "bufferViews": g.views,
        "buffers": [{ "byteLength": g.bin.len() }],
    });
    if !images.is_empty()
        && let Some(o) = doc.as_object_mut()
    {
        o.insert("images".into(), json!(images));
        o.insert("textures".into(), json!(textures));
        o.insert("samplers".into(), json!([{ "magFilter": 9729, "minFilter": 9987, "wrapS": 10497, "wrapT": 10497 }]));
    }
    (doc, g.bin)
}

/// The drawing as a binary glTF (.glb).
pub fn write_glb(d: &Drawing) -> Vec<u8> {
    let (doc, bin) = gltf(d);
    let mut json = serde_json::to_vec(&doc).unwrap_or_default();
    while !json.len().is_multiple_of(4) {
        json.push(b' ');
    }
    let total = 12 + 8 + json.len() + if bin.is_empty() { 0 } else { 8 + bin.len() };
    let mut out = Vec::with_capacity(total);
    out.extend_from_slice(b"glTF");
    out.extend(2u32.to_le_bytes());
    out.extend((total as u32).to_le_bytes());
    out.extend((json.len() as u32).to_le_bytes());
    out.extend(b"JSON");
    out.extend(json);
    if !bin.is_empty() {
        out.extend((bin.len() as u32).to_le_bytes());
        out.extend(b"BIN\0");
        out.extend(bin);
    }
    out
}

/// The drawing as a glTF JSON file with its buffer embedded (a data URI).
pub fn write_gltf(d: &Drawing) -> Vec<u8> {
    let (mut doc, bin) = gltf(d);
    if let Some(b) = doc.get_mut("buffers").and_then(|b| b.get_mut(0)).and_then(Value::as_object_mut) {
        b.insert("uri".into(), json!(format!("data:application/octet-stream;base64,{}", b64_encode(&bin))));
    }
    serde_json::to_vec_pretty(&doc).unwrap_or_default()
}

/// Model space as one scene: the block references' surfaces placed among the rest (their
/// materials and textures joined to the main scene's).
fn world_scene(d: &Drawing) -> Scene {
    let parts = parts(d);
    let mut out = parts.main;
    for (_, sc, placements) in parts.blocks {
        let (s0, t0) = (out.surfaces.len() as u32, out.textures.len() as u32);
        out.textures.extend(sc.textures.iter().cloned());
        out.surfaces.extend(sc.surfaces.iter().cloned().map(|mut m| {
            m.texture = m.texture.map(|t| t + t0);
            m
        }));
        for m in &placements {
            let at = |p: Vec3| {
                let q = [0, 1, 2].map(|r| m[r] * p.x + m[4 + r] * p.y + m[8 + r] * p.z + m[12 + r]);
                Vec3::new(q[0], q[1], q[2])
            };
            for t in &sc.tris {
                if out.tris.len() >= MAX_TRIS {
                    break;
                }
                out.tris.push(Tri { p: t.p.map(at), mat: if t.mat == NO_SURFACE { NO_SURFACE } else { t.mat + s0 }, ..*t });
            }
        }
    }
    out
}

/// Every triangle of the drawing's model space, block references placed.
fn all_tris(d: &Drawing) -> Vec<Tri> {
    world_scene(d).tris
}

/// OBJ output: the OBJ text, its MTL text, and the textures the MTL names (file name, PNG).
pub struct ObjOut {
    pub obj: String,
    pub mtl: String,
    pub textures: Vec<(String, Vec<u8>)>,
}

/// The drawing as OBJ (Y up, drawing units) with materials in `mtl_name`.
pub fn write_obj(d: &Drawing, mtl_name: &str) -> ObjOut {
    use std::fmt::Write;
    let sc = world_scene(d);
    let mut mats = Vec::new();
    let groups = group(&sc, &mut mats, Vec3::ZERO);
    let mut obj = format!("# CADaddy\nmtllib {mtl_name}\n");
    let mut mtl = String::from("# CADaddy\n");
    let mut textures = Vec::new();
    let stem = mtl_name.trim_end_matches(".mtl");
    for (i, m) in mats.iter().enumerate() {
        let name = obj_name(&m.name, i);
        let _ = write!(
            mtl,
            "newmtl {name}\nKd {:.6} {:.6} {:.6}\nd {:.4}\nNs {:.1}\nPr {:.4}\nPm {:.4}\n",
            m.base[0],
            m.base[1],
            m.base[2],
            m.base[3],
            (1.0 - m.roughness).powi(2) * 1000.0,
            m.roughness,
            m.metallic
        );
        if let Some(png) = m.texture.and_then(|t| sc.textures.get(t as usize)).and_then(|r| r.encode_png()) {
            let file = format!("{stem}-{i}.png");
            let _ = writeln!(mtl, "map_Kd {file}");
            textures.push((file, png));
        }
        mtl.push('\n');
    }
    let (mut v, mut vt, mut vn) = (1usize, 1usize, 1usize);
    for (mi, tris, uvs) in &groups {
        let _ = writeln!(obj, "usemtl {}", mats.get(*mi).map(|m| obj_name(&m.name, *mi)).unwrap_or_default());
        for (t, uv) in tris.iter().zip(uvs) {
            let n = (t[1] - t[0]).cross(t[2] - t[0]).normalized();
            for (p, q) in t.iter().zip(uv) {
                // Y up: drawing (x, y, z) is (x, z, −y).
                let _ = writeln!(obj, "v {} {} {}\nvt {} {}", p.x, p.z, -p.y, q[0], q[1]);
            }
            let _ = writeln!(obj, "vn {} {} {}", n.x, n.z, -n.y);
            let _ = writeln!(obj, "f {}/{}/{} {}/{}/{} {}/{}/{}", v, vt, vn, v + 1, vt + 1, vn, v + 2, vt + 2, vn);
            v += 3;
            vt += 3;
            vn += 1;
        }
    }
    ObjOut { obj, mtl, textures }
}

/// OBJ with its MTL and textures, as one bundle (what `write(d, "name.obj.bundle")` returns):
/// for each file a little-endian u32 name length, the name (UTF-8), a u32 data length, the data.
/// The OBJ comes first, named `name.obj`.
pub fn obj_bundle(d: &Drawing, stem: &str) -> Vec<u8> {
    let out = write_obj(d, &format!("{stem}.mtl"));
    let mut b = Vec::new();
    let mut put = |name: &str, data: &[u8]| {
        b.extend((name.len() as u32).to_le_bytes());
        b.extend(name.as_bytes());
        b.extend((data.len() as u32).to_le_bytes());
        b.extend(data);
    };
    put(&format!("{stem}.obj"), out.obj.as_bytes());
    put(&format!("{stem}.mtl"), out.mtl.as_bytes());
    for (n, png) in &out.textures {
        put(n, png);
    }
    b
}

/// A material name as OBJ allows it (no spaces).
fn obj_name(s: &str, i: usize) -> String {
    let n: String = s.chars().map(|c| if c.is_whitespace() { '_' } else { c }).collect();
    if n.is_empty() { format!("material{i}") } else { n }
}

/// The drawing's surfaces as binary STL (Z up, drawing units).
pub fn write_stl(d: &Drawing) -> Vec<u8> {
    let tris = all_tris(d);
    let mut out = Vec::with_capacity(84 + tris.len() * 50);
    let mut header = [b' '; 80];
    header[..11].copy_from_slice(b"CADaddy STL");
    out.extend_from_slice(&header);
    out.extend((tris.len() as u32).to_le_bytes());
    for t in &tris {
        let n = (t.p[1] - t.p[0]).cross(t.p[2] - t.p[0]).normalized();
        for v in std::iter::once(n).chain(t.p) {
            out.extend((v.x as f32).to_le_bytes());
            out.extend((v.y as f32).to_le_bytes());
            out.extend((v.z as f32).to_le_bytes());
        }
        out.extend(0u16.to_le_bytes());
    }
    out
}

// ---------------- what comes in ----------------

/// Triangles into one mesh, corners that coincide shared.
fn mesh_of(tris: &[[Vec3; 3]]) -> Option<Mesh> {
    use std::collections::HashMap;
    let size = tris.iter().flatten().fold(0.0f64, |m, p| m.max(p.x.abs()).max(p.y.abs()).max(p.z.abs())).max(1e-9);
    let q = size * 1e-9;
    let mut ids: HashMap<[i64; 3], u32> = HashMap::new();
    let mut vertices = Vec::new();
    let mut faces = Vec::new();
    for t in tris.iter().take(cadaddy_doc::MAX_MESH_FACES) {
        let mut v = [0u32; 3];
        for (k, p) in t.iter().enumerate() {
            if !p.is_finite() {
                return None;
            }
            let key = [(p.x / q).round() as i64, (p.y / q).round() as i64, (p.z / q).round() as i64];
            v[k] = *ids.entry(key).or_insert_with(|| {
                vertices.push(*p);
                (vertices.len() - 1) as u32
            });
        }
        if v[0] != v[1] && v[1] != v[2] && v[0] != v[2] {
            faces.push(MeshFace { v: v.to_vec(), hidden: 0 });
        }
        if vertices.len() >= cadaddy_doc::MAX_MESH_VERTICES {
            break;
        }
    }
    (!faces.is_empty()).then_some(Mesh { vertices, faces, kind: MeshKind::Subd })
}

/// A drawing of meshes, each with its material (added to the drawing).
fn drawing_of(meshes: Vec<(Vec<[Vec3; 3]>, Option<Material>)>) -> Result<Drawing> {
    let mut d = Drawing::new_metric();
    let mut n = 0usize;
    for (tris, mat) in meshes {
        let Some(m) = mesh_of(&tris) else { continue };
        let mut c = Common::default();
        if let Some(mat) = mat {
            let mat = mat.sane();
            if !d.materials.iter().any(|x| x.name == mat.name) && !Material::is_builtin(&mat.name) {
                d.materials.push(mat.clone());
            }
            c.material = Some(mat.name);
        }
        d.add(&Space::Model, c, EntityKind::Mesh(m)).map_err(|e| IoError::Format(e.to_string()))?;
        n += 1;
    }
    if n == 0 {
        return Err(IoError::Format("no surfaces in the file".into()));
    }
    Ok(d)
}

/// A file next to `name` (its folder), when files can be read.
fn sibling(name: &str, rel: &str) -> Option<Vec<u8>> {
    #[cfg(not(target_arch = "wasm32"))]
    {
        if rel.contains("..") || rel.starts_with('/') || rel.contains(':') {
            return None;
        }
        let dir = std::path::Path::new(name).parent()?;
        let p = dir.join(rel);
        let meta = std::fs::metadata(&p).ok()?;
        if meta.len() > 256 << 20 {
            return None;
        }
        std::fs::read(p).ok()
    }
    #[cfg(target_arch = "wasm32")]
    {
        let _ = (name, rel);
        None
    }
}

fn sibling_path(name: &str, rel: &str) -> String {
    std::path::Path::new(name).parent().map(|d| d.join(rel).to_string_lossy().into_owned()).unwrap_or_else(|| rel.to_string())
}

fn unlin(v: f64) -> u8 {
    (v.clamp(0.0, 1.0).powf(1.0 / 2.2) * 255.0).round() as u8
}

/// STL, binary or ASCII: one mesh.
pub fn read_stl(bytes: &[u8]) -> Result<Drawing> {
    let ascii = bytes.len() >= 6 && bytes[..5].eq_ignore_ascii_case(b"solid") && std::str::from_utf8(bytes).is_ok_and(|t| t.contains("facet"));
    let mut tris = Vec::new();
    if ascii {
        let text = std::str::from_utf8(bytes).map_err(|_| IoError::Format("not UTF-8".into()))?;
        let mut cur: Vec<Vec3> = Vec::new();
        for line in text.lines() {
            let mut w = line.split_whitespace();
            if w.next() == Some("vertex") {
                let f: Vec<f64> = w.take(3).filter_map(|x| x.parse().ok()).collect();
                if let [x, y, z] = f[..] {
                    cur.push(Vec3::new(x, y, z));
                }
                if cur.len() == 3 {
                    tris.push([cur[0], cur[1], cur[2]]);
                    cur.clear();
                    if tris.len() >= MAX_TRIS {
                        break;
                    }
                }
            } else if line.trim_start().starts_with("endloop") {
                cur.clear();
            }
        }
    } else {
        let count = bytes
            .get(80..84)
            .map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]) as usize)
            .ok_or_else(|| IoError::Format("too short for STL".into()))?;
        let have = bytes.len().saturating_sub(84) / 50;
        for i in 0..count.min(have).min(MAX_TRIS) {
            let at = 84 + i * 50 + 12;
            let f = |k: usize| bytes.get(at + k * 4..at + k * 4 + 4).map_or(0.0, |b| f64::from(f32::from_le_bytes([b[0], b[1], b[2], b[3]])));
            tris.push([Vec3::new(f(0), f(1), f(2)), Vec3::new(f(3), f(4), f(5)), Vec3::new(f(6), f(7), f(8))]);
        }
    }
    drawing_of(vec![(tris, None)])
}

/// OBJ with its MTL (read next to the file): a mesh per object and material.
pub fn read_obj(bytes: &[u8], name: &str) -> Result<Drawing> {
    let text = String::from_utf8_lossy(bytes);
    let mut v: Vec<Vec3> = Vec::new();
    let mut mats: Vec<Material> = Vec::new();
    // (object, material) → triangles.
    let mut groups: Vec<((String, String), Vec<[Vec3; 3]>)> = Vec::new();
    let (mut object, mut material) = (String::new(), String::new());
    let mut total = 0usize;
    for line in text.lines() {
        let line = line.split('#').next().unwrap_or("").trim();
        let mut w = line.split_whitespace();
        match w.next() {
            Some("v") => {
                let f: Vec<f64> = w.take(3).filter_map(|x| x.parse().ok()).collect();
                if let [x, y, z] = f[..] {
                    // Y up in the file: drawing (x, −z, y).
                    v.push(Vec3::new(x, -z, y));
                }
            }
            Some("o") | Some("g") => object = w.collect::<Vec<_>>().join(" "),
            Some("usemtl") => material = w.collect::<Vec<_>>().join(" "),
            Some("mtllib") => {
                for lib in w {
                    if let Some(m) = sibling(name, lib) {
                        mats.extend(parse_mtl(&String::from_utf8_lossy(&m), name));
                    }
                }
            }
            Some("f") => {
                let idx: Vec<usize> = w
                    .filter_map(|c| {
                        let i: i64 = c.split('/').next()?.parse().ok()?;
                        let n = v.len() as i64;
                        let i = if i < 0 { n + i } else { i - 1 };
                        (0..n).contains(&i).then_some(i as usize)
                    })
                    .collect();
                if idx.len() < 3 {
                    continue;
                }
                let key = (object.clone(), material.clone());
                let gi = match groups.iter().position(|g| g.0 == key) {
                    Some(i) => i,
                    None => {
                        groups.push((key, Vec::new()));
                        groups.len() - 1
                    }
                };
                for k in 1..idx.len() - 1 {
                    if let (Some(a), Some(b), Some(c), Some(g)) = (v.get(idx[0]), v.get(idx[k]), v.get(idx[k + 1]), groups.get_mut(gi)) {
                        g.1.push([*a, *b, *c]);
                        total += 1;
                    }
                }
                if total >= MAX_TRIS {
                    break;
                }
            }
            _ => {}
        }
    }
    drawing_of(groups.into_iter().map(|((_, m), t)| (t, mats.iter().find(|x| x.name == m).cloned())).collect())
}

/// Materials of an MTL file (PBR extension fields Pr, Pm when given).
fn parse_mtl(text: &str, name: &str) -> Vec<Material> {
    let mut out: Vec<Material> = Vec::new();
    for line in text.lines() {
        let line = line.split('#').next().unwrap_or("").trim();
        let mut w = line.split_whitespace();
        let key = w.next();
        let rest: Vec<&str> = w.collect();
        let nums: Vec<f64> = rest.iter().filter_map(|x| x.parse().ok()).collect();
        if key == Some("newmtl") {
            out.push(Material::new(&rest.join(" ")));
            continue;
        }
        let Some(m) = out.last_mut() else { continue };
        match key {
            Some("Kd") if nums.len() >= 3 => m.color = Rgb(unlin(nums[0]), unlin(nums[1]), unlin(nums[2])),
            Some("d") if !nums.is_empty() => m.opacity = nums[0],
            Some("Tr") if !nums.is_empty() => m.opacity = 1.0 - nums[0],
            Some("Ns") if !nums.is_empty() => m.roughness = 1.0 - (nums[0].max(0.0) / 1000.0).sqrt().min(1.0),
            Some("Pr") if !nums.is_empty() => m.roughness = nums[0],
            Some("Pm") if !nums.is_empty() => m.metallic = nums[0],
            Some("map_Kd") => {
                if let Some(file) = rest.last() {
                    m.color_map = Some(Texture::File(sibling_path(name, file)));
                }
            }
            _ => {}
        }
    }
    out
}

/// glTF (.gltf JSON or .glb binary): every triangle primitive of the default scene, placed by its
/// nodes, as meshes with their materials.
pub fn read_gltf(bytes: &[u8], name: &str) -> Result<Drawing> {
    let bad = |m: &str| IoError::Format(format!("glTF: {m}"));
    let (doc, bin): (Value, Option<&[u8]>) = if bytes.starts_with(b"glTF") {
        let u32_at = |o: usize| bytes.get(o..o + 4).map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]) as usize);
        let jlen = u32_at(12).ok_or_else(|| bad("truncated"))?;
        let json = bytes.get(20..20 + jlen).ok_or_else(|| bad("truncated JSON chunk"))?;
        let doc: Value = serde_json::from_slice(json).map_err(|e| bad(&e.to_string()))?;
        let at = 20 + jlen;
        let bin = u32_at(at).and_then(|l| bytes.get(at + 8..at + 8 + l));
        (doc, bin)
    } else {
        (serde_json::from_slice(bytes).map_err(|e| bad(&e.to_string()))?, None)
    };
    // The buffers' bytes.
    let buffers: Vec<Vec<u8>> = doc["buffers"]
        .as_array()
        .map(|a| {
            a.iter()
                .enumerate()
                .map(|(i, b)| match b["uri"].as_str() {
                    Some(u) if u.starts_with("data:") => u.split_once(',').and_then(|(_, d)| b64_decode(d)).unwrap_or_default(),
                    Some(u) => sibling(name, u).unwrap_or_default(),
                    None if i == 0 => bin.map(<[u8]>::to_vec).unwrap_or_default(),
                    None => Vec::new(),
                })
                .collect()
        })
        .unwrap_or_default();
    let views = doc["bufferViews"].as_array().cloned().unwrap_or_default();
    let accessors = doc["accessors"].as_array().cloned().unwrap_or_default();
    let usize_of = |v: &Value| v.as_u64().map(|n| n as usize);
    // An accessor's values as f64 tuples of `n` (positions: 3) or indices (1).
    let read = |ai: usize, want: usize| -> Option<Vec<f64>> {
        let a = accessors.get(ai)?;
        let count = usize_of(&a["count"])?.min(MAX_TRIS * 3);
        let comps = match a["type"].as_str()? {
            "SCALAR" => 1,
            "VEC2" => 2,
            "VEC3" => 3,
            "VEC4" => 4,
            _ => return None,
        };
        if comps < want {
            return None;
        }
        let ct = a["componentType"].as_u64()?;
        let size = match ct {
            5126 | 5125 => 4,
            5123 | 5122 => 2,
            5121 | 5120 => 1,
            _ => return None,
        };
        let view = views.get(usize_of(&a["bufferView"])?)?;
        let buf = buffers.get(usize_of(&view["buffer"]).unwrap_or(0))?;
        let start = usize_of(&view["byteOffset"]).unwrap_or(0).checked_add(usize_of(&a["byteOffset"]).unwrap_or(0))?;
        let stride = usize_of(&view["byteStride"]).filter(|s| *s >= comps * size).unwrap_or(comps * size);
        let mut out = Vec::with_capacity(count * want);
        for i in 0..count {
            let base = start.checked_add(i.checked_mul(stride)?)?;
            for k in 0..want {
                let o = base + k * size;
                let b = buf.get(o..o + size)?;
                out.push(match ct {
                    5126 => f64::from(f32::from_le_bytes([b[0], b[1], b[2], b[3]])),
                    5125 => f64::from(u32::from_le_bytes([b[0], b[1], b[2], b[3]])),
                    5123 => f64::from(u16::from_le_bytes([b[0], b[1]])),
                    5122 => f64::from(i16::from_le_bytes([b[0], b[1]])),
                    5121 => f64::from(b[0]),
                    _ => f64::from(b[0] as i8),
                });
            }
        }
        Some(out)
    };
    // Materials.
    let mats: Vec<Material> = doc["materials"]
        .as_array()
        .map(|a| {
            a.iter()
                .enumerate()
                .map(|(i, m)| {
                    let pbr = &m["pbrMetallicRoughness"];
                    let mut mat = Material::new(
                        m["name"].as_str().filter(|n| !n.trim().is_empty()).map_or_else(|| format!("glTF material {i}"), str::to_string).as_str(),
                    );
                    if let Some(c) = pbr["baseColorFactor"].as_array() {
                        let f = |k: usize| c.get(k).and_then(Value::as_f64).unwrap_or(1.0);
                        mat.color = Rgb(unlin(f(0)), unlin(f(1)), unlin(f(2)));
                        mat.opacity = f(3);
                    }
                    mat.metallic = pbr["metallicFactor"].as_f64().unwrap_or(1.0);
                    mat.roughness = pbr["roughnessFactor"].as_f64().unwrap_or(1.0);
                    // A texture in its own file (embedded images stay behind).
                    if let Some(ti) = pbr["baseColorTexture"]["index"].as_u64()
                        && let Some(src) = doc["textures"][ti as usize]["source"].as_u64()
                        && let Some(uri) = doc["images"][src as usize]["uri"].as_str()
                        && !uri.starts_with("data:")
                    {
                        mat.color_map = Some(Texture::File(sibling_path(name, uri)));
                    }
                    mat
                })
                .collect()
        })
        .unwrap_or_default();
    // glTF is Y up in metres: back to Z up in millimetres (the new drawing is metric).
    let to_mm = 1000.0;
    let mut meshes: Vec<(Vec<[Vec3; 3]>, Option<Material>)> = Vec::new();
    let mut total = 0usize;
    let roots: Vec<usize> =
        doc["scenes"][usize_of(&doc["scene"]).unwrap_or(0)]["nodes"].as_array().map(|a| a.iter().filter_map(usize_of).collect()).unwrap_or_default();
    let mut stack: Vec<(usize, [f64; 16], usize)> = roots.into_iter().map(|n| (n, IDENTITY, 0)).collect();
    let mut visited = 0usize;
    while let Some((ni, parent, depth)) = stack.pop() {
        visited += 1;
        if visited > MAX_NODES || depth > MAX_DEPTH {
            break;
        }
        let node = &doc["nodes"][ni];
        let m = mat_mul(&parent, &node_matrix(node));
        if let Some(children) = node["children"].as_array() {
            for c in children.iter().filter_map(usize_of) {
                stack.push((c, m, depth + 1));
            }
        }
        let Some(mi) = usize_of(&node["mesh"]) else { continue };
        for prim in doc["meshes"][mi]["primitives"].as_array().map(Vec::as_slice).unwrap_or(&[]) {
            if prim["mode"].as_u64().unwrap_or(4) != 4 {
                continue;
            }
            let Some(pos) = usize_of(&prim["attributes"]["POSITION"]).and_then(|a| read(a, 3)) else { continue };
            let pts: Vec<Vec3> = pos
                .chunks(3)
                .map(|c| {
                    let p = transform(&m, [c[0], c[1], c[2]]);
                    Vec3::new(p[0] * to_mm, -p[2] * to_mm, p[1] * to_mm)
                })
                .collect();
            let idx: Vec<usize> = match usize_of(&prim["indices"]).and_then(|a| read(a, 1)) {
                Some(i) => i.into_iter().map(|x| x as usize).collect(),
                None => (0..pts.len()).collect(),
            };
            let tris: Vec<[Vec3; 3]> =
                idx.chunks(3).filter_map(|c| Some([*pts.get(*c.first()?)?, *pts.get(*c.get(1)?)?, *pts.get(*c.get(2)?)?])).collect();
            total += tris.len();
            meshes.push((tris, usize_of(&prim["material"]).and_then(|k| mats.get(k)).cloned()));
            if total >= MAX_TRIS {
                break;
            }
        }
    }
    let mut d = drawing_of(meshes)?;
    d.header.set_i64("INSUNITS", 4);
    Ok(d)
}

const IDENTITY: [f64; 16] = [1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0];

/// A node's local matrix (column-major): `matrix`, or translation · rotation · scale.
fn node_matrix(n: &Value) -> [f64; 16] {
    if let Some(m) = n["matrix"].as_array().filter(|m| m.len() == 16) {
        let mut out = IDENTITY;
        for (k, v) in m.iter().enumerate() {
            out[k] = v.as_f64().filter(|x| x.is_finite()).unwrap_or(out[k]);
        }
        return out;
    }
    let arr = |key: &str, d: &[f64]| -> Vec<f64> {
        n[key]
            .as_array()
            .map(|a| a.iter().map(|v| v.as_f64().filter(|x| x.is_finite()).unwrap_or(0.0)).collect())
            .filter(|v: &Vec<f64>| v.len() == d.len())
            .unwrap_or_else(|| d.to_vec())
    };
    let t = arr("translation", &[0.0, 0.0, 0.0]);
    let r = arr("rotation", &[0.0, 0.0, 0.0, 1.0]);
    let s = arr("scale", &[1.0, 1.0, 1.0]);
    let (x, y, z, w) = (r[0], r[1], r[2], r[3]);
    let rot = [
        1.0 - 2.0 * (y * y + z * z),
        2.0 * (x * y + z * w),
        2.0 * (x * z - y * w),
        2.0 * (x * y - z * w),
        1.0 - 2.0 * (x * x + z * z),
        2.0 * (y * z + x * w),
        2.0 * (x * z + y * w),
        2.0 * (y * z - x * w),
        1.0 - 2.0 * (x * x + y * y),
    ];
    [
        rot[0] * s[0],
        rot[1] * s[0],
        rot[2] * s[0],
        0.0,
        rot[3] * s[1],
        rot[4] * s[1],
        rot[5] * s[1],
        0.0,
        rot[6] * s[2],
        rot[7] * s[2],
        rot[8] * s[2],
        0.0,
        t[0],
        t[1],
        t[2],
        1.0,
    ]
}

fn mat_mul(a: &[f64; 16], b: &[f64; 16]) -> [f64; 16] {
    let mut out = [0.0; 16];
    for c in 0..4 {
        for r in 0..4 {
            out[c * 4 + r] = (0..4).map(|k| a[k * 4 + r] * b[c * 4 + k]).sum();
        }
    }
    out
}

fn transform(m: &[f64; 16], p: [f64; 3]) -> [f64; 3] {
    [0, 1, 2].map(|r| m[r] * p[0] + m[4 + r] * p[1] + m[8 + r] * p[2] + m[12 + r])
}
