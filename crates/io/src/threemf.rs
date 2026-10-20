//! 3MF out (the 3D Manufacturing Format, a public specification): the drawing's solids and meshes
//! as one welded triangle mesh, coloured per triangle through base materials, in an OPC zip
//! package (`[Content_Types].xml`, `_rels/.rels`, `3D/3dmodel.model`) written by our own code.

use std::collections::HashMap;
use std::fmt::Write as _;

use cadaddy_doc::Drawing;

use crate::{IoError, Result, mesh3d_io};

/// The model part's unit for the drawing's INSUNITS (3MF knows micron to metre, inch and foot).
fn unit(d: &Drawing) -> (&'static str, f64) {
    match d.header.i64("INSUNITS", 0) {
        1 => ("inch", 1.0),
        2 => ("foot", 1.0),
        4 => ("millimeter", 1.0),
        5 => ("centimeter", 1.0),
        6 => ("meter", 1.0),
        13 => ("micron", 1.0),
        // Anything else: millimetres, scaled from the unit's metres.
        u if u != 0 => ("millimeter", cadaddy_doc::geo::metres_per_unit(u) * 1000.0),
        _ if d.header.i64("MEASUREMENT", 1) == 1 => ("millimeter", 1.0),
        _ => ("inch", 1.0),
    }
}

fn num(v: f64) -> String {
    let s = format!("{v:.6}");
    let s = s.trim_end_matches('0').trim_end_matches('.');
    if s.is_empty() || s == "-0" { "0".into() } else { s.to_string() }
}

/// The 3D model part's XML.
fn model_xml(d: &Drawing) -> Result<String> {
    let tris = mesh3d_io::scene_tris(d);
    if tris.is_empty() {
        return Err(IoError::Format("nothing to write to 3MF: no solids, meshes or 3D faces".into()));
    }
    let (unit, k) = unit(d);
    let mut colours: Vec<[u8; 3]> = Vec::new();
    let mut colour_index: HashMap<[u8; 3], usize> = HashMap::new();
    let mut verts: Vec<[f64; 3]> = Vec::new();
    let mut vert_index: HashMap<[u64; 3], usize> = HashMap::new();
    let mut faces: Vec<([usize; 3], usize)> = Vec::with_capacity(tris.len());
    for (p, rgb) in &tris {
        let mut idx = [0usize; 3];
        for (slot, v) in idx.iter_mut().zip(p) {
            let q = [v.x * k, v.y * k, v.z * k];
            if !q.iter().all(|c| c.is_finite()) {
                continue;
            }
            let key = q.map(f64::to_bits);
            *slot = *vert_index.entry(key).or_insert_with(|| {
                verts.push(q);
                verts.len() - 1
            });
        }
        // A degenerate triangle (two corners the same) is no part of a 3MF mesh.
        if idx[0] == idx[1] || idx[1] == idx[2] || idx[0] == idx[2] {
            continue;
        }
        let c = *colour_index.entry(*rgb).or_insert_with(|| {
            colours.push(*rgb);
            colours.len() - 1
        });
        faces.push((idx, c));
    }
    if faces.is_empty() {
        return Err(IoError::Format("nothing to write to 3MF: every triangle is degenerate".into()));
    }
    let mut x = String::with_capacity(verts.len() * 60 + faces.len() * 70 + 1024);
    x.push_str("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n");
    let _ = writeln!(x, "<model unit=\"{unit}\" xml:lang=\"en-US\" xmlns=\"http://schemas.microsoft.com/3dmanufacturing/core/2015/02\">");
    x.push_str(" <metadata name=\"Application\">CADaddy</metadata>\n <resources>\n  <basematerials id=\"1\">\n");
    for c in &colours {
        let _ = writeln!(x, "   <base name=\"Colour {0:02X}{1:02X}{2:02X}\" displaycolor=\"#{0:02X}{1:02X}{2:02X}\"/>", c[0], c[1], c[2]);
    }
    x.push_str("  </basematerials>\n  <object id=\"2\" type=\"model\" pid=\"1\" pindex=\"0\">\n   <mesh>\n    <vertices>\n");
    for v in &verts {
        let _ = writeln!(x, "     <vertex x=\"{}\" y=\"{}\" z=\"{}\"/>", num(v[0]), num(v[1]), num(v[2]));
    }
    x.push_str("    </vertices>\n    <triangles>\n");
    for (t, c) in &faces {
        let _ = writeln!(x, "     <triangle v1=\"{}\" v2=\"{}\" v3=\"{}\" pid=\"1\" p1=\"{c}\"/>", t[0], t[1], t[2]);
    }
    x.push_str("    </triangles>\n   </mesh>\n  </object>\n </resources>\n <build>\n  <item objectid=\"2\"/>\n </build>\n</model>\n");
    Ok(x)
}

const CONTENT_TYPES: &str = "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<Types xmlns=\"http://schemas.openxmlformats.org/package/2006/content-types\"><Default Extension=\"rels\" ContentType=\"application/vnd.openxmlformats-package.relationships+xml\"/><Default Extension=\"model\" ContentType=\"application/vnd.ms-package.3dmanufacturing-3dmodel+xml\"/></Types>\n";
const RELS: &str = "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\"><Relationship Target=\"/3D/3dmodel.model\" Id=\"rel0\" Type=\"http://schemas.microsoft.com/3dmanufacturing/2013/01/3dmodel\"/></Relationships>\n";

/// A zip archive of `entries` (name, data, deflate?): the minimum 3MF readers need.
fn zip(entries: &[(&str, &[u8], bool)]) -> Result<Vec<u8>> {
    let too_big = || IoError::Format("the 3MF package would be larger than 4 GiB".into());
    let mut out = Vec::new();
    let mut central = Vec::new();
    for (name, data, deflate) in entries {
        let packed = if *deflate { miniz_oxide::deflate::compress_to_vec(data, 6) } else { data.to_vec() };
        let method: u16 = if *deflate { 8 } else { 0 };
        let crc = crc32fast::hash(data);
        let offset = u32::try_from(out.len()).map_err(|_| too_big())?;
        let (size, csize) = (u32::try_from(data.len()).map_err(|_| too_big())?, u32::try_from(packed.len()).map_err(|_| too_big())?);
        let nlen = u16::try_from(name.len()).map_err(|_| too_big())?;
        let header = |sig: u32, central: bool| {
            let mut h = Vec::with_capacity(46 + name.len());
            h.extend(sig.to_le_bytes());
            if central {
                h.extend(20u16.to_le_bytes());
            }
            for v in [20u16, 0, method, 0, 0x21] {
                h.extend(v.to_le_bytes());
            }
            h.extend(crc.to_le_bytes());
            h.extend(csize.to_le_bytes());
            h.extend(size.to_le_bytes());
            h.extend(nlen.to_le_bytes());
            h.extend(0u16.to_le_bytes());
            if central {
                for v in [0u16, 0, 0] {
                    h.extend(v.to_le_bytes());
                }
                h.extend(0u32.to_le_bytes());
                h.extend(offset.to_le_bytes());
            }
            h.extend(name.as_bytes());
            h
        };
        out.extend(header(0x0403_4b50, false));
        out.extend(&packed);
        central.extend(header(0x0201_4b50, true));
    }
    let cd_at = u32::try_from(out.len()).map_err(|_| too_big())?;
    let cd_len = u32::try_from(central.len()).map_err(|_| too_big())?;
    out.extend(central);
    let n = u16::try_from(entries.len()).map_err(|_| too_big())?;
    out.extend(0x0605_4b50u32.to_le_bytes());
    for v in [0u16, 0, n, n] {
        out.extend(v.to_le_bytes());
    }
    out.extend(cd_len.to_le_bytes());
    out.extend(cd_at.to_le_bytes());
    out.extend(0u16.to_le_bytes());
    Ok(out)
}

/// The drawing's 3D objects as a 3MF package.
pub fn write_3mf(d: &Drawing) -> Result<Vec<u8>> {
    let model = model_xml(d)?;
    zip(&[
        ("[Content_Types].xml", CONTENT_TYPES.as_bytes(), false),
        ("_rels/.rels", RELS.as_bytes(), false),
        ("3D/3dmodel.model", model.as_bytes(), true),
    ])
}

#[cfg(test)]
mod tests {
    use super::*;
    use cadaddy_doc::{Common, EntityKind, Mesh, MeshFace, Space};
    use cadaddy_geom::Vec3;

    fn cube() -> Drawing {
        let mut d = Drawing::new_metric();
        let v = [
            (0.0, 0.0, 0.0),
            (10.0, 0.0, 0.0),
            (10.0, 10.0, 0.0),
            (0.0, 10.0, 0.0),
            (0.0, 0.0, 10.0),
            (10.0, 0.0, 10.0),
            (10.0, 10.0, 10.0),
            (0.0, 10.0, 10.0),
        ];
        let faces = [[0, 3, 2, 1], [4, 5, 6, 7], [0, 1, 5, 4], [1, 2, 6, 5], [2, 3, 7, 6], [3, 0, 4, 7]];
        let m = Mesh {
            vertices: v.iter().map(|(x, y, z)| Vec3::new(*x, *y, *z)).collect(),
            faces: faces.iter().map(|f| MeshFace { v: f.to_vec(), hidden: 0 }).collect(),
            ..Mesh::default()
        };
        d.add(&Space::Model, Common::default(), EntityKind::Mesh(m)).unwrap();
        d
    }

    #[test]
    fn a_cube_is_a_welded_mesh_in_a_valid_package() {
        let bytes = write_3mf(&cube()).unwrap();
        assert!(bytes.starts_with(b"PK\x03\x04"));
        // The package's parts, found through the central directory's names.
        for part in ["[Content_Types].xml", "_rels/.rels", "3D/3dmodel.model"] {
            assert!(bytes.windows(part.len()).any(|w| w == part.as_bytes()), "{part}");
        }
        let xml = model_xml(&cube()).unwrap();
        assert_eq!(xml.matches("<vertex ").count(), 8, "welded");
        assert_eq!(xml.matches("<triangle ").count(), 12);
        assert!(xml.contains("unit=\"millimeter\""));
        assert!(xml.contains("displaycolor=\"#"));
    }

    #[test]
    fn nothing_3d_is_an_error() {
        assert!(write_3mf(&Drawing::new_metric()).is_err());
    }
}
