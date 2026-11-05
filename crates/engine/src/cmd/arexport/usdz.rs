//! The AR scene as a USDZ package: Pixar's "Usdz File Format Specification" (openusd.org) — a
//! zip archive with no compression and no encryption whose every file's data starts at a
//! multiple of 64 bytes from the start of the archive (padding in the local headers' extra
//! field), the first file being the root layer. The root layer is USD text (`.usda`, the USD
//! "Text File Format"): stage metadata `upAxis = "Y"`, `metersPerUnit = 1`, `defaultPrim`; one
//! `UsdPreviewSurface` material per colour (UsdPreviewSurface Specification); each repeated part
//! once as a prototype under an abstract (`class`) prim, and each element an `instanceable`
//! Xform referencing it, with its metadata in `customData`. The zip structures follow PKWARE's
//! APPNOTE.TXT (6.3): local file headers, the central directory and its end record; CRC-32 is
//! the zip polynomial 0xEDB88320.

use std::fmt::Write as _;

use serde_json::Value;

use super::Scene;

/// The root layer's name inside the package.
pub const ROOT: &str = "model.usda";
const ALIGN: usize = 64;
/// The extra-field id the alignment padding is written under (any id a reader doesn't know is
/// skipped, APPNOTE 4.6.1).
const PAD_ID: u16 = 0x1986;
const ROOT_PRIM: &str = "CADaddy_AR";

/// A USD identifier (prim and dictionary key names): letters, digits and `_`, not starting with
/// a digit.
fn ident(s: &str) -> String {
    let mut out: String = s.chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '_' }).take(80).collect();
    if out.is_empty() || out.starts_with(|c: char| c.is_ascii_digit()) {
        out.insert(0, '_');
    }
    out
}

/// A quoted USD string.
fn quote(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars().take(2_000) {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c.is_control() => {}
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

fn num(v: f64) -> String {
    if !v.is_finite() {
        return "0".into();
    }
    let r = (v * 1e6).round() / 1e6;
    let s = format!("{r}");
    if s == "-0" { "0".into() } else { s }
}

fn num32(v: f32) -> String {
    num(f64::from(v))
}

/// A JSON object as the entries of a USD dictionary (`string k = "v"`, `double k = 1`,
/// `bool k = 1`, `dictionary k = {…}`, `string[] k = […]`), `depth` levels in.
fn dict_body(out: &mut String, m: &serde_json::Map<String, Value>, indent: usize, depth: usize) {
    let pad = " ".repeat(indent);
    let mut seen = std::collections::BTreeSet::new();
    for (k, v) in m.iter().take(200) {
        let key = ident(k);
        if !seen.insert(key.clone()) {
            continue;
        }
        let line = match v {
            Value::Null => continue,
            Value::Bool(b) => format!("bool {key} = {}", u8::from(*b)),
            Value::Number(n) => match n.as_i64() {
                Some(i) => format!("int64 {key} = {i}"),
                None => format!("double {key} = {}", num(n.as_f64().unwrap_or(0.0))),
            },
            Value::String(s) => format!("string {key} = {}", quote(s)),
            Value::Object(o) if depth < 4 => {
                let _ = writeln!(out, "{pad}dictionary {key} = {{");
                dict_body(out, o, indent + 4, depth + 1);
                let _ = writeln!(out, "{pad}}}");
                continue;
            }
            Value::Array(a) if !a.is_empty() && a.iter().all(Value::is_string) => {
                format!("string[] {key} = [{}]", a.iter().take(200).filter_map(Value::as_str).map(quote).collect::<Vec<_>>().join(", "))
            }
            Value::Array(a) if !a.is_empty() && a.iter().all(Value::is_number) => {
                format!("double[] {key} = [{}]", a.iter().take(200).map(|x| num(x.as_f64().unwrap_or(0.0))).collect::<Vec<_>>().join(", "))
            }
            other => format!("string {key} = {}", quote(&other.to_string())),
        };
        let _ = writeln!(out, "{pad}{line}");
    }
}

/// The root layer's text.
pub(crate) fn layer(scene: &Scene) -> String {
    let mut out = String::with_capacity(4096 + scene.geometries.iter().map(|g| g.positions.len() * 64 + g.indices.len() * 8).sum::<usize>());
    out.push_str("#usda 1.0\n(\n    customLayerData = {\n");
    if let Some(info) = scene.info.as_object() {
        out.push_str("        dictionary cadaddy = {\n");
        dict_body(&mut out, info, 12, 1);
        out.push_str("        }\n");
    }
    out.push_str("    }\n");
    let _ = writeln!(out, "    defaultPrim = \"{ROOT_PRIM}\"");
    out.push_str("    doc = \"CADaddy bim.export.ar\"\n    metersPerUnit = 1\n    upAxis = \"Y\"\n)\n\n");
    let _ = writeln!(out, "def Xform \"{ROOT_PRIM}\" (\n    kind = \"component\"\n)\n{{");
    // Materials.
    out.push_str("    def Scope \"Materials\"\n    {\n");
    for (i, m) in scene.materials.iter().enumerate() {
        let _ = writeln!(out, "        def Material \"m{i}\" (\n            doc = {}\n        )\n        {{", quote(&m.name));
        let _ = writeln!(out, "            token outputs:surface.connect = </{ROOT_PRIM}/Materials/m{i}/Surface.outputs:surface>");
        out.push_str("\n            def Shader \"Surface\"\n            {\n                uniform token info:id = \"UsdPreviewSurface\"\n");
        let _ = writeln!(out, "                color3f inputs:diffuseColor = ({}, {}, {})", num32(m.rgb[0]), num32(m.rgb[1]), num32(m.rgb[2]));
        out.push_str("                float inputs:metallic = 0\n");
        let _ = writeln!(out, "                float inputs:opacity = {}", num32(m.opacity));
        out.push_str("                float inputs:roughness = 0.8\n                token outputs:surface\n            }\n        }\n");
    }
    out.push_str("    }\n\n");
    // The parts, once each.
    out.push_str("    class Scope \"Prototypes\"\n    {\n");
    for (i, g) in scene.geometries.iter().enumerate() {
        let (lo, hi) = g.bounds();
        let double = scene.materials.get(g.material).is_some_and(|m| m.opacity < 1.0);
        let _ = writeln!(out, "        def Xform \"part{i}\"\n        {{");
        out.push_str(
            "            def Mesh \"mesh\" (\n                prepend apiSchemas = [\"MaterialBindingAPI\"]\n            )\n            {\n",
        );
        let _ = writeln!(out, "                uniform bool doubleSided = {}", u8::from(double));
        let _ = writeln!(
            out,
            "                float3[] extent = [({}, {}, {}), ({}, {}, {})]",
            num32(lo[0]),
            num32(lo[1]),
            num32(lo[2]),
            num32(hi[0]),
            num32(hi[1]),
            num32(hi[2])
        );
        out.push_str("                int[] faceVertexCounts = [");
        for k in 0..g.triangles() {
            out.push_str(if k == 0 { "3" } else { ", 3" });
        }
        out.push_str("]\n                int[] faceVertexIndices = [");
        for (k, ix) in g.indices.iter().enumerate() {
            if k > 0 {
                out.push_str(", ");
            }
            let _ = write!(out, "{ix}");
        }
        out.push_str("]\n");
        let _ = writeln!(out, "                rel material:binding = </{ROOT_PRIM}/Materials/m{}>", g.material);
        out.push_str("                normal3f[] normals = [");
        for (k, n) in g.normals.iter().enumerate() {
            let _ = write!(out, "{}({}, {}, {})", if k > 0 { ", " } else { "" }, num32(n[0]), num32(n[1]), num32(n[2]));
        }
        out.push_str("] (\n                    interpolation = \"vertex\"\n                )\n                point3f[] points = [");
        for (k, p) in g.positions.iter().enumerate() {
            let _ = write!(out, "{}({}, {}, {})", if k > 0 { ", " } else { "" }, num32(p[0]), num32(p[1]), num32(p[2]));
        }
        out.push_str("]\n                uniform token orientation = \"rightHanded\"\n                uniform token subdivisionScheme = \"none\"\n            }\n        }\n");
    }
    out.push_str("    }\n");
    // The elements.
    for n in &scene.nodes {
        let _ = writeln!(out, "\n    def Xform \"{}\" (", ident(&n.name));
        out.push_str("        customData = {\n            dictionary cadaddy = {\n");
        dict_body(&mut out, &n.meta, 16, 1);
        out.push_str("            }\n        }\n        instanceable = true\n");
        let _ = writeln!(out, "        prepend references = </{ROOT_PRIM}/Prototypes/part{}>\n    )\n    {{", n.geometry);
        let t = n.translation;
        let _ = writeln!(out, "        double3 xformOp:translate = ({}, {}, {})", num(t[0]), num(t[1]), num(t[2]));
        out.push_str("        uniform token[] xformOpOrder = [\"xformOp:translate\"]\n    }\n");
    }
    out.push_str("}\n");
    out
}

fn crc32(data: &[u8]) -> u32 {
    static TABLE: std::sync::OnceLock<[u32; 256]> = std::sync::OnceLock::new();
    let t = TABLE.get_or_init(|| {
        let mut t = [0u32; 256];
        for (i, slot) in t.iter_mut().enumerate() {
            let mut c = i as u32;
            for _ in 0..8 {
                c = if c & 1 != 0 { 0xEDB8_8320 ^ (c >> 1) } else { c >> 1 };
            }
            *slot = c;
        }
        t
    });
    let mut c = !0u32;
    for b in data {
        c = t.get(((c ^ u32::from(*b)) & 0xFF) as usize).copied().unwrap_or(0) ^ (c >> 8);
    }
    !c
}

/// An uncompressed zip of `files` (name, data) with every file's data at a multiple of 64 bytes.
pub(crate) fn zip_store(files: &[(&str, &[u8])]) -> Vec<u8> {
    let le16 = |v: usize| u16::try_from(v).unwrap_or(u16::MAX).to_le_bytes();
    let le32 = |v: usize| u32::try_from(v).unwrap_or(u32::MAX).to_le_bytes();
    let mut out: Vec<u8> = Vec::new();
    let mut central: Vec<u8> = Vec::new();
    for (name, data) in files {
        let offset = out.len();
        let crc = crc32(data);
        // Header (30) + name, then the extra field's own 4 bytes, then padding to 64.
        let base = offset + 30 + name.len() + 4;
        let pad = (ALIGN - base % ALIGN) % ALIGN;
        out.extend([0x50, 0x4B, 0x03, 0x04]);
        out.extend(le16(20)); // version needed: 2.0
        out.extend(le16(0)); // flags
        out.extend(le16(0)); // stored
        out.extend(le16(0)); // time
        out.extend(le16(0x21)); // date: 1980-01-01
        out.extend(crc.to_le_bytes());
        out.extend(le32(data.len()));
        out.extend(le32(data.len()));
        out.extend(le16(name.len()));
        out.extend(le16(4 + pad));
        out.extend(name.as_bytes());
        out.extend(PAD_ID.to_le_bytes());
        out.extend(le16(pad));
        out.extend(std::iter::repeat_n(0u8, pad));
        out.extend_from_slice(data);
        central.extend([0x50, 0x4B, 0x01, 0x02]);
        central.extend(le16(20)); // made by
        central.extend(le16(20));
        central.extend(le16(0));
        central.extend(le16(0));
        central.extend(le16(0));
        central.extend(le16(0x21));
        central.extend(crc.to_le_bytes());
        central.extend(le32(data.len()));
        central.extend(le32(data.len()));
        central.extend(le16(name.len()));
        central.extend(le16(0)); // extra
        central.extend(le16(0)); // comment
        central.extend(le16(0)); // disk
        central.extend(le16(0)); // internal attributes
        central.extend(le32(0)); // external attributes
        central.extend(le32(offset));
        central.extend(name.as_bytes());
    }
    let cd_offset = out.len();
    let cd_len = central.len();
    out.extend(central);
    out.extend([0x50, 0x4B, 0x05, 0x06]);
    out.extend(le16(0));
    out.extend(le16(0));
    out.extend(le16(files.len()));
    out.extend(le16(files.len()));
    out.extend(le32(cd_len));
    out.extend(le32(cd_offset));
    out.extend(le16(0));
    out
}

/// The scene as a `.usdz` package.
pub(crate) fn write(scene: &Scene) -> Vec<u8> {
    let text = layer(scene);
    zip_store(&[(ROOT, text.as_bytes())])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crc32_is_the_zip_one() {
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
        assert_eq!(crc32(b""), 0);
    }

    #[test]
    fn every_file_starts_on_64_bytes() {
        let z = zip_store(&[("a.usda", b"hello"), ("textures/b.png", &[1u8; 77])]);
        // Walk the local headers.
        let mut at = 0usize;
        for want in [&b"hello"[..], &[1u8; 77][..]] {
            assert_eq!(&z[at..at + 4], &[0x50, 0x4B, 0x03, 0x04]);
            let n = u16::from_le_bytes([z[at + 26], z[at + 27]]) as usize;
            let x = u16::from_le_bytes([z[at + 28], z[at + 29]]) as usize;
            let data = at + 30 + n + x;
            assert_eq!(data % 64, 0, "file data at {data}");
            assert_eq!(&z[data..data + want.len()], want);
            at = data + want.len();
        }
    }

    #[test]
    fn strings_and_keys_are_escaped() {
        assert_eq!(quote("a\"b\\c\nd"), "\"a\\\"b\\\\c\\nd\"");
        assert_eq!(ident("2 pole"), "_2_pole");
        assert_eq!(ident(""), "_");
    }
}
