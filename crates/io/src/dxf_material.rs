//! Materials in DXF: the `ACAD_MATERIAL` dictionary of MATERIAL objects (DXF Reference, OBJECTS
//! section), which entities (group 347 among their AcDbEntity groups) and layers (group 347)
//! point at. Every drawing has the materials Global, ByLayer and ByBlock.
//!
//! Written so other programs read what they can (name, description, diffuse colour and texture
//! file with its mapping, glossiness, opacity, refraction index); CADaddy's physically based
//! values and library textures follow as its own extended data and are read back exactly.

use std::collections::HashMap;

use cadaddy_doc::color::Rgb;
use cadaddy_doc::material::{self, Mapping, Material, Projection, Texture};
use cadaddy_dxf::Tag;

use crate::dxf_ext;

/// Most MATERIAL objects read from one file.
const MAX_READ: usize = material::MAX_MATERIALS;

/// The diffuse mapper's transform (16 reals, row major) for a mapping: texture coordinates from
/// surface coordinates (scale to one tile, rotation, offset).
fn matrix(m: &Mapping) -> [f64; 16] {
    let (s, c) = m.rotation.sin_cos();
    let (su, sv) = (1.0 / m.size_u, 1.0 / m.size_v);
    [c * su, s * su, 0.0, -m.offset_u * su, -s * sv, c * sv, 0.0, -m.offset_v * sv, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0]
}

/// A mapping from a diffuse mapper's projection and transform (tile size and rotation from its
/// first two rows).
fn mapping_from(projection: i64, m: &[f64]) -> Mapping {
    let at = |i: usize| m.get(i).copied().filter(|v| v.is_finite()).unwrap_or(0.0);
    let (a, b, e, f) = (at(0), at(1), at(4), at(5));
    let (lu, lv) = ((a * a + b * b).sqrt(), (e * e + f * f).sqrt());
    let mut out = Mapping { projection: Projection::from_dxf(projection), ..Mapping::default() };
    if m.len() >= 8 && lu > 1e-12 && lv > 1e-12 {
        out.size_u = 1.0 / lu;
        out.size_v = 1.0 / lv;
        out.rotation = b.atan2(a);
        out.offset_u = -at(3) * out.size_u;
        out.offset_v = -at(7) * out.size_v;
    }
    out.sane()
}

/// A colour as an AcCmEntityColor value (true colour).
fn entity_color(c: Rgb) -> i64 {
    0xC200_0000_i64 | i64::from(c.0) << 16 | i64::from(c.1) << 8 | i64::from(c.2)
}

fn texture_value(t: &Option<Texture>) -> String {
    match t {
        Some(Texture::File(p)) => format!("file:{p}"),
        Some(Texture::Library(id)) => format!("library:{id}"),
        None => String::new(),
    }
}

fn texture_from(s: &str) -> Option<Texture> {
    let s = s.trim();
    if let Some(p) = s.strip_prefix("file:") {
        Some(Texture::File(p.to_string()))
    } else {
        s.strip_prefix("library:").map(|id| Texture::Library(id.to_string()))
    }
    .filter(|t| !matches!(t, Texture::File(p) | Texture::Library(p) if p.trim().is_empty()))
}

/// The ACAD_MATERIAL dictionary (`dict`, owned by `root`) and its MATERIAL objects: the built-in
/// Global, ByLayer and ByBlock, then the drawing's. `handles` gives each material's handle by
/// upper-case name.
pub(crate) fn objects(materials: &[Material], handles: &HashMap<String, String>, dict: &str, root: &str) -> Vec<Tag> {
    let mut out = vec![
        Tag::s(0, "DICTIONARY"),
        Tag::s(5, dict),
        Tag::s(102, "{ACAD_REACTORS"),
        Tag::s(330, root),
        Tag::s(102, "}"),
        Tag::s(330, root),
        Tag::s(100, "AcDbDictionary"),
        Tag::i(281, 1),
    ];
    let builtin = [material::BY_BLOCK, material::BY_LAYER, material::GLOBAL];
    let names: Vec<&str> = builtin.iter().copied().chain(materials.iter().map(|m| m.name.as_str())).collect();
    for n in &names {
        if let Some(h) = handles.get(&n.to_ascii_uppercase()) {
            out.push(Tag::s(3, dxf_ext::xdata_str(n)));
            out.push(Tag::s(350, h.clone()));
        }
    }
    for n in builtin {
        if let Some(h) = handles.get(&n.to_ascii_uppercase()) {
            out.extend(object(&Material::new(n), h, dict, false));
        }
    }
    for m in materials {
        if let Some(h) = handles.get(&m.name.to_ascii_uppercase()) {
            out.extend(object(m, h, dict, true));
        }
    }
    out
}

/// One MATERIAL object; `own` adds the diffuse values and CADaddy's extended data.
fn object(m: &Material, h: &str, dict: &str, own: bool) -> Vec<Tag> {
    let mut t = vec![
        Tag::s(0, "MATERIAL"),
        Tag::s(5, h),
        Tag::s(102, "{ACAD_REACTORS"),
        Tag::s(330, dict),
        Tag::s(102, "}"),
        Tag::s(330, dict),
        Tag::s(100, "AcDbMaterial"),
        Tag::s(1, dxf_ext::xdata_str(&m.name)),
    ];
    if !own {
        return t;
    }
    if !m.description.is_empty() {
        t.push(Tag::s(2, dxf_ext::xdata_str(&m.description)));
    }
    // Diffuse: the colour (override), then a texture file when it has one, with its mapper.
    t.extend([Tag::i(71, 1), Tag::f(41, 1.0), Tag::i(91, entity_color(m.color)), Tag::f(42, 1.0), Tag::i(72, 1)]);
    if let Some(Texture::File(p)) = &m.color_map {
        t.push(Tag::s(3, p.clone()));
    }
    t.extend([Tag::i(73, m.mapping.projection.dxf()), Tag::i(74, 1), Tag::i(75, 1)]);
    t.extend(matrix(&m.mapping).into_iter().map(|v| Tag::f(43, v)));
    t.push(Tag::f(44, 1.0 - m.roughness));
    t.push(Tag::f(140, m.opacity));
    if let Some(Texture::File(p)) = &m.normal_map {
        t.extend([Tag::f(143, m.normal_strength), Tag::i(179, 1), Tag::s(8, p.clone())]);
    }
    t.push(Tag::f(145, m.refraction));
    // CADaddy's own values, read back exactly.
    let mp = &m.mapping;
    let pairs: [(&str, String); 13] = [
        ("roughness", m.roughness.to_string()),
        ("metallic", m.metallic.to_string()),
        ("opacity", m.opacity.to_string()),
        ("normalStrength", m.normal_strength.to_string()),
        ("colorMap", texture_value(&m.color_map)),
        ("roughnessMap", texture_value(&m.roughness_map)),
        ("normalMap", texture_value(&m.normal_map)),
        ("projection", mp.projection.as_str().to_string()),
        ("sizeU", mp.size_u.to_string()),
        ("sizeV", mp.size_v.to_string()),
        ("offsetU", mp.offset_u.to_string()),
        ("offsetV", mp.offset_v.to_string()),
        ("rotation", mp.rotation.to_string()),
    ];
    t.push(Tag::s(1001, dxf_ext::APP));
    for (k, v) in pairs {
        t.push(Tag::s(1000, k));
        t.push(Tag::s(1000, dxf_ext::xdata_str(&v).to_string()));
    }
    t
}

/// The value after key `k` in CADaddy's key/value extended data.
fn own_value(x: &[Tag], k: &str) -> Option<String> {
    let i = x.iter().position(|t| t.code == 1000 && t.str().trim() == k)?;
    x.get(i + 1).filter(|t| t.code == 1000).map(Tag::str)
}

/// A MATERIAL object's material (`None` for Global, ByLayer and ByBlock).
fn read_object(tags: &[Tag]) -> Option<Material> {
    let name = tags.iter().find(|t| t.code == 1)?.str();
    if name.trim().is_empty() || Material::is_builtin(&name) {
        return None;
    }
    // The groups before the extended data; codes repeat across mappers, so read in order.
    let body: Vec<&Tag> = tags.iter().take_while(|t| t.code < 1000).collect();
    let first = |code: i32| body.iter().find(|t| t.code == code).copied();
    let mut m = Material::new(&name);
    m.description = first(2).map(Tag::str).unwrap_or_default();
    if let Some(c) = first(91).map(Tag::i64) {
        m.color = Rgb((c >> 16 & 0xff) as u8, (c >> 8 & 0xff) as u8, (c & 0xff) as u8);
    }
    m.color_map = first(3).map(Tag::str).filter(|p| !p.trim().is_empty()).map(Texture::File);
    m.normal_map = first(8).map(Tag::str).filter(|p| !p.trim().is_empty()).map(Texture::File);
    if let Some(g) = first(44).map(Tag::f64) {
        m.roughness = 1.0 - g;
    }
    if let Some(o) = first(140).map(Tag::f64) {
        m.opacity = o;
    }
    if let Some(r) = first(145).map(Tag::f64) {
        m.refraction = r;
    }
    // The diffuse mapper: its projection (73) and the first 16 reals of group 43.
    let proj = first(73).map_or(1, Tag::i64);
    let mx: Vec<f64> = body.iter().filter(|t| t.code == 43).take(16).map(|t| t.f64()).collect();
    m.mapping = mapping_from(proj, &mx);
    // CADaddy's own values.
    let x = dxf_ext::xdata(tags, dxf_ext::APP);
    if !x.is_empty() {
        let num = |k: &str| own_value(x, k).and_then(|v| v.trim().parse::<f64>().ok());
        if let Some(v) = num("roughness") {
            m.roughness = v;
        }
        if let Some(v) = num("metallic") {
            m.metallic = v;
        }
        if let Some(v) = num("opacity") {
            m.opacity = v;
        }
        if let Some(v) = num("normalStrength") {
            m.normal_strength = v;
        }
        m.color_map = own_value(x, "colorMap").map_or(m.color_map, |v| texture_from(&v));
        m.roughness_map = own_value(x, "roughnessMap").and_then(|v| texture_from(&v));
        m.normal_map = own_value(x, "normalMap").map_or(m.normal_map, |v| texture_from(&v));
        let mp = &mut m.mapping;
        if let Some(p) = own_value(x, "projection").and_then(|v| Projection::parse(&v)) {
            mp.projection = p;
        }
        for (k, field) in [
            ("sizeU", &mut mp.size_u),
            ("sizeV", &mut mp.size_v),
            ("offsetU", &mut mp.offset_u),
            ("offsetV", &mut mp.offset_v),
            ("rotation", &mut mp.rotation),
        ] {
            if let Some(v) = num(k) {
                *field = v;
            }
        }
    }
    Some(m.sane())
}

/// The materials of a file's OBJECTS section records, and every MATERIAL's name by upper-case
/// handle (Global, ByLayer and ByBlock included), for the 347 groups that point at them.
pub(crate) fn read(objects: &[(String, Vec<Tag>)]) -> (Vec<Material>, HashMap<String, String>) {
    let mut out: Vec<Material> = Vec::new();
    let mut names = HashMap::new();
    for (kind, tags) in objects.iter().filter(|(k, _)| k == "MATERIAL").take(MAX_READ) {
        let _ = kind;
        let Some(name) = tags.iter().find(|t| t.code == 1).map(Tag::str) else { continue };
        if let Some(h) = tags.iter().find(|t| t.code == 5).map(|t| t.str().trim().to_ascii_uppercase()) {
            names.insert(h, name.trim().to_string());
        }
        if let Some(m) = read_object(tags)
            && !out.iter().any(|o| o.name.eq_ignore_ascii_case(&m.name))
        {
            out.push(m);
        }
    }
    (out, names)
}

/// What a 347 group's material name means for an entity: `None` for ByLayer (the default),
/// else the name (ByBlock, Global or a material).
pub(crate) fn entity_material(name: &str) -> Option<String> {
    (!name.eq_ignore_ascii_case(material::BY_LAYER)).then(|| name.to_string())
}

/// What a 347 group's material name means for a layer: `None` for Global (the default).
pub(crate) fn layer_material(name: &str) -> Option<String> {
    (!name.eq_ignore_ascii_case(material::GLOBAL)).then(|| name.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mapper_matrix_roundtrips() {
        let m = Mapping { projection: Projection::Box, size_u: 450.0, size_v: 300.0, offset_u: 12.5, offset_v: -3.0, rotation: 0.3 };
        let back = mapping_from(m.projection.dxf(), &matrix(&m));
        for (a, b) in
            [(back.size_u, m.size_u), (back.size_v, m.size_v), (back.rotation, m.rotation), (back.offset_u, m.offset_u), (back.offset_v, m.offset_v)]
        {
            assert!((a - b).abs() < 1e-9, "{back:?}");
        }
        assert_eq!(back.projection, Projection::Box);
        // Garbage matrices give the default mapping.
        assert_eq!(mapping_from(9, &[f64::NAN; 16]), Mapping::default());
        assert_eq!(mapping_from(1, &[1.0]), Mapping::default());
    }
}
