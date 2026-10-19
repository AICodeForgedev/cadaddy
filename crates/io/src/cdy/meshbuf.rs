//! Model space objects with their mesh geometry as binary buffers (ENTS schema v2).
//!
//! A walkthrough model is mostly meshes (walls, floors, furniture: thousands of vertices each).
//! In the v1 encoding every coordinate is a token of a value tree, read back one by one; here a
//! mesh of [`BINARY_MIN`] vertices or more keeps only its record in the tokens (vertices and faces
//! left empty), and its geometry follows them as flat little-endian arrays, read back with a copy.
//!
//! The payload: the v1 token list, then one buffer per binary mesh —
//! `item u32, vertices u32, faces u32, indices u32, hidden u32`, the vertices as `x y z` f64s,
//! each face's vertex count as u32, the indices as u32, then `(face u32, mask u64)` for each face
//! with hidden edges — then `meshes u32` and `tokens length u64`. The tokens come first so a
//! damaged chunk still salvages the objects before the damage.
//!
//! A chunk with no mesh that large is written as v1, so files without meshes stay readable by
//! CADaddy versions that don't know v2.

use std::collections::HashMap;
use std::sync::Arc;

use cadaddy_doc::geom::Vec3;
use cadaddy_doc::{Entity, EntityKind, Mesh, MeshFace};
use serde_json::Value;

use super::tokens;

/// Meshes with at least this many vertices have binary buffers.
pub const BINARY_MIN: usize = 16;
/// Most vertices, faces or indices one buffer declares (each also checked against the bytes
/// that are there before anything is allocated).
const MAX_COUNT: usize = 1 << 28;

/// One chunk's objects: `(schema version, payload)`, v2 when a mesh has binary buffers.
pub fn encode(items: &[Arc<Entity>]) -> Result<(u16, Vec<u8>), String> {
    let mut list = Vec::with_capacity(items.len());
    let mut meshes: Vec<(u32, &Mesh)> = Vec::new();
    for (i, e) in items.iter().enumerate() {
        match &e.kind {
            EntityKind::Mesh(m) if m.vertices.len() >= BINARY_MIN => {
                let stripped = Entity {
                    handle: e.handle,
                    common: e.common.clone(),
                    kind: EntityKind::Mesh(Mesh { vertices: Vec::new(), faces: Vec::new(), kind: m.kind }),
                };
                list.push(serde_json::to_value(&stripped).map_err(|e| e.to_string())?);
                meshes.push((u32::try_from(i).map_err(|_| "too many objects in a chunk")?, m));
            }
            _ => list.push(serde_json::to_value(e.as_ref()).map_err(|e| e.to_string())?),
        }
    }
    let toks = tokens::encode(&Value::Array(list));
    if meshes.is_empty() {
        return Ok((1, toks));
    }
    let count = |n: usize| u32::try_from(n).map_err(|_| "a mesh is too large for a .cdy file".to_string());
    let mut out = toks;
    let toks_len = out.len() as u64;
    for (item, m) in &meshes {
        let indices: usize = m.faces.iter().map(|f| f.v.len()).sum();
        let hidden: Vec<(u32, u64)> = m.faces.iter().enumerate().filter(|(_, f)| f.hidden != 0).map(|(i, f)| (i as u32, f.hidden)).collect();
        for n in [*item, count(m.vertices.len())?, count(m.faces.len())?, count(indices)?, count(hidden.len())?] {
            out.extend_from_slice(&n.to_le_bytes());
        }
        out.reserve(m.vertices.len() * 24 + m.faces.len() * 4 + indices * 4 + hidden.len() * 12);
        for v in &m.vertices {
            for c in [v.x, v.y, v.z] {
                out.extend_from_slice(&c.to_le_bytes());
            }
        }
        for f in &m.faces {
            out.extend_from_slice(&count(f.v.len())?.to_le_bytes());
        }
        for f in &m.faces {
            for i in &f.v {
                out.extend_from_slice(&i.to_le_bytes());
            }
        }
        for (f, mask) in hidden {
            out.extend_from_slice(&f.to_le_bytes());
            out.extend_from_slice(&mask.to_le_bytes());
        }
    }
    out.extend_from_slice(&count(meshes.len())?.to_le_bytes());
    out.extend_from_slice(&toks_len.to_le_bytes());
    Ok((2, out))
}

/// Reads little-endian numbers off the front of a byte slice.
struct Cursor<'a>(&'a [u8]);

impl<'a> Cursor<'a> {
    fn take(&mut self, n: usize) -> Option<&'a [u8]> {
        let (head, rest) = self.0.split_at_checked(n)?;
        self.0 = rest;
        Some(head)
    }
    fn u32(&mut self) -> Option<u32> {
        self.take(4)?.try_into().ok().map(u32::from_le_bytes)
    }
    fn count(&mut self) -> Option<usize> {
        usize::try_from(self.u32()?).ok().filter(|n| *n <= MAX_COUNT)
    }
    /// `n` items of `size` bytes, when that many are there.
    fn array(&mut self, n: usize, size: usize) -> Option<&'a [u8]> {
        self.take(n.checked_mul(size)?)
    }
}

fn u32s(b: &[u8]) -> impl Iterator<Item = u32> + '_ {
    b.as_chunks::<4>().0.iter().map(|c| u32::from_le_bytes(*c))
}

fn f64_at(c: &[u8]) -> f64 {
    let mut a = [0u8; 8];
    a.copy_from_slice(c);
    f64::from_le_bytes(a)
}

/// A v2 payload split into its token list and its meshes by item; `None` when it's damaged.
pub fn decode(data: &[u8]) -> Option<(Value, HashMap<usize, Mesh>)> {
    let tail_at = data.len().checked_sub(12)?;
    let tail = data.get(tail_at..)?;
    let n = u32::from_le_bytes(tail.get(..4)?.try_into().ok()?);
    let toks_len = usize::try_from(u64::from_le_bytes(tail.get(4..)?.try_into().ok()?)).ok()?;
    let toks = data.get(..toks_len)?;
    let mut c = Cursor(data.get(toks_len..tail_at)?);
    let mut meshes = HashMap::new();
    for _ in 0..n {
        let item = c.count()?;
        let (nv, nf, ni, nh) = (c.count()?, c.count()?, c.count()?, c.count()?);
        let verts = c.array(nv, 24)?;
        let sizes = c.array(nf, 4)?;
        let idx = c.array(ni, 4)?;
        let hidden = c.array(nh, 12)?;
        let vertices = verts.as_chunks::<24>().0.iter().map(|v| Vec3::new(f64_at(&v[0..8]), f64_at(&v[8..16]), f64_at(&v[16..24]))).collect();
        let mut flat = u32s(idx);
        let mut faces = Vec::with_capacity(nf);
        let mut left = ni;
        for k in u32s(sizes) {
            let k = usize::try_from(k).ok()?;
            left = left.checked_sub(k)?;
            faces.push(MeshFace { v: flat.by_ref().take(k).collect(), hidden: 0 });
        }
        if left != 0 {
            return None;
        }
        for h in hidden.as_chunks::<12>().0 {
            let f = usize::try_from(u32::from_le_bytes([h[0], h[1], h[2], h[3]])).ok()?;
            faces.get_mut(f)?.hidden = u64::from_le_bytes(h.get(4..12)?.try_into().ok()?);
        }
        meshes.insert(item, Mesh { vertices, faces, kind: Default::default() });
    }
    if !c.0.is_empty() {
        return None;
    }
    Some((tokens::decode(toks)?, meshes))
}

/// Put a binary mesh's geometry back into its object (read with its record's `kind`).
pub fn fill(e: &mut Entity, mut m: Mesh) {
    if let EntityKind::Mesh(rec) = &mut e.kind {
        m.kind = rec.kind;
        *rec = m;
    }
}
