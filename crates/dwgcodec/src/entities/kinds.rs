//! Entity data by type (spec §20.4.2 onward), each written as the DXF Reference's groups for that
//! entity.

use super::{Common, G, Group, MAX_ITEMS, Names, Obj, Value, count};
use crate::error::corrupt;
use crate::{Error, Result, Version};

/// The DXF name of a fixed object type that is an entity (spec §20.3).
pub fn fixed_name(t: u16) -> Option<&'static str> {
    Some(match t {
        1 => "TEXT",
        2 => "ATTRIB",
        3 => "ATTDEF",
        6 => "SEQEND",
        7 => "INSERT",
        8 => "MINSERT",
        10..=14 => "VERTEX",
        15 | 16 | 29 | 30 => "POLYLINE",
        17 => "ARC",
        18 => "CIRCLE",
        19 => "LINE",
        27 => "POINT",
        28 => "3DFACE",
        31 => "SOLID",
        32 => "TRACE",
        35 => "ELLIPSE",
        36 => "SPLINE",
        40 => "RAY",
        41 => "XLINE",
        34 => "VIEWPORT",
        44 => "MTEXT",
        47 => "MLINE",
        20..=26 => "DIMENSION",
        45 => "LEADER",
        46 => "TOLERANCE",
        77 => "LWPOLYLINE",
        78 => "HATCH",
        _ => return None,
    })
}

/// Entity names decoded here (fixed types and the same entities as classes).
pub fn known(name: &str) -> bool {
    matches!(
        name,
        "TEXT"
            | "ATTRIB"
            | "ATTDEF"
            | "SEQEND"
            | "INSERT"
            | "MINSERT"
            | "VERTEX"
            | "POLYLINE"
            | "ARC"
            | "CIRCLE"
            | "LINE"
            | "POINT"
            | "3DFACE"
            | "SOLID"
            | "TRACE"
            | "ELLIPSE"
            | "SPLINE"
            | "RAY"
            | "XLINE"
            | "MTEXT"
            | "LWPOLYLINE"
            | "HATCH"
            | "DIMENSION"
            | "LEADER"
            | "TOLERANCE"
            | "MLINE"
            | "IMAGE"
            | "WIPEOUT"
            | "VIEWPORT"
            | "MULTILEADER"
    )
}

fn deg(r: f64) -> f64 {
    r.to_degrees()
}

/// Decodes `name`'s data from `o` into `out` (after the common groups).
pub fn decode(name: &str, o: &mut Obj, c: &Common, names: &Names, out: &mut Vec<Group>, children: &mut Vec<u64>) -> Result<()> {
    let _ = c;
    let mut g = G(out);
    match name {
        "LINE" => line(o, &mut g),
        "CIRCLE" | "ARC" => {
            let center = o.d.bd3()?;
            let radius = o.d.bd()?;
            let thickness = o.d.bt(true)?;
            let ext = o.d.be(true)?;
            g.s(100, "AcDbCircle");
            g.thickness(thickness);
            g.p(10, center);
            g.f(40, radius);
            g.extrusion(ext);
            if name == "ARC" {
                let (a, b) = (o.d.bd()?, o.d.bd()?);
                g.s(100, "AcDbArc");
                g.f(50, deg(a));
                g.f(51, deg(b));
            }
            Ok(())
        }
        "POINT" => {
            let p = o.d.bd3()?;
            let thickness = o.d.bt(true)?;
            let ext = o.d.be(true)?;
            let angle = o.d.bd()?;
            g.s(100, "AcDbPoint");
            g.p(10, p);
            g.thickness(thickness);
            g.extrusion(ext);
            if angle != 0.0 {
                g.f(50, deg(angle));
            }
            Ok(())
        }
        "ELLIPSE" => {
            let center = o.d.bd3()?;
            let major = o.d.bd3()?;
            let ext = o.d.bd3()?;
            let ratio = o.d.bd()?;
            let (a, b) = (o.d.bd()?, o.d.bd()?);
            g.s(100, "AcDbEllipse");
            g.p(10, center);
            g.p(11, major);
            g.extrusion(ext);
            g.f(40, ratio);
            g.f(41, a);
            g.f(42, b);
            Ok(())
        }
        "TEXT" => {
            let t = text_data(o)?;
            let style = o.href()?;
            text_groups(&mut g, &t, style.and_then(|h| names.name(h)), true);
            g.s(100, "AcDbText");
            if t.valign != 0 {
                g.i(73, i64::from(t.valign));
            }
            Ok(())
        }
        "ATTRIB" | "ATTDEF" => attrib(name, o, names, &mut g),
        "SEQEND" => Ok(()),
        "INSERT" | "MINSERT" => insert(name, o, names, &mut g, children),
        "VERTEX" => vertex(o, &mut g),
        "POLYLINE" => polyline(o, &mut g, children),
        "SOLID" | "TRACE" => {
            let thickness = o.d.bt(true)?;
            let elevation = o.d.bd()?;
            let pts = [o.d.rd2()?, o.d.rd2()?, o.d.rd2()?, o.d.rd2()?];
            let ext = o.d.be(true)?;
            g.s(100, "AcDbTrace");
            for (i, p) in pts.iter().enumerate() {
                g.p(10 + i as i32, [p[0], p[1], elevation]);
            }
            g.thickness(thickness);
            g.extrusion(ext);
            Ok(())
        }
        "3DFACE" => {
            let no_flags = o.d.b()?;
            let z_zero = o.d.b()?;
            let x = o.d.rd()?;
            let y = o.d.rd()?;
            let z = if z_zero { 0.0 } else { o.d.rd()? };
            let mut prev = [x, y, z];
            g.s(100, "AcDbFace");
            g.p(10, prev);
            for code in [11, 12, 13] {
                let p = [o.d.dd(prev[0])?, o.d.dd(prev[1])?, o.d.dd(prev[2])?];
                g.p(code, p);
                prev = p;
            }
            if !no_flags {
                let f = o.d.bs()?;
                if f != 0 {
                    g.i(70, i64::from(f));
                }
            }
            Ok(())
        }
        "RAY" | "XLINE" => {
            let p = o.d.bd3()?;
            let v = o.d.bd3()?;
            g.s(100, if name == "RAY" { "AcDbRay" } else { "AcDbXline" });
            g.p(10, p);
            g.p(11, v);
            Ok(())
        }
        "SPLINE" => spline(o, &mut g),
        "MTEXT" => mtext(o, names, &mut g),
        "LWPOLYLINE" => lwpolyline(o, &mut g),
        "HATCH" => hatch(o, &mut g),
        "MLINE" => mline(o, names, &mut g),
        "IMAGE" | "WIPEOUT" => image(name, o, &mut g),
        "VIEWPORT" => viewport(o, &mut g),
        "MULTILEADER" => mleader(o, &mut g),
        "DIMENSION" => dimension(o, names, &mut g),
        "LEADER" => leader(o, names, &mut g),
        "TOLERANCE" => {
            let at = o.d.bd3()?;
            let xdir = o.d.bd3()?;
            let ext = o.d.bd3()?;
            let text = o.text()?;
            let style = o.href()?;
            g.s(100, "AcDbFcf");
            if let Some(s) = style.and_then(|h| names.name(h)) {
                g.s(3, s);
            }
            g.p(10, at);
            g.s(1, text);
            g.extrusion(ext);
            g.p(11, xdir);
            Ok(())
        }
        _ => Err(Error::Unsupported(format!("{name} entities are not decoded yet"))),
    }
}

/// LINE (spec §20.4.21): from R2000 the z's are flagged zero and the end point's coordinates are
/// stored as defaults from the start's.
fn line(o: &mut Obj, g: &mut G) -> Result<()> {
    let z_zero = o.d.b()?;
    let sx = o.d.rd()?;
    let ex = o.d.dd(sx)?;
    let sy = o.d.rd()?;
    let ey = o.d.dd(sy)?;
    let (sz, ez) = if z_zero {
        (0.0, 0.0)
    } else {
        let sz = o.d.rd()?;
        (sz, o.d.dd(sz)?)
    };
    let thickness = o.d.bt(true)?;
    let ext = o.d.be(true)?;
    g.s(100, "AcDbLine");
    g.thickness(thickness);
    g.p(10, [sx, sy, sz]);
    g.p(11, [ex, ey, ez]);
    g.extrusion(ext);
    Ok(())
}

/// The text fields TEXT, ATTRIB and ATTDEF share (spec §20.4.3).
struct TextData {
    elevation: f64,
    insert: [f64; 2],
    align: Option<[f64; 2]>,
    ext: [f64; 3],
    thickness: f64,
    oblique: f64,
    rotation: f64,
    height: f64,
    width: f64,
    value: String,
    generation: i16,
    halign: i16,
    valign: i16,
}

fn text_data(o: &mut Obj) -> Result<TextData> {
    let flags = o.d.rc()?;
    let elevation = if flags & 0x01 == 0 { o.d.rd()? } else { 0.0 };
    let insert = o.d.rd2()?;
    let align = if flags & 0x02 == 0 { Some([o.d.dd(insert[0])?, o.d.dd(insert[1])?]) } else { None };
    let ext = o.d.be(true)?;
    let thickness = o.d.bt(true)?;
    let oblique = if flags & 0x04 == 0 { o.d.rd()? } else { 0.0 };
    let rotation = if flags & 0x08 == 0 { o.d.rd()? } else { 0.0 };
    let height = o.d.rd()?;
    let width = if flags & 0x10 == 0 { o.d.rd()? } else { 1.0 };
    let value = o.text()?;
    let generation = if flags & 0x20 == 0 { o.d.bs()? } else { 0 };
    let halign = if flags & 0x40 == 0 { o.d.bs()? } else { 0 };
    let valign = if flags & 0x80 == 0 { o.d.bs()? } else { 0 };
    Ok(TextData { elevation, insert, align, ext, thickness, oblique, rotation, height, width, value, generation, halign, valign })
}

/// The AcDbText groups (TEXT, and the first part of ATTRIB and ATTDEF).
fn text_groups(g: &mut G, t: &TextData, style: Option<&str>, value: bool) {
    g.s(100, "AcDbText");
    g.thickness(t.thickness);
    g.p(10, [t.insert[0], t.insert[1], t.elevation]);
    g.f(40, t.height);
    if value {
        g.s(1, t.value.clone());
    }
    if t.rotation != 0.0 {
        g.f(50, deg(t.rotation));
    }
    if t.width != 1.0 {
        g.f(41, t.width);
    }
    if t.oblique != 0.0 {
        g.f(51, deg(t.oblique));
    }
    if let Some(s) = style {
        g.s(7, s);
    }
    if t.generation != 0 {
        g.i(71, i64::from(t.generation));
    }
    if t.halign != 0 {
        g.i(72, i64::from(t.halign));
    }
    if let Some(a) = t.align
        && (t.halign != 0 || t.valign != 0)
    {
        g.p(11, [a[0], a[1], t.elevation]);
    }
    g.extrusion(t.ext);
}

/// ATTRIB and ATTDEF (spec §20.4.4, §20.4.5).
fn attrib(name: &str, o: &mut Obj, names: &Names, g: &mut G) -> Result<()> {
    let t = text_data(o)?;
    if o.at_least(Version::R2010) {
        o.d.rc()?;
    }
    if o.at_least(Version::R2018) {
        let kind = o.d.rc()?;
        if kind != 1 {
            return Err(Error::Unsupported("multiline attributes are not decoded yet".into()));
        }
    }
    let tag = o.text()?;
    let field_len = o.d.bs()?;
    let flags = o.d.rc()?;
    let lock = if o.at_least(Version::R2007) { o.d.b()? } else { false };
    let prompt = if name == "ATTDEF" {
        if o.at_least(Version::R2010) {
            o.d.rc()?;
        }
        Some(o.text()?)
    } else {
        None
    };
    let style = o.href()?;
    text_groups(g, &t, style.and_then(|h| names.name(h)), true);
    match prompt {
        Some(p) => {
            g.s(100, "AcDbAttributeDefinition");
            g.s(3, p);
        }
        None => g.s(100, "AcDbAttribute"),
    }
    g.s(2, tag);
    g.i(70, i64::from(flags));
    if field_len != 0 {
        g.i(73, i64::from(field_len));
    }
    if t.valign != 0 {
        g.i(74, i64::from(t.valign));
    }
    if lock {
        g.i(280, 1);
    }
    Ok(())
}

/// INSERT and MINSERT (spec §20.4.9, §20.4.10).
fn insert(name: &str, o: &mut Obj, names: &Names, g: &mut G, children: &mut Vec<u64>) -> Result<()> {
    let at = o.d.bd3()?;
    let scale = match o.d.bb()? {
        0 => {
            let x = o.d.rd()?;
            [x, o.d.dd(x)?, o.d.dd(x)?]
        }
        1 => [1.0, o.d.dd(1.0)?, o.d.dd(1.0)?],
        2 => {
            let x = o.d.rd()?;
            [x, x, x]
        }
        _ => [1.0, 1.0, 1.0],
    };
    let rotation = o.d.bd()?;
    let ext = o.d.bd3()?;
    let has_attribs = o.d.b()?;
    let owned = if has_attribs && o.at_least(Version::R2004) { count(o.d.bl()?, "attributes")? } else { 0 };
    let grid = if name == "MINSERT" { Some((o.d.bs()?, o.d.bs()?, o.d.bd()?, o.d.bd()?)) } else { None };
    let block = o.href()?;
    if has_attribs {
        if o.at_least(Version::R2004) {
            for _ in 0..owned {
                if let Some(h) = o.href()? {
                    children.push(h);
                }
            }
        } else {
            // First and last attribute (0 for none): the chain between them is followed in
            // [`super::decode_all`].
            for _ in 0..2 {
                children.push(o.href()?.unwrap_or(0));
            }
        }
        if let Some(h) = o.href()? {
            children.push(h);
        }
    }
    g.s(100, if name == "MINSERT" { "AcDbMInsertBlock" } else { "AcDbBlockReference" });
    if has_attribs {
        g.i(66, 1);
    }
    g.s(2, block.and_then(|h| names.block(h)).unwrap_or(""));
    g.p(10, at);
    if scale[0] != 1.0 {
        g.f(41, scale[0]);
    }
    if scale[1] != 1.0 {
        g.f(42, scale[1]);
    }
    if scale[2] != 1.0 {
        g.f(43, scale[2]);
    }
    if rotation != 0.0 {
        g.f(50, deg(rotation));
    }
    if let Some((cols, rows, cs, rs)) = grid {
        g.i(70, i64::from(cols));
        g.i(71, i64::from(rows));
        g.f(44, cs);
        g.f(45, rs);
    }
    g.extrusion(ext);
    Ok(())
}

/// VERTEX of each polyline kind (spec §20.4.11–§20.4.15), by object type.
fn vertex(o: &mut Obj, g: &mut G) -> Result<()> {
    g.s(100, "AcDbVertex");
    match o.type_code {
        10 => {
            let flags = o.d.rc()?;
            let p = o.d.bd3()?;
            let start = o.d.bd()?;
            let end = if start < 0.0 { start.abs() } else { o.d.bd()? };
            let bulge = o.d.bd()?;
            if o.at_least(Version::R2010) {
                o.d.bl()?;
            }
            let tangent = o.d.bd()?;
            g.s(100, "AcDb2dVertex");
            g.p(10, p);
            if start != 0.0 {
                g.f(40, start.abs());
            }
            if end != 0.0 {
                g.f(41, end);
            }
            if bulge != 0.0 {
                g.f(42, bulge);
            }
            g.i(70, i64::from(flags));
            if flags & 2 != 0 {
                g.f(50, deg(tangent));
            }
        }
        11..=13 => {
            let flags = o.d.rc()?;
            let p = o.d.bd3()?;
            g.s(
                100,
                match o.type_code {
                    11 => "AcDb3dPolylineVertex",
                    12 => "AcDbPolygonMeshVertex",
                    _ => "AcDbPolyFaceMeshVertex",
                },
            );
            g.p(10, p);
            let f = i64::from(flags) | if o.type_code == 13 { 128 | 64 } else { 0 };
            g.i(70, f);
        }
        _ => {
            let idx = [o.d.bs()?, o.d.bs()?, o.d.bs()?, o.d.bs()?];
            g.s(100, "AcDbFaceRecord");
            g.p(10, [0.0, 0.0, 0.0]);
            g.i(70, 128);
            for (i, v) in idx.iter().enumerate() {
                if *v != 0 {
                    g.i(71 + i as i32, i64::from(*v));
                }
            }
        }
    }
    Ok(())
}

/// POLYLINE of each kind (spec §20.4.16, §20.4.17, §20.4.29, §20.4.30): its own groups; its
/// vertices and SEQEND are children.
fn polyline(o: &mut Obj, g: &mut G, children: &mut Vec<u64>) -> Result<()> {
    let t = o.type_code;
    let mut owned = 0;
    match t {
        15 => {
            let flags = o.d.bs()?;
            let curve = o.d.bs()?;
            let (sw, ew) = (o.d.bd()?, o.d.bd()?);
            let thickness = o.d.bt(true)?;
            let elevation = o.d.bd()?;
            let ext = o.d.be(true)?;
            if o.at_least(Version::R2004) {
                owned = count(o.d.bl()?, "vertices")?;
            }
            g.s(100, "AcDb2dPolyline");
            g.i(66, 1);
            g.p(10, [0.0, 0.0, elevation]);
            g.thickness(thickness);
            g.i(70, i64::from(flags));
            if sw != 0.0 {
                g.f(40, sw);
            }
            if ew != 0.0 {
                g.f(41, ew);
            }
            if curve != 0 {
                g.i(75, i64::from(curve));
            }
            g.extrusion(ext);
        }
        16 => {
            let spline = o.d.rc()?;
            let closed = o.d.rc()?;
            if o.at_least(Version::R2004) {
                owned = count(o.d.bl()?, "vertices")?;
            }
            let mut flags = 8 | i64::from(closed & 1);
            if spline & 4 != 0 {
                flags |= 4;
            }
            g.s(100, "AcDb3dPolyline");
            g.i(66, 1);
            g.p(10, [0.0, 0.0, 0.0]);
            g.i(70, flags);
            let curve = match spline & 3 {
                1 => 5,
                2 => 6,
                _ => 0,
            };
            if spline & 4 != 0 && curve != 0 {
                g.i(75, curve);
            }
        }
        29 => {
            let nv = o.d.bs()?;
            let nf = o.d.bs()?;
            if o.at_least(Version::R2004) {
                owned = count(o.d.bl()?, "vertices")?;
            }
            g.s(100, "AcDbPolyFaceMesh");
            g.i(66, 1);
            g.p(10, [0.0, 0.0, 0.0]);
            g.i(70, 64);
            g.i(71, i64::from(nv));
            g.i(72, i64::from(nf));
        }
        _ => {
            let flags = o.d.bs()?;
            let curve = o.d.bs()?;
            let (m, n, md, nd) = (o.d.bs()?, o.d.bs()?, o.d.bs()?, o.d.bs()?);
            if o.at_least(Version::R2004) {
                owned = count(o.d.bl()?, "vertices")?;
            }
            g.s(100, "AcDbPolygonMesh");
            g.i(66, 1);
            g.p(10, [0.0, 0.0, 0.0]);
            g.i(70, i64::from(flags) | 16);
            g.i(71, i64::from(m));
            g.i(72, i64::from(n));
            if md != 0 {
                g.i(73, i64::from(md));
            }
            if nd != 0 {
                g.i(74, i64::from(nd));
            }
            if curve != 0 {
                g.i(75, i64::from(curve));
            }
        }
    }
    if o.at_least(Version::R2004) {
        for _ in 0..owned {
            if let Some(h) = o.href()? {
                children.push(h);
            }
        }
    } else {
        // First and last vertex (0 for none), as for INSERT's attributes.
        for _ in 0..2 {
            children.push(o.href()?.unwrap_or(0));
        }
    }
    if let Some(h) = o.href()? {
        children.push(h);
    }
    Ok(())
}

/// SPLINE (spec §20.4.36).
fn spline(o: &mut Obj, g: &mut G) -> Result<()> {
    let mut scenario = o.d.bl()?;
    if o.at_least(Version::R2013) {
        let flags1 = o.d.bl()?;
        // The knot parameterization.
        o.d.bl()?;
        if flags1 & 1 != 0 {
            scenario = 2;
        }
    }
    let degree = o.d.bl()?;
    g.s(100, "AcDbSpline");
    let (mut fit_tol, mut start_tan, mut end_tan, mut fits) = (0.0, None, None, Vec::new());
    let (mut rational, mut closed, mut periodic) = (false, false, false);
    let (mut knot_tol, mut ctrl_tol) = (0.0, 0.0);
    let (mut knots, mut ctrls, mut weights) = (Vec::new(), Vec::new(), Vec::new());
    match scenario {
        2 => {
            fit_tol = o.d.bd()?;
            start_tan = Some(o.d.bd3()?);
            end_tan = Some(o.d.bd3()?);
            let n = count(o.d.bl()?, "fit points")?;
            for _ in 0..n {
                fits.push(o.d.bd3()?);
            }
        }
        1 => {
            rational = o.d.b()?;
            closed = o.d.b()?;
            periodic = o.d.b()?;
            knot_tol = o.d.bd()?;
            ctrl_tol = o.d.bd()?;
            let nk = count(o.d.bl()?, "knots")?;
            let nc = count(o.d.bl()?, "control points")?;
            let weighted = o.d.b()?;
            if nk.saturating_add(nc) > MAX_ITEMS {
                return Err(Error::TooLarge("a spline"));
            }
            for _ in 0..nk {
                knots.push(o.d.bd()?);
            }
            for _ in 0..nc {
                ctrls.push(o.d.bd3()?);
                if weighted {
                    weights.push(o.d.bd()?);
                }
            }
        }
        _ => return Err(corrupt("a spline's scenario")),
    }
    // Planar (8) isn't stored in the DWG data: not set here.
    let mut flags = 0;
    if closed {
        flags |= 1;
    }
    if periodic {
        flags |= 2;
    }
    if rational {
        flags |= 4;
    }
    if scenario == 2 && fits.len() > 1 {
        flags |= 1024;
    }
    g.i(70, flags);
    g.i(71, i64::from(degree));
    g.i(72, knots.len() as i64);
    g.i(73, ctrls.len() as i64);
    g.i(74, fits.len() as i64);
    if knot_tol != 0.0 {
        g.f(42, knot_tol);
    }
    if ctrl_tol != 0.0 {
        g.f(43, ctrl_tol);
    }
    if scenario == 2 {
        g.f(44, fit_tol);
    }
    if let Some(t) = start_tan
        && t != [0.0; 3]
    {
        g.p(12, t);
    }
    if let Some(t) = end_tan
        && t != [0.0; 3]
    {
        g.p(13, t);
    }
    for k in knots {
        g.f(40, k);
    }
    for w in &weights {
        g.f(41, *w);
    }
    for p in ctrls {
        g.p(10, p);
    }
    for p in fits {
        g.p(11, p);
    }
    Ok(())
}

/// MTEXT (spec §20.4.46): the text in 250-character chunks (3, then 1 for the last).
fn mtext(o: &mut Obj, names: &Names, g: &mut G) -> Result<()> {
    let at = o.d.bd3()?;
    let ext = o.d.bd3()?;
    let xdir = o.d.bd3()?;
    let width = o.d.bd()?;
    let defined_height = if o.at_least(Version::R2007) { Some(o.d.bd()?) } else { None };
    let height = o.d.bd()?;
    let attach = o.d.bs()?;
    let dir = o.d.bs()?;
    let ext_h = o.d.bd()?;
    let ext_w = o.d.bd()?;
    let text = o.text()?;
    let spacing_style = o.d.bs()?;
    let spacing = o.d.bd()?;
    o.d.b()?;
    let mut bg = None;
    if o.at_least(Version::R2004) {
        let flags = o.d.bl()?;
        let has = if o.at_least(Version::R2018) { flags & 0x11 != 0 } else { flags & 1 != 0 };
        if has {
            let scale = o.d.bd()?;
            let color = o.d.bs()?;
            let rgb = o.d.bl()? as u32;
            let cflags = o.d.rc()?;
            if cflags & 1 != 0 {
                o.text()?;
            }
            if cflags & 2 != 0 {
                o.text()?;
            }
            let transparency = o.d.bl()?;
            bg = Some((flags, scale, color, rgb, transparency));
        } else if flags != 0 {
            bg = Some((flags, 0.0, 0, 0, 0));
        }
    }
    let style = o.href()?;
    g.s(100, "AcDbMText");
    g.p(10, at);
    g.f(40, height);
    g.f(41, width);
    if let Some(h) = defined_height
        && h != 0.0
    {
        g.f(46, h);
    }
    g.i(71, i64::from(attach));
    g.i(72, i64::from(dir));
    let chars: Vec<char> = text.chars().collect();
    let chunks: Vec<String> = chars.chunks(250).map(|c| c.iter().collect()).collect();
    if let Some((last, rest)) = chunks.split_last() {
        for c in rest {
            g.s(3, c.clone());
        }
        g.s(1, last.clone());
    } else {
        g.s(1, "");
    }
    if let Some(s) = style.and_then(|h| names.name(h)) {
        g.s(7, s);
    }
    g.extrusion(ext);
    g.p(11, xdir);
    g.f(42, ext_w);
    g.f(43, ext_h);
    g.i(73, i64::from(spacing_style));
    g.f(44, spacing);
    if let Some((flags, scale, color, rgb, transparency)) = bg {
        g.i(90, i64::from(flags));
        if flags & 1 != 0 {
            g.i(63, i64::from(color & 0x1ff));
            if rgb >> 24 == 0xC2 {
                g.i(421, i64::from(rgb & 0xFF_FFFF));
            }
            g.f(45, scale);
            g.i(441, i64::from(transparency));
        }
    }
    Ok(())
}

/// LWPOLYLINE (spec §20.4.85).
fn lwpolyline(o: &mut Obj, g: &mut G) -> Result<()> {
    let flags = o.d.bs()?;
    let width = if flags & 4 != 0 { o.d.bd()? } else { 0.0 };
    let elevation = if flags & 8 != 0 { o.d.bd()? } else { 0.0 };
    let thickness = if flags & 2 != 0 { o.d.bd()? } else { 0.0 };
    let ext = if flags & 1 != 0 { o.d.bd3()? } else { [0.0, 0.0, 1.0] };
    let n = count(o.d.bl()?, "vertices")?;
    let nb = if flags & 16 != 0 { count(o.d.bl()?, "bulges")? } else { 0 };
    let nid = if o.at_least(Version::R2010) && flags & 1024 != 0 { count(o.d.bl()?, "vertex ids")? } else { 0 };
    let nw = if flags & 32 != 0 { count(o.d.bl()?, "widths")? } else { 0 };
    if n.saturating_add(nb).saturating_add(nw) > MAX_ITEMS {
        return Err(Error::TooLarge("a polyline"));
    }
    let mut pts: Vec<[f64; 2]> = Vec::with_capacity(n.min(1 << 16));
    for i in 0..n {
        let p = match pts.last() {
            Some(prev) if i > 0 => [o.d.dd(prev[0])?, o.d.dd(prev[1])?],
            _ => o.d.rd2()?,
        };
        pts.push(p);
    }
    let bulges: Vec<f64> = (0..nb).map(|_| o.d.bd()).collect::<Result<_>>()?;
    for _ in 0..nid {
        o.d.bl()?;
    }
    let widths: Vec<(f64, f64)> = (0..nw).map(|_| Ok((o.d.bd()?, o.d.bd()?))).collect::<Result<_>>()?;
    g.s(100, "AcDbPolyline");
    g.i(90, n as i64);
    let mut f = 0;
    if flags & 512 != 0 {
        f |= 1;
    }
    if flags & 256 != 0 {
        f |= 128;
    }
    g.i(70, f);
    if flags & 4 != 0 {
        g.f(43, width);
    }
    if elevation != 0.0 {
        g.f(38, elevation);
    }
    g.thickness(thickness);
    for (i, p) in pts.iter().enumerate() {
        g.p2(10, *p);
        if let Some((s, e)) = widths.get(i)
            && (*s != 0.0 || *e != 0.0)
        {
            g.f(40, *s);
            g.f(41, *e);
        }
        if let Some(b) = bulges.get(i)
            && *b != 0.0
        {
            g.f(42, *b);
        }
    }
    g.extrusion(ext);
    Ok(())
}

/// HATCH (spec §20.4.80): gradient, boundary paths (polylines or edges: lines, arcs, elliptical
/// arcs, splines), style, pattern lines and seed points; the boundary objects' handles at the end.
fn hatch(o: &mut Obj, g: &mut G) -> Result<()> {
    let mut gradient = None;
    if o.at_least(Version::R2004) {
        let is_gradient = o.d.bl()?;
        let reserved = o.d.bl()?;
        let angle = o.d.bd()?;
        let shift = o.d.bd()?;
        let single = o.d.bl()?;
        let tint = o.d.bd()?;
        let n = count(o.d.bl()?, "gradient colours")?;
        let mut colors = Vec::new();
        for _ in 0..n.min(64) {
            let value = o.d.bd()?;
            o.d.bs()?;
            let rgb = o.d.bl()? as u32;
            o.d.rc()?;
            colors.push((value, rgb));
        }
        let name = o.text()?;
        gradient = Some((is_gradient, reserved, angle, shift, single, tint, colors, name));
    }
    let elevation = o.d.bd()?;
    let ext = o.d.bd3()?;
    let pattern = o.text()?;
    let solid = o.d.b()?;
    let assoc = o.d.b()?;
    let npaths = count(o.d.bl()?, "hatch boundaries")?;
    g.s(100, "AcDbHatch");
    g.p(10, [0.0, 0.0, elevation]);
    g.p(210, ext);
    g.s(2, pattern);
    g.i(70, i64::from(solid));
    g.i(71, i64::from(assoc));
    g.i(91, npaths as i64);
    let mut derived = false;
    let mut boundary_counts = Vec::new();
    for _ in 0..npaths {
        let flags = o.d.bl()?;
        if flags & 4 != 0 {
            derived = true;
        }
        g.i(92, i64::from(flags));
        if flags & 2 == 0 {
            let n = count(o.d.bl()?, "boundary edges")?;
            g.i(93, n as i64);
            for _ in 0..n {
                let kind = o.d.rc()?;
                g.i(72, i64::from(kind));
                match kind {
                    1 => {
                        g.p2(10, o.d.rd2()?);
                        g.p2(11, o.d.rd2()?);
                    }
                    2 => {
                        g.p2(10, o.d.rd2()?);
                        g.f(40, o.d.bd()?);
                        g.f(50, deg(o.d.bd()?));
                        g.f(51, deg(o.d.bd()?));
                        g.i(73, i64::from(o.d.b()?));
                    }
                    3 => {
                        g.p2(10, o.d.rd2()?);
                        g.p2(11, o.d.rd2()?);
                        g.f(40, o.d.bd()?);
                        g.f(50, deg(o.d.bd()?));
                        g.f(51, deg(o.d.bd()?));
                        g.i(73, i64::from(o.d.b()?));
                    }
                    4 => {
                        let degree = o.d.bl()?;
                        let rational = o.d.b()?;
                        let periodic = o.d.b()?;
                        let nk = count(o.d.bl()?, "knots")?;
                        let nc = count(o.d.bl()?, "control points")?;
                        g.i(94, i64::from(degree));
                        g.i(73, i64::from(rational));
                        g.i(74, i64::from(periodic));
                        g.i(95, nk as i64);
                        g.i(96, nc as i64);
                        for _ in 0..nk {
                            g.f(40, o.d.bd()?);
                        }
                        for _ in 0..nc {
                            g.p2(10, o.d.rd2()?);
                            if rational {
                                g.f(42, o.d.bd()?);
                            }
                        }
                        if o.at_least(Version::R2010) {
                            let nf = count(o.d.bl()?, "fit points")?;
                            g.i(97, nf as i64);
                            if nf > 0 {
                                for _ in 0..nf {
                                    g.p2(11, o.d.rd2()?);
                                }
                                g.p2(12, o.d.rd2()?);
                                g.p2(13, o.d.rd2()?);
                            }
                        }
                    }
                    _ => return Err(corrupt("a hatch boundary edge type")),
                }
            }
        } else {
            let bulges = o.d.b()?;
            let closed = o.d.b()?;
            let n = count(o.d.bl()?, "boundary vertices")?;
            g.i(72, i64::from(bulges));
            g.i(73, i64::from(closed));
            g.i(93, n as i64);
            for _ in 0..n {
                g.p2(10, o.d.rd2()?);
                if bulges {
                    g.f(42, o.d.bd()?);
                }
            }
        }
        let nb = count(o.d.bl()?, "boundary objects")?;
        boundary_counts.push(nb);
        g.i(97, nb as i64);
        // The objects' handles come after everything else: placeholders, filled in below.
        for _ in 0..nb {
            g.h(330, 0);
        }
    }
    let style = o.d.bs()?;
    let ptype = o.d.bs()?;
    g.i(75, i64::from(style));
    g.i(76, i64::from(ptype));
    if !solid {
        let angle = o.d.bd()?;
        let scale = o.d.bd()?;
        let double = o.d.b()?;
        let nlines = count(i32::from(o.d.bs()?), "pattern lines")?;
        g.f(52, deg(angle));
        g.f(41, scale);
        g.i(77, i64::from(double));
        g.i(78, nlines as i64);
        for _ in 0..nlines {
            g.f(53, deg(o.d.bd()?));
            g.f(43, o.d.bd()?);
            g.f(44, o.d.bd()?);
            g.f(45, o.d.bd()?);
            g.f(46, o.d.bd()?);
            let nd = count(i32::from(o.d.bs()?), "dashes")?;
            g.i(79, nd as i64);
            for _ in 0..nd {
                g.f(49, o.d.bd()?);
            }
        }
    }
    if derived {
        g.f(47, o.d.bd()?);
    }
    let nseeds = count(o.d.bl()?, "seed points")?;
    g.i(98, nseeds as i64);
    for _ in 0..nseeds {
        g.p2(10, o.d.rd2()?);
    }
    if let Some((is_gradient, reserved, angle, shift, single, tint, colors, name)) = gradient.filter(|g| g.0 != 0) {
        g.i(450, i64::from(is_gradient));
        g.i(451, i64::from(reserved));
        g.f(460, angle);
        g.f(461, shift);
        g.i(452, i64::from(single));
        g.f(462, tint);
        g.i(453, colors.len() as i64);
        for (value, rgb) in colors {
            g.f(463, value);
            g.i(421, i64::from(rgb & 0xFF_FFFF));
        }
        g.s(470, name);
    }
    // The boundary objects, in order, into the placeholders.
    let total: usize = boundary_counts.iter().sum();
    let mut handles = Vec::with_capacity(total.min(1 << 16));
    for _ in 0..total {
        handles.push(o.href()?.unwrap_or(0));
    }
    let mut it = handles.into_iter();
    for grp in g.0.iter_mut() {
        if grp.code == 330 && grp.value == Value::Handle(0) {
            grp.value = Value::Handle(it.next().unwrap_or(0));
        }
    }
    Ok(())
}

/// DIMENSION of each kind (spec §20.4.20–§20.4.26): the common dimension data, then the kind's
/// points; the dimension style and the block that draws it are handles.
fn dimension(o: &mut Obj, names: &Names, g: &mut G) -> Result<()> {
    if o.at_least(Version::R2010) {
        o.d.rc()?;
    }
    let ext = o.d.bd3()?;
    let mid = o.d.rd2()?;
    let elevation = o.d.bd()?;
    let flags1 = o.d.rc()?;
    let text = o.text()?;
    let text_rot = o.d.bd()?;
    let horiz = o.d.bd()?;
    let _ins_scale = o.d.bd3()?;
    let _ins_rot = o.d.bd()?;
    let attach = o.d.bs()?;
    let spacing_style = o.d.bs()?;
    let spacing = o.d.bd()?;
    let measurement = o.d.bd()?;
    if o.at_least(Version::R2007) {
        o.d.b()?;
        o.d.b()?;
        o.d.b()?;
    }
    let p12 = o.d.rd2()?;
    let t = o.type_code;
    let base = match t {
        20 => 6,
        21 => 0,
        22 => 1,
        23 => 5,
        24 => 2,
        25 => 4,
        _ => 3,
    };
    // Points by kind, in DXF terms.
    let pts: Vec<(i32, [f64; 3])>;
    let (mut rotation, mut oblique, mut leader, mut ord_x) = (None, None, None, false);
    match t {
        20 => {
            let p10 = o.d.bd3()?;
            let p13 = o.d.bd3()?;
            let p14 = o.d.bd3()?;
            ord_x = o.d.rc()? & 1 != 0;
            pts = vec![(10, p10), (13, p13), (14, p14)];
        }
        21 | 22 => {
            let p13 = o.d.bd3()?;
            let p14 = o.d.bd3()?;
            let p10 = o.d.bd3()?;
            oblique = Some(o.d.bd()?);
            if t == 21 {
                rotation = Some(o.d.bd()?);
            }
            pts = vec![(10, p10), (13, p13), (14, p14)];
        }
        23 => {
            let p10 = o.d.bd3()?;
            let p13 = o.d.bd3()?;
            let p14 = o.d.bd3()?;
            let p15 = o.d.bd3()?;
            pts = vec![(10, p10), (13, p13), (14, p14), (15, p15)];
        }
        24 => {
            let p16 = o.d.rd2()?;
            let p13 = o.d.bd3()?;
            let p14 = o.d.bd3()?;
            let p15 = o.d.bd3()?;
            let p10 = o.d.bd3()?;
            pts = vec![(10, p10), (13, p13), (14, p14), (15, p15), (16, [p16[0], p16[1], elevation])];
        }
        25 => {
            let p10 = o.d.bd3()?;
            let p15 = o.d.bd3()?;
            leader = Some(o.d.bd()?);
            pts = vec![(10, p10), (15, p15)];
        }
        _ => {
            // Diameter: the point on the arc (15) comes first.
            let p15 = o.d.bd3()?;
            let p10 = o.d.bd3()?;
            leader = Some(o.d.bd()?);
            pts = vec![(10, p10), (15, p15)];
        }
    }
    let style = o.href()?;
    let block = o.href()?;
    let mut kind = base;
    if flags1 & 1 == 0 {
        kind |= 128;
    }
    if ord_x {
        kind |= 64;
    }
    g.s(100, "AcDbDimension");
    g.s(2, block.and_then(|h| names.block(h)).unwrap_or(""));
    for (code, p) in pts.iter().filter(|(c, _)| *c == 10) {
        g.p(*code, *p);
    }
    g.p(11, [mid[0], mid[1], elevation]);
    if matches!(t, 21 | 22) {
        g.p(12, [p12[0], p12[1], elevation]);
    }
    g.i(70, i64::from(kind));
    if !text.is_empty() {
        g.s(1, text);
    }
    if text_rot != 0.0 {
        g.f(53, deg(text_rot));
    }
    if horiz != 0.0 {
        g.f(51, deg(horiz));
    }
    g.i(71, i64::from(attach));
    g.i(72, i64::from(spacing_style));
    g.f(41, spacing);
    g.f(42, measurement);
    g.extrusion(ext);
    if let Some(s) = style.and_then(|h| names.name(h)) {
        g.s(3, s);
    }
    g.s(
        100,
        match t {
            20 => "AcDbOrdinateDimension",
            21 | 22 => "AcDbAlignedDimension",
            23 => "AcDb3PointAngularDimension",
            24 => "AcDb2LineAngularDimension",
            25 => "AcDbRadialDimension",
            _ => "AcDbDiametricDimension",
        },
    );
    for (code, p) in pts.iter().filter(|(c, _)| *c != 10) {
        g.p(*code, *p);
    }
    if let Some(r) = rotation {
        g.f(50, deg(r));
    }
    if let Some(ob) = oblique
        && ob != 0.0
    {
        g.f(52, deg(ob));
    }
    if t == 21 {
        g.s(100, "AcDbRotatedDimension");
    }
    if let Some(l) = leader {
        g.f(40, l);
    }
    Ok(())
}

/// LEADER (spec §20.4.45).
fn leader(o: &mut Obj, names: &Names, g: &mut G) -> Result<()> {
    o.d.b()?;
    let annot = o.d.bs()?;
    let path = o.d.bs()?;
    let n = count(o.d.bl()?, "leader vertices")?;
    let pts: Vec<[f64; 3]> = (0..n).map(|_| o.d.bd3()).collect::<Result<_>>()?;
    let _origin = o.d.bd3()?;
    let ext = o.d.bd3()?;
    let xdir = o.d.bd3()?;
    let offset_block = o.d.bd3()?;
    let offset_annot = o.d.bd3()?;
    // The annotation box's size up to R2007; R2010+ files written by another program don't have
    // it (observed: the bits after the annotation offset read as the flags and the three trailing
    // fields only). To be checked against AutoCAD's own files.
    let (box_h, box_w) = if o.at_least(Version::R2010) { (None, None) } else { (Some(o.d.bd()?), Some(o.d.bd()?)) };
    let hook_on_x = o.d.b()?;
    let arrow = o.d.b()?;
    o.d.bs()?;
    o.d.b()?;
    o.d.b()?;
    let annotation = o.href()?;
    let style = o.href()?;
    g.s(100, "AcDbLeader");
    if let Some(s) = style.and_then(|h| names.name(h)) {
        g.s(3, s);
    }
    g.i(71, i64::from(arrow));
    g.i(72, i64::from(path));
    g.i(73, i64::from(annot));
    g.i(74, i64::from(hook_on_x));
    g.i(75, i64::from(annotation.is_some()));
    if let Some(h) = box_h {
        g.f(40, h);
    }
    if let Some(w) = box_w {
        g.f(41, w);
    }
    g.i(76, n as i64);
    for p in pts {
        g.p(10, p);
    }
    if let Some(a) = annotation {
        g.h(340, a);
    }
    g.extrusion(ext);
    g.p(211, xdir);
    g.p(212, offset_block);
    g.p(213, offset_annot);
    Ok(())
}

/// MLINE (spec §20.4.47).
fn mline(o: &mut Obj, names: &Names, g: &mut G) -> Result<()> {
    let scale = o.d.bd()?;
    let just = o.d.rc()?;
    let base = o.d.bd3()?;
    let ext = o.d.bd3()?;
    let open_closed = o.d.bs()?;
    let lines = usize::from(o.d.rc()?);
    let nverts = count(i32::from(o.d.bs()?), "multiline vertices")?;
    if nverts.saturating_mul(lines.max(1)) > MAX_ITEMS {
        return Err(Error::TooLarge("a multiline"));
    }
    type Vertex = ([f64; 3], [f64; 3], [f64; 3], Vec<(Vec<f64>, Vec<f64>)>);
    let mut verts: Vec<Vertex> = Vec::with_capacity(nverts.min(1 << 12));
    for _ in 0..nverts {
        let p = o.d.bd3()?;
        let dir = o.d.bd3()?;
        let miter = o.d.bd3()?;
        let mut elems = Vec::with_capacity(lines);
        for _ in 0..lines {
            let n = count(i32::from(o.d.bs()?), "multiline parameters")?;
            let params: Vec<f64> = (0..n).map(|_| o.d.bd()).collect::<Result<_>>()?;
            let nf = count(i32::from(o.d.bs()?), "multiline fill parameters")?;
            let fills: Vec<f64> = (0..nf).map(|_| o.d.bd()).collect::<Result<_>>()?;
            elems.push((params, fills));
        }
        verts.push((p, dir, miter, elems));
    }
    let style = o.href()?;
    g.s(100, "AcDbMline");
    g.s(2, style.and_then(|h| names.name(h)).unwrap_or("STANDARD"));
    if let Some(h) = style {
        g.h(340, h);
    }
    g.f(40, scale);
    g.i(70, i64::from(just));
    g.i(71, i64::from(open_closed));
    g.i(72, nverts as i64);
    g.i(73, lines as i64);
    g.p(10, base);
    g.p(210, ext);
    for (p, dir, miter, elems) in verts {
        g.p(11, p);
        g.p(12, dir);
        g.p(13, miter);
        for (params, fills) in elems {
            g.i(74, params.len() as i64);
            for x in params {
                g.f(41, x);
            }
            g.i(75, fills.len() as i64);
            for x in fills {
                g.f(42, x);
            }
        }
    }
    Ok(())
}

/// IMAGE and WIPEOUT (spec §20.4.?? IMAGE, WIPEOUT: the same data).
fn image(name: &str, o: &mut Obj, g: &mut G) -> Result<()> {
    let version = o.d.bl()?;
    let at = o.d.bd3()?;
    let u = o.d.bd3()?;
    let v = o.d.bd3()?;
    let size = o.d.rd2()?;
    let props = o.d.bs()?;
    let clipping = o.d.b()?;
    let brightness = o.d.rc()?;
    let contrast = o.d.rc()?;
    let fade = o.d.rc()?;
    let clip_mode = if o.at_least(Version::R2010) { Some(o.d.b()?) } else { None };
    let clip_type = o.d.bs()?;
    let clip: Vec<[f64; 2]> = if clip_type == 1 {
        vec![o.d.rd2()?, o.d.rd2()?]
    } else {
        let n = count(o.d.bl()?, "clip vertices")?;
        (0..n).map(|_| o.d.rd2()).collect::<Result<_>>()?
    };
    let def = o.href()?;
    let reactor = o.href()?;
    g.s(100, "AcDbRasterImage");
    g.i(90, i64::from(version));
    g.p(10, at);
    g.p(11, u);
    g.p(12, v);
    g.p2(13, size);
    if let Some(h) = def {
        g.h(340, h);
    }
    g.i(70, i64::from(props));
    g.i(280, i64::from(clipping));
    g.i(281, i64::from(brightness));
    g.i(282, i64::from(contrast));
    g.i(283, i64::from(fade));
    if let Some(h) = reactor {
        g.h(360, h);
    }
    g.i(71, i64::from(clip_type));
    g.i(91, clip.len() as i64);
    for p in clip {
        g.p2(14, p);
    }
    if let Some(m) = clip_mode {
        g.i(290, i64::from(m));
    }
    if name == "WIPEOUT" {
        g.s(100, "AcDbWipeout");
    }
    Ok(())
}

/// VIEWPORT (spec §20.4.34).
fn viewport(o: &mut Obj, g: &mut G) -> Result<()> {
    let center = o.d.bd3()?;
    let width = o.d.bd()?;
    let height = o.d.bd()?;
    let target = o.d.bd3()?;
    let direction = o.d.bd3()?;
    let twist = o.d.bd()?;
    let view_height = o.d.bd()?;
    let lens = o.d.bd()?;
    let front = o.d.bd()?;
    let back = o.d.bd()?;
    let snap_angle = o.d.bd()?;
    let view_center = o.d.rd2()?;
    let snap_base = o.d.rd2()?;
    let snap_spacing = o.d.rd2()?;
    let grid_spacing = o.d.rd2()?;
    let circle_zoom = o.d.bs()?;
    let grid_major = if o.at_least(Version::R2007) { Some(o.d.bs()?) } else { None };
    let frozen = count(o.d.bl()?, "frozen layers")?;
    let status = o.d.bl()?;
    let style_sheet = o.text()?;
    let render_mode = o.d.rc()?;
    let ucs_at_origin = o.d.b()?;
    let ucs_per_vp = o.d.b()?;
    let ucs_origin = o.d.bd3()?;
    let ucs_x = o.d.bd3()?;
    let ucs_y = o.d.bd3()?;
    let elevation = o.d.bd()?;
    let ortho = o.d.bs()?;
    let shadeplot = if o.at_least(Version::R2004) { Some(o.d.bs()?) } else { None };
    let mut lights = None;
    if o.at_least(Version::R2007) {
        let default_lights = o.d.b()?;
        let lighting = o.d.rc()?;
        let brightness = o.d.bd()?;
        let contrast = o.d.bd()?;
        let ambient = o.d.bs()?;
        // The ambient colour's true colour and names (R2004+ CMC).
        let _rgb = o.d.bl()?;
        let cflags = o.d.rc()?;
        if cflags & 1 != 0 {
            o.text()?;
        }
        if cflags & 2 != 0 {
            o.text()?;
        }
        lights = Some((default_lights, lighting, brightness, contrast, ambient));
    }
    let mut frozen_handles = Vec::new();
    for _ in 0..frozen {
        if let Some(h) = o.href()? {
            frozen_handles.push(h);
        }
    }
    let clip = o.href()?;
    g.s(100, "AcDbViewport");
    g.p(10, center);
    g.f(40, width);
    g.f(41, height);
    g.p2(12, view_center);
    g.p2(13, snap_base);
    g.p2(14, snap_spacing);
    g.p2(15, grid_spacing);
    g.p(16, direction);
    g.p(17, target);
    g.f(42, lens);
    g.f(43, front);
    g.f(44, back);
    g.f(45, view_height);
    g.f(50, deg(snap_angle));
    g.f(51, deg(twist));
    g.i(72, i64::from(circle_zoom));
    for h in frozen_handles {
        g.h(331, h);
    }
    g.i(90, i64::from(status));
    if let Some(h) = clip {
        g.h(340, h);
    }
    if !style_sheet.is_empty() {
        g.s(1, style_sheet);
    }
    g.i(281, i64::from(render_mode));
    g.i(71, i64::from(ucs_per_vp));
    g.i(74, i64::from(ucs_at_origin));
    g.p(110, ucs_origin);
    g.p(111, ucs_x);
    g.p(112, ucs_y);
    g.i(79, i64::from(ortho));
    g.f(146, elevation);
    if let Some(s) = shadeplot {
        g.i(170, i64::from(s));
    }
    if let Some(m) = grid_major {
        g.i(61, i64::from(m));
    }
    if let Some((dl, lt, b, c, amb)) = lights {
        g.i(292, i64::from(dl));
        g.i(282, i64::from(lt));
        g.f(141, b);
        g.f(142, c);
        g.i(63, i64::from(amb));
    }
    Ok(())
}

/// A colour of a non-entity kind (spec §2.11 CMC): the index, and from R2004 the raw colour value
/// (method byte and RGB, as DXF writes it in a multileader's colour groups) and names.
pub(crate) fn cmc(o: &mut Obj) -> Result<i64> {
    let index = o.d.bs()?;
    if !o.at_least(Version::R2004) {
        return Ok(i64::from(index));
    }
    let raw = o.d.bl()?;
    let flags = o.d.rc()?;
    if flags & 1 != 0 {
        o.text()?;
    }
    if flags & 2 != 0 {
        o.text()?;
    }
    Ok(i64::from(raw))
}

/// MULTILEADER (spec §20.4.86): the context data (leader roots and their lines, the text or
/// block content) in DXF's CONTEXT_DATA{ LEADER{ LEADER_LINE{ } } } blocks, then the entity's
/// own properties.
fn mleader(o: &mut Obj, g: &mut G) -> Result<()> {
    let r2010 = o.at_least(Version::R2010);
    let version = if r2010 { o.d.bs()? } else { 2 };
    g.s(100, "AcDbMLeader");
    // The version, stored from R2010.
    if r2010 {
        g.i(270, i64::from(version));
    }
    g.s(300, "CONTEXT_DATA{");
    // Leader roots: decoded first (they come first in the data), written after the content.
    let nroots = count(o.d.bl()?, "leader roots")?;
    let mut roots = Vec::new();
    for _ in 0..nroots.min(1 << 12) {
        let mut r = Vec::new();
        let mut rg = G(&mut r);
        let valid = o.d.b()?;
        let unknown = o.d.b()?;
        let connection = o.d.bd3()?;
        let direction = o.d.bd3()?;
        rg.s(302, "LEADER{");
        rg.i(290, i64::from(valid));
        rg.i(291, i64::from(unknown));
        rg.p(10, connection);
        rg.p(11, direction);
        let nbreaks = count(o.d.bl()?, "leader breaks")?;
        for _ in 0..nbreaks {
            rg.p(12, o.d.bd3()?);
            rg.p(13, o.d.bd3()?);
        }
        let index = o.d.bl()?;
        let landing = o.d.bd()?;
        rg.i(90, i64::from(index));
        rg.f(40, landing);
        let nlines = count(o.d.bl()?, "leader lines")?;
        for _ in 0..nlines {
            rg.s(304, "LEADER_LINE{");
            let npts = count(o.d.bl()?, "leader line points")?;
            for _ in 0..npts {
                rg.p(10, o.d.bd3()?);
            }
            let nbreak = count(o.d.bl()?, "leader line breaks")?;
            if nbreak > 0 {
                let seg = o.d.bl()?;
                rg.i(90, i64::from(seg));
                let pairs = count(o.d.bl()?, "break pairs")?;
                for _ in 0..pairs {
                    rg.p(11, o.d.bd3()?);
                    rg.p(12, o.d.bd3()?);
                }
            }
            let line_index = o.d.bl()?;
            rg.i(91, i64::from(line_index));
            if r2010 {
                let kind = o.d.bs()?;
                let color = cmc(o)?;
                let ltype = o.href()?;
                let weight = o.d.bl()?;
                let arrow_size = o.d.bd()?;
                let arrow = o.href()?;
                let flags = o.d.bl()?;
                rg.i(170, i64::from(kind));
                rg.i(92, color);
                rg.h(340, ltype.unwrap_or(0));
                rg.i(171, i64::from(weight));
                rg.f(40, arrow_size);
                rg.h(341, arrow.unwrap_or(0));
                rg.i(93, i64::from(flags));
            }
            rg.s(305, "}");
        }
        if r2010 {
            rg.i(271, i64::from(o.d.bs()?));
        }
        rg.s(303, "}");
        roots.push(r);
    }
    g.f(40, o.d.bd()?);
    g.p(10, o.d.bd3()?);
    g.f(41, o.d.bd()?);
    g.f(140, o.d.bd()?);
    g.f(145, o.d.bd()?);
    g.i(174, i64::from(o.d.bs()?));
    g.i(175, i64::from(o.d.bs()?));
    g.i(176, i64::from(o.d.bs()?));
    g.i(177, i64::from(o.d.bs()?));
    let has_text = o.d.b()?;
    g.i(290, i64::from(has_text));
    if has_text {
        let label = o.text()?;
        g.s(304, label);
        g.p(11, o.d.bd3()?);
        if let Some(h) = o.href()? {
            g.h(340, h);
        }
        g.p(12, o.d.bd3()?);
        g.p(13, o.d.bd3()?);
        g.f(42, o.d.bd()?);
        g.f(43, o.d.bd()?);
        g.f(44, o.d.bd()?);
        g.f(45, o.d.bd()?);
        g.i(170, i64::from(o.d.bs()?));
        g.i(90, cmc(o)?);
        g.i(171, i64::from(o.d.bs()?));
        g.i(172, i64::from(o.d.bs()?));
        g.i(91, cmc(o)?);
        g.f(141, o.d.bd()?);
        g.i(92, i64::from(o.d.bl()?));
        g.i(291, i64::from(o.d.b()?));
        g.i(292, i64::from(o.d.b()?));
        g.i(173, i64::from(o.d.bs()?));
        g.i(293, i64::from(o.d.b()?));
        g.f(142, o.d.bd()?);
        g.f(143, o.d.bd()?);
        g.i(294, i64::from(o.d.b()?));
        let ncols = count(o.d.bl()?, "column heights")?;
        for _ in 0..ncols {
            g.f(144, o.d.bd()?);
        }
        g.i(295, i64::from(o.d.b()?));
    }
    // Block content: its flag follows the text content (or its absence).
    let has_block = o.d.b()?;
    g.i(296, i64::from(has_block));
    {
        if has_block {
            if let Some(h) = o.href()? {
                g.h(341, h);
            }
            g.p(14, o.d.bd3()?);
            g.p(15, o.d.bd3()?);
            g.p(16, o.d.bd3()?);
            g.f(46, o.d.bd()?);
            g.i(93, cmc(o)?);
            for _ in 0..16 {
                g.f(47, o.d.bd()?);
            }
        }
    }
    g.p(110, o.d.bd3()?);
    g.p(111, o.d.bd3()?);
    g.p(112, o.d.bd3()?);
    g.i(297, i64::from(o.d.b()?));
    for r in roots {
        g.0.extend(r);
    }
    if r2010 {
        let top = o.d.bs()?;
        let bottom = o.d.bs()?;
        g.i(272, i64::from(bottom));
        g.i(273, i64::from(top));
    }
    g.s(301, "}");
    // The entity's own properties.
    if let Some(h) = o.href()? {
        g.h(340, h);
    }
    g.i(90, i64::from(o.d.bl()?));
    g.i(170, i64::from(o.d.bs()?));
    g.i(91, cmc(o)?);
    if let Some(h) = o.href()? {
        g.h(341, h);
    }
    g.i(171, i64::from(o.d.bl()?));
    g.i(290, i64::from(o.d.b()?));
    g.i(291, i64::from(o.d.b()?));
    g.f(41, o.d.bd()?);
    if let Some(h) = o.href()? {
        g.h(342, h);
    }
    g.f(42, o.d.bd()?);
    g.i(172, i64::from(o.d.bs()?));
    if let Some(h) = o.href()? {
        g.h(343, h);
    }
    g.i(173, i64::from(o.d.bs()?));
    g.i(95, i64::from(o.d.bs()?));
    g.i(174, i64::from(o.d.bs()?));
    g.i(175, i64::from(o.d.bs()?));
    g.i(92, cmc(o)?);
    g.i(292, i64::from(o.d.b()?));
    if let Some(h) = o.href()? {
        g.h(344, h);
    }
    g.i(93, cmc(o)?);
    g.p(10, o.d.bd3()?);
    g.f(43, o.d.bd()?);
    // DXF writes the scale factor (stored near the end) after the block rotation.
    let scale_at = g.0.len();
    g.i(176, i64::from(o.d.bs()?));
    g.i(293, i64::from(o.d.b()?));
    if !r2010 {
        let n = count(o.d.bl()?, "arrowheads")?;
        for _ in 0..n {
            g.i(94, i64::from(o.d.b()?));
            if let Some(h) = o.href()? {
                g.h(345, h);
            }
        }
    }
    let nlabels = count(o.d.bl()?, "block labels")?;
    for _ in 0..nlabels {
        let attdef = o.href()?;
        let text = o.text()?;
        let index = o.d.bs()?;
        let width = o.d.bd()?;
        if let Some(h) = attdef {
            g.h(330, h);
        }
        g.i(177, i64::from(index));
        g.f(44, width);
        g.s(302, text);
    }
    g.i(294, i64::from(o.d.b()?));
    g.i(178, i64::from(o.d.bs()?));
    g.i(179, i64::from(o.d.bs()?));
    let scale = o.d.bd()?;
    g.0.insert(scale_at, Group { code: 45, value: Value::Real(scale) });
    if r2010 {
        g.i(271, i64::from(o.d.bs()?));
        let top = o.d.bs()?;
        let bottom = o.d.bs()?;
        g.i(272, i64::from(bottom));
        g.i(273, i64::from(top));
    }
    if o.at_least(Version::R2013) {
        g.i(295, i64::from(o.d.b()?));
    }
    Ok(())
}
