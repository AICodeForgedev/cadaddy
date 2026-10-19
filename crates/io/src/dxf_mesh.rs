//! Meshes in DXF (DXF Reference, MESH and POLYLINE): the MESH entity (AcDbSubDMesh, read at its
//! base level) and the POLYLINE polyface (flag 64) and M×N polygon (flag 16) meshes, all read
//! into [`Mesh`]; hostile counts are capped.

use cadaddy_doc::{MAX_FACE_VERTICES, MAX_MESH_FACES, MAX_MESH_VERTICES, Mesh, MeshFace, MeshKind};
use cadaddy_dxf::Tag;
use cadaddy_geom::Vec3;

/// A MESH entity's vertices and faces; `None` when it has no usable face.
pub(crate) fn read_subd(tags: &[Tag]) -> Option<Mesh> {
    let start = tags.iter().position(|t| t.code == 100 && t.str() == "AcDbSubDMesh").unwrap_or(0);
    let tags = tags.get(start..)?;
    let mut i = 0;
    let int = |t: &Tag| t.i64();
    // 92: the vertex count, then that many 10/20/30 points.
    let mut vertices = Vec::new();
    while let Some(t) = tags.get(i) {
        i += 1;
        if t.code == 92 {
            let n = usize::try_from(int(t)).unwrap_or(0).min(MAX_MESH_VERTICES);
            while vertices.len() < n {
                let (Some(x), Some(y), Some(z)) = (tags.get(i), tags.get(i + 1), tags.get(i + 2)) else { break };
                if x.code != 10 || y.code != 20 || z.code != 30 {
                    break;
                }
                vertices.push(Vec3::new(x.f64(), y.f64(), z.f64()));
                i += 3;
            }
            break;
        }
    }
    // 93: the face list's size, then that many 90s: each face's vertex count and its indices.
    let mut faces = Vec::new();
    while let Some(t) = tags.get(i) {
        i += 1;
        if t.code == 93 {
            let size = usize::try_from(int(t)).unwrap_or(0);
            let end = i.saturating_add(size).min(tags.len());
            while i < end && faces.len() < MAX_MESH_FACES {
                let Some(n) = tags.get(i).filter(|t| t.code == 90).map(|t| usize::try_from(int(t)).unwrap_or(0)) else { break };
                i += 1;
                if n > MAX_FACE_VERTICES || i + n > end {
                    break;
                }
                let v: Vec<u32> = tags
                    .get(i..i + n)
                    .unwrap_or(&[])
                    .iter()
                    .filter_map(|t| u32::try_from(int(t)).ok())
                    .filter(|k| (*k as usize) < vertices.len())
                    .collect();
                i += n;
                if v.len() == n && distinct(&v) >= 3 {
                    faces.push(MeshFace { v, hidden: 0 });
                }
            }
            break;
        }
    }
    (!faces.is_empty()).then_some(Mesh { vertices, faces, kind: MeshKind::Subd })
}

/// How many different vertices a face names (a face of fewer than three is no surface).
fn distinct(v: &[u32]) -> usize {
    let mut s: Vec<u32> = v.to_vec();
    s.sort_unstable();
    s.dedup();
    s.len()
}

/// A polyface mesh: each face record names three or four vertices by 1-based index; a negative
/// index hides the edge that starts at that vertex. Faces naming a vertex that isn't there are
/// left out.
pub(crate) fn polyface(vertices: Vec<Vec3>, records: &[[i64; 4]]) -> Option<Mesh> {
    let mut faces = Vec::new();
    for r in records.iter().take(MAX_MESH_FACES) {
        let mut v = Vec::with_capacity(4);
        let mut hidden = 0u64;
        let mut ok = true;
        for &ix in r.iter().filter(|ix| **ix != 0) {
            match usize::try_from(ix.unsigned_abs())
                .ok()
                .and_then(|n| n.checked_sub(1))
                .filter(|n| *n < vertices.len())
                .and_then(|n| u32::try_from(n).ok())
            {
                Some(k) => {
                    if ix < 0 {
                        hidden |= 1 << v.len();
                    }
                    v.push(k);
                }
                None => ok = false,
            }
        }
        if ok && distinct(&v) >= 3 {
            faces.push(MeshFace { v, hidden });
        }
    }
    (!faces.is_empty()).then_some(Mesh { vertices, faces, kind: MeshKind::Polyface })
}

/// An M×N polygon mesh (71/72, or the smoothed 73/74 when the vertex count matches those),
/// closed in M by bit 1 and in N by bit 32; vertices run row by row. Its faces are the quads
/// between rows and columns.
pub(crate) fn polygon(mut vertices: Vec<Vec3>, dims: [Option<i64>; 4], flags: i64) -> Option<Mesh> {
    let pair = |a: Option<i64>, b: Option<i64>| Some((usize::try_from(a?).ok()?, usize::try_from(b?).ok()?));
    let fits = |(m, n): (usize, usize)| m >= 2 && n >= 2 && m.checked_mul(n).is_some_and(|mn| mn <= vertices.len());
    let exact = |(m, n): (usize, usize)| fits((m, n)) && m * n == vertices.len();
    let [a, b, c, d] = dims;
    let (m, n) = [pair(a, b), pair(c, d)].into_iter().flatten().find(|x| exact(*x)).or_else(|| pair(a, b).filter(|x| fits(*x)))?;
    vertices.truncate(m * n);
    let (closed_m, closed_n) = (flags & 1 != 0, flags & 32 != 0);
    let rows = if closed_m { m } else { m - 1 };
    let cols = if closed_n { n } else { n - 1 };
    let at = |i: usize, j: usize| u32::try_from((i % m) * n + j % n).ok();
    let mut faces = Vec::new();
    for i in 0..rows {
        for j in 0..cols {
            if faces.len() >= MAX_MESH_FACES {
                break;
            }
            if let (Some(p), Some(q), Some(r), Some(s)) = (at(i, j), at(i, j + 1), at(i + 1, j + 1), at(i + 1, j)) {
                faces.push(MeshFace { v: vec![p, q, r, s], hidden: 0 });
            }
        }
    }
    let kind = MeshKind::Polygon { m: u32::try_from(m).ok()?, n: u32::try_from(n).ok()?, closed_m, closed_n };
    (!faces.is_empty()).then_some(Mesh { vertices, faces, kind })
}
