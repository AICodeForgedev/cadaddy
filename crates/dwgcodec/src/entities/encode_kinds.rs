//! Encoders by record type: each the inverse of its decoder in [`super::kinds`] and
//! [`super::records`] (the same fields in the same order, from the DXF groups).

use super::encode::{EntityHead, Handles, ObjWriter, Rec, cmc, entity_common, object_common, table_entry};
use crate::Version;
use crate::objects::RawObject;

/// Object type numbers (spec §20.3).
pub mod types {
    pub const TEXT: u16 = 1;
    pub const ATTRIB: u16 = 2;
    pub const ATTDEF: u16 = 3;
    pub const BLOCK: u16 = 4;
    pub const ENDBLK: u16 = 5;
    pub const SEQEND: u16 = 6;
    pub const INSERT: u16 = 7;
    pub const MINSERT: u16 = 8;
    pub const VERTEX_2D: u16 = 10;
    pub const VERTEX_3D: u16 = 11;
    pub const VERTEX_MESH: u16 = 12;
    pub const VERTEX_PFACE: u16 = 13;
    pub const VERTEX_PFACE_FACE: u16 = 14;
    pub const POLYLINE_2D: u16 = 15;
    pub const POLYLINE_3D: u16 = 16;
    pub const ARC: u16 = 17;
    pub const CIRCLE: u16 = 18;
    pub const LINE: u16 = 19;
    pub const DIMENSION_ORDINATE: u16 = 20;
    pub const DIMENSION_LINEAR: u16 = 21;
    pub const DIMENSION_ALIGNED: u16 = 22;
    pub const DIMENSION_ANG3PT: u16 = 23;
    pub const DIMENSION_ANG2LN: u16 = 24;
    pub const DIMENSION_RADIUS: u16 = 25;
    pub const DIMENSION_DIAMETER: u16 = 26;
    pub const POINT: u16 = 27;
    pub const FACE3D: u16 = 28;
    pub const POLYLINE_PFACE: u16 = 29;
    pub const POLYLINE_MESH: u16 = 30;
    pub const SOLID: u16 = 31;
    pub const TRACE: u16 = 32;
    pub const VIEWPORT: u16 = 34;
    pub const ELLIPSE: u16 = 35;
    pub const SPLINE: u16 = 36;
    pub const RAY: u16 = 40;
    pub const XLINE: u16 = 41;
    pub const DICTIONARY: u16 = 42;
    pub const MTEXT: u16 = 44;
    pub const LEADER: u16 = 45;
    pub const TOLERANCE: u16 = 46;
    pub const MLINE: u16 = 47;
    pub const BLOCK_CONTROL: u16 = 48;
    pub const BLOCK_HEADER: u16 = 49;
    pub const LAYER_CONTROL: u16 = 50;
    pub const LAYER: u16 = 51;
    pub const STYLE_CONTROL: u16 = 52;
    pub const STYLE: u16 = 53;
    pub const LTYPE_CONTROL: u16 = 56;
    pub const LTYPE: u16 = 57;
    pub const VIEW_CONTROL: u16 = 60;
    pub const UCS_CONTROL: u16 = 62;
    pub const VPORT_CONTROL: u16 = 64;
    pub const APPID_CONTROL: u16 = 66;
    pub const APPID: u16 = 67;
    pub const DIMSTYLE_CONTROL: u16 = 68;
    pub const DIMSTYLE: u16 = 69;
    pub const VX_CONTROL: u16 = 70;
    pub const VIEW: u16 = 61;
    pub const UCS: u16 = 63;
    pub const VPORT: u16 = 65;
    pub const GROUP: u16 = 72;
    pub const MLINESTYLE: u16 = 73;
    pub const LWPOLYLINE: u16 = 77;
    pub const HATCH: u16 = 78;
    pub const XRECORD: u16 = 79;
    pub const PLACEHOLDER: u16 = 80;
    pub const LAYOUT: u16 = 82;
}

fn rad(deg: f64) -> f64 {
    deg.to_radians()
}

/// An entity's record, or `None` for an entity type not encoded yet.
/// `kids` are the record's attributes or vertices (written after it, owned by it) and `seqend`
/// their SEQEND.
pub fn entity(
    v: Version,
    name: &str,
    handle: u64,
    r: &Rec,
    head: &EntityHead,
    names: &Handles,
    kids: &[u64],
    seqend: Option<u64>,
) -> Option<RawObject> {
    let t = match name {
        "LINE" => types::LINE,
        "CIRCLE" => types::CIRCLE,
        "ARC" => types::ARC,
        "POINT" => types::POINT,
        "TEXT" => types::TEXT,
        "LWPOLYLINE" => types::LWPOLYLINE,
        "INSERT" => types::INSERT,
        "BLOCK" => types::BLOCK,
        "ENDBLK" => types::ENDBLK,
        "SEQEND" => types::SEQEND,
        "ATTRIB" => types::ATTRIB,
        "ATTDEF" => types::ATTDEF,
        "MINSERT" => types::MINSERT,
        "ELLIPSE" => types::ELLIPSE,
        "SOLID" => types::SOLID,
        "TRACE" => types::TRACE,
        "3DFACE" => types::FACE3D,
        "RAY" => types::RAY,
        "XLINE" => types::XLINE,
        "TOLERANCE" => types::TOLERANCE,
        "SPLINE" => types::SPLINE,
        "MTEXT" => types::MTEXT,
        "VERTEX" => vertex_type(r),
        "POLYLINE" => polyline_type(r),
        "HATCH" => types::HATCH,
        "LEADER" => types::LEADER,
        "VIEWPORT" => types::VIEWPORT,
        "MULTILEADER" => names.class("MULTILEADER")?,
        "MLINE" => types::MLINE,
        "IMAGE" | "WIPEOUT" => names.class(name)?,
        "DIMENSION" => match r.id(70, 0) & 7 {
            0 => types::DIMENSION_LINEAR,
            1 => types::DIMENSION_ALIGNED,
            2 => types::DIMENSION_ANG2LN,
            3 => types::DIMENSION_DIAMETER,
            4 => types::DIMENSION_RADIUS,
            5 => types::DIMENSION_ANG3PT,
            _ => types::DIMENSION_ORDINATE,
        },
        _ => return None,
    };
    let mut w = ObjWriter::new(v, t, handle);
    entity_common(&mut w, r, head, names);
    match name {
        "LINE" => {
            let (a, b) = (r.p(10), r.p(11));
            let z_zero = a[2] == 0.0 && b[2] == 0.0;
            w.d.b(z_zero);
            w.d.rd(a[0]);
            w.d.dd(b[0], a[0]);
            w.d.rd(a[1]);
            w.d.dd(b[1], a[1]);
            if !z_zero {
                w.d.rd(a[2]);
                w.d.dd(b[2], a[2]);
            }
            w.d.bt(r.fd(39, 0.0), true);
            w.d.be(extrusion(r), true);
        }
        "CIRCLE" | "ARC" => {
            w.d.bd3(r.p(10));
            w.d.bd(r.fd(40, 1.0));
            w.d.bt(r.fd(39, 0.0), true);
            w.d.be(extrusion(r), true);
            if name == "ARC" {
                w.d.bd(rad(r.fd(50, 0.0)));
                w.d.bd(rad(r.fd(51, 360.0)));
            }
        }
        "POINT" => {
            w.d.bd3(r.p(10));
            w.d.bt(r.fd(39, 0.0), true);
            w.d.be(extrusion(r), true);
            w.d.bd(rad(r.fd(50, 0.0)));
        }
        "TEXT" => {
            text_data(&mut w, r);
            w.href(5, names.get("STYLE", r.s(7).unwrap_or("Standard")));
        }
        "LWPOLYLINE" => lwpolyline(&mut w, r),
        "INSERT" | "MINSERT" => {
            w.d.bd3(r.p(10));
            let s = [r.fd(41, 1.0), r.fd(42, 1.0), r.fd(43, 1.0)];
            if s == [1.0, 1.0, 1.0] {
                w.d.bb(3);
            } else if s[0] == s[1] && s[1] == s[2] {
                w.d.bb(2);
                w.d.rd(s[0]);
            } else {
                w.d.bb(0);
                w.d.rd(s[0]);
                w.d.dd(s[1], s[0]);
                w.d.dd(s[2], s[0]);
            }
            w.d.bd(rad(r.fd(50, 0.0)));
            w.d.bd3(extrusion(r));
            let attribs = !kids.is_empty();
            w.d.b(attribs);
            if attribs && w.at_least(Version::R2004) {
                w.d.bl(kids.len() as i32);
            }
            if name == "MINSERT" {
                w.d.bs(r.id(70, 1) as i16);
                w.d.bs(r.id(71, 1) as i16);
                w.d.bd(r.fd(44, 0.0));
                w.d.bd(r.fd(45, 0.0));
            }
            w.href(5, names.get("BLOCK_RECORD", r.s(2).unwrap_or("")));
            if attribs {
                owned(&mut w, kids, seqend);
            }
        }
        "ATTRIB" | "ATTDEF" => {
            text_data_v(&mut w, r, 74);
            if w.at_least(Version::R2010) {
                w.d.rc(0);
            }
            if w.at_least(Version::R2018) {
                // A single-line attribute.
                w.d.rc(1);
            }
            w.text(r.s(2).unwrap_or(""));
            w.d.bs(r.id(73, 0) as i16);
            w.d.rc(r.id(70, 0) as u8);
            if w.at_least(Version::R2007) {
                w.d.b(r.id(280, 0) != 0);
            }
            if name == "ATTDEF" {
                if w.at_least(Version::R2010) {
                    w.d.rc(0);
                }
                w.text(r.s(3).unwrap_or(""));
            }
            w.href(5, names.get("STYLE", r.s(7).unwrap_or("Standard")));
        }
        "ELLIPSE" => {
            w.d.bd3(r.p(10));
            w.d.bd3(r.p(11));
            w.d.bd3(extrusion(r));
            w.d.bd(r.fd(40, 1.0));
            w.d.bd(r.fd(41, 0.0));
            w.d.bd(r.fd(42, std::f64::consts::TAU));
        }
        "SOLID" | "TRACE" => {
            w.d.bt(r.fd(39, 0.0), true);
            w.d.bd(r.fd(30, 0.0));
            let p = [r.p2(10), r.p2(11), r.p2(12), if r.f(13).is_some() { r.p2(13) } else { r.p2(12) }];
            for q in p {
                w.d.rd2(q);
            }
            w.d.be(extrusion(r), true);
        }
        "3DFACE" => {
            let f = r.id(70, 0);
            let a = r.p(10);
            w.d.b(f == 0);
            w.d.b(a[2] == 0.0);
            w.d.rd(a[0]);
            w.d.rd(a[1]);
            if a[2] != 0.0 {
                w.d.rd(a[2]);
            }
            let mut prev = a;
            for code in [11, 12, 13] {
                let p = if r.f(code).is_some() { r.p(code) } else { prev };
                w.d.dd(p[0], prev[0]);
                w.d.dd(p[1], prev[1]);
                w.d.dd(p[2], prev[2]);
                prev = p;
            }
            if f != 0 {
                w.d.bs(f as i16);
            }
        }
        "RAY" | "XLINE" => {
            w.d.bd3(r.p(10));
            w.d.bd3(r.p(11));
        }
        "TOLERANCE" => {
            w.d.bd3(r.p(10));
            w.d.bd3(r.p(11));
            w.d.bd3(extrusion(r));
            w.text(r.s(1).unwrap_or(""));
            w.href(5, names.get("DIMSTYLE", r.s(3).unwrap_or("Standard")));
        }
        "SPLINE" => spline(&mut w, r),
        "MTEXT" => mtext(&mut w, r, names),
        "VERTEX" => vertex(&mut w, r),
        "POLYLINE" => {
            polyline(&mut w, r, kids.len());
            owned(&mut w, kids, seqend);
        }
        "HATCH" => hatch(&mut w, r),
        "LEADER" => leader(&mut w, r, names),
        "VIEWPORT" => viewport(&mut w, r),
        "MULTILEADER" => mleader(&mut w, r, names),
        "MLINE" => mline(&mut w, r, names),
        "IMAGE" | "WIPEOUT" => image(&mut w, r),
        "DIMENSION" => dimension(&mut w, r, names, t),
        "BLOCK" => w.text(r.s(2).unwrap_or("")),
        _ => {}
    }
    Some(w.finish())
}

fn extrusion(r: &Rec) -> [f64; 3] {
    if r.f(230).is_some() { r.p(210) } else { [0.0, 0.0, 1.0] }
}

/// The text fields TEXT, ATTRIB and ATTDEF share (inverse of the decoder's `text_data`).
fn text_data(w: &mut ObjWriter, r: &Rec) {
    text_data_v(w, r, 73);
}

/// [`text_data`] with the vertical alignment from `valign_code` (73 for TEXT, 74 for attributes).
fn text_data_v(w: &mut ObjWriter, r: &Rec, valign_code: i32) {
    let at = r.p(10);
    let align = r.f(11).map(|_| r.p(11));
    let (oblique, rotation, width) = (r.fd(51, 0.0), r.fd(50, 0.0), r.fd(41, 1.0));
    let (generation, halign, valign) = (r.id(71, 0), r.id(72, 0), r.id(valign_code, 0));
    let mut flags = 0u8;
    if at[2] == 0.0 {
        flags |= 0x01;
    }
    if align.is_none() {
        flags |= 0x02;
    }
    if oblique == 0.0 {
        flags |= 0x04;
    }
    if rotation == 0.0 {
        flags |= 0x08;
    }
    if width == 1.0 {
        flags |= 0x10;
    }
    if generation == 0 {
        flags |= 0x20;
    }
    if halign == 0 {
        flags |= 0x40;
    }
    if valign == 0 {
        flags |= 0x80;
    }
    w.d.rc(flags);
    if flags & 0x01 == 0 {
        w.d.rd(at[2]);
    }
    w.d.rd2([at[0], at[1]]);
    if let Some(a) = align {
        w.d.dd(a[0], at[0]);
        w.d.dd(a[1], at[1]);
    }
    w.d.be(extrusion(r), true);
    w.d.bt(r.fd(39, 0.0), true);
    if flags & 0x04 == 0 {
        w.d.rd(rad(oblique));
    }
    if flags & 0x08 == 0 {
        w.d.rd(rad(rotation));
    }
    w.d.rd(r.fd(40, 1.0));
    if flags & 0x10 == 0 {
        w.d.rd(width);
    }
    w.text(r.s(1).unwrap_or(""));
    if flags & 0x20 == 0 {
        w.d.bs(generation as i16);
    }
    if flags & 0x40 == 0 {
        w.d.bs(halign as i16);
    }
    if flags & 0x80 == 0 {
        w.d.bs(valign as i16);
    }
}

fn lwpolyline(w: &mut ObjWriter, r: &Rec) {
    let xs: Vec<f64> = r.all(10).filter_map(|g| num(&g.value)).collect();
    let ys: Vec<f64> = r.all(20).filter_map(|g| num(&g.value)).collect();
    let n = xs.len().min(ys.len());
    // Per-vertex widths and bulges follow their vertex in the groups.
    let mut bulges = vec![0.0; n];
    let mut widths = vec![(0.0, 0.0); n];
    let mut vi: isize = -1;
    for g in r.0 {
        match g.code {
            10 => vi += 1,
            40..=42 if vi >= 0 && (vi as usize) < n => {
                let x = num(&g.value).unwrap_or(0.0);
                let i = vi as usize;
                match g.code {
                    40 => widths[i].0 = x,
                    41 => widths[i].1 = x,
                    _ => bulges[i] = x,
                }
            }
            _ => {}
        }
    }
    let has_bulges = bulges.iter().any(|b| *b != 0.0);
    let has_widths = widths.iter().any(|w| w.0 != 0.0 || w.1 != 0.0);
    let const_width = r.f(43);
    let elevation = r.fd(38, 0.0);
    let thickness = r.fd(39, 0.0);
    let ext = extrusion(r);
    let mut flags = 0i16;
    if ext != [0.0, 0.0, 1.0] {
        flags |= 1;
    }
    if thickness != 0.0 {
        flags |= 2;
    }
    if const_width.is_some() {
        flags |= 4;
    }
    if elevation != 0.0 {
        flags |= 8;
    }
    if has_bulges {
        flags |= 16;
    }
    if has_widths {
        flags |= 32;
    }
    let f70 = r.id(70, 0);
    if f70 & 128 != 0 {
        flags |= 256;
    }
    if f70 & 1 != 0 {
        flags |= 512;
    }
    w.d.bs(flags);
    if let Some(cw) = const_width {
        w.d.bd(cw);
    }
    if flags & 8 != 0 {
        w.d.bd(elevation);
    }
    if flags & 2 != 0 {
        w.d.bd(thickness);
    }
    if flags & 1 != 0 {
        w.d.bd3(ext);
    }
    w.d.bl(n as i32);
    if has_bulges {
        w.d.bl(n as i32);
    }
    if has_widths {
        w.d.bl(n as i32);
    }
    let mut prev: Option<[f64; 2]> = None;
    for i in 0..n {
        let p = [xs[i], ys[i]];
        match prev {
            Some(q) => {
                w.d.dd(p[0], q[0]);
                w.d.dd(p[1], q[1]);
            }
            None => w.d.rd2(p),
        }
        prev = Some(p);
    }
    if has_bulges {
        for b in &bulges {
            w.d.bd(*b);
        }
    }
    if has_widths {
        for (s, e) in &widths {
            w.d.bd(*s);
            w.d.bd(*e);
        }
    }
}

/// The handles of owned attributes or vertices: from R2004 each one, before that the first and
/// last; then their SEQEND.
fn owned(w: &mut ObjWriter, kids: &[u64], seqend: Option<u64>) {
    if w.at_least(Version::R2004) {
        for k in kids {
            w.href(3, Some(*k));
        }
    } else {
        w.href(4, kids.first().copied());
        w.href(4, kids.last().copied());
    }
    w.href(3, seqend);
}

/// A VERTEX's object type, from its flags (DXF 70).
fn vertex_type(r: &Rec) -> u16 {
    let f = r.id(70, 0);
    if f & 128 != 0 && f & 64 == 0 {
        types::VERTEX_PFACE_FACE
    } else if f & 128 != 0 {
        types::VERTEX_PFACE
    } else if f & 64 != 0 {
        types::VERTEX_MESH
    } else if f & 32 != 0 {
        types::VERTEX_3D
    } else {
        types::VERTEX_2D
    }
}

/// A POLYLINE's object type, from its flags (DXF 70).
fn polyline_type(r: &Rec) -> u16 {
    let f = r.id(70, 0);
    if f & 64 != 0 {
        types::POLYLINE_PFACE
    } else if f & 16 != 0 {
        types::POLYLINE_MESH
    } else if f & 8 != 0 {
        types::POLYLINE_3D
    } else {
        types::POLYLINE_2D
    }
}

fn vertex(w: &mut ObjWriter, r: &Rec) {
    let f = r.id(70, 0);
    match vertex_type(r) {
        types::VERTEX_2D => {
            w.d.rc(f as u8);
            w.d.bd3(r.p(10));
            let (s, e) = (r.fd(40, 0.0), r.fd(41, 0.0));
            if s == e && s != 0.0 {
                w.d.bd(-s);
            } else {
                w.d.bd(s);
                w.d.bd(e);
            }
            w.d.bd(r.fd(42, 0.0));
            if w.at_least(Version::R2010) {
                w.d.bl(0);
            }
            w.d.bd(rad(r.fd(50, 0.0)));
        }
        types::VERTEX_PFACE_FACE => {
            for code in 71..=74 {
                w.d.bs(r.id(code, 0) as i16);
            }
        }
        t => {
            let flags = if t == types::VERTEX_PFACE { f & !(128 | 64) } else { f };
            w.d.rc(flags as u8);
            w.d.bd3(r.p(10));
        }
    }
}

fn polyline(w: &mut ObjWriter, r: &Rec, n: usize) {
    let f = r.id(70, 0);
    let count = |w: &mut ObjWriter| {
        if w.at_least(Version::R2004) {
            w.d.bl(n as i32);
        }
    };
    match polyline_type(r) {
        types::POLYLINE_2D => {
            w.d.bs(f as i16);
            w.d.bs(r.id(75, 0) as i16);
            w.d.bd(r.fd(40, 0.0));
            w.d.bd(r.fd(41, 0.0));
            w.d.bt(r.fd(39, 0.0), true);
            w.d.bd(r.fd(30, 0.0));
            w.d.be(extrusion(r), true);
            count(w);
        }
        types::POLYLINE_3D => {
            let mut spline = 0u8;
            if f & 4 != 0 {
                spline |= 4;
                spline |= match r.id(75, 0) {
                    5 => 1,
                    6 => 2,
                    _ => 0,
                };
            }
            w.d.rc(spline);
            w.d.rc((f & 1) as u8);
            count(w);
        }
        types::POLYLINE_PFACE => {
            w.d.bs(r.id(71, 0) as i16);
            w.d.bs(r.id(72, 0) as i16);
            count(w);
        }
        _ => {
            w.d.bs((f & !16) as i16);
            w.d.bs(r.id(75, 0) as i16);
            for code in 71..=74 {
                w.d.bs(r.id(code, 0) as i16);
            }
            count(w);
        }
    }
}

fn spline(w: &mut ObjWriter, r: &Rec) {
    let flags = r.id(70, 0);
    let fits: Vec<[f64; 3]> = points(r, 11);
    let ctrls: Vec<[f64; 3]> = points(r, 10);
    let knots: Vec<f64> = r.all(40).filter_map(|g| num(&g.value)).collect();
    let weights: Vec<f64> = r.all(41).filter_map(|g| num(&g.value)).collect();
    // Fit points when there are any (DWG stores one or the other: the curve follows from them).
    let by_fit = !fits.is_empty();
    let scenario = if by_fit { 2 } else { 1 };
    w.d.bl(scenario);
    if w.at_least(Version::R2013) {
        w.d.bl(if by_fit { 1 } else { 0 });
        w.d.bl(0);
    }
    w.d.bl(r.id(71, 3) as i32);
    if by_fit {
        w.d.bd(r.fd(44, 0.0));
        w.d.bd3(r.f(12).map(|_| r.p(12)).unwrap_or([0.0; 3]));
        w.d.bd3(r.f(13).map(|_| r.p(13)).unwrap_or([0.0; 3]));
        w.d.bl(fits.len() as i32);
        for p in &fits {
            w.d.bd3(*p);
        }
    } else {
        let weighted = !weights.is_empty() && weights.len() == ctrls.len();
        w.d.b(flags & 4 != 0);
        w.d.b(flags & 1 != 0);
        w.d.b(flags & 2 != 0);
        w.d.bd(r.fd(42, 0.0));
        w.d.bd(r.fd(43, 0.0));
        w.d.bl(knots.len() as i32);
        w.d.bl(ctrls.len() as i32);
        w.d.b(weighted);
        for k in &knots {
            w.d.bd(*k);
        }
        for (i, p) in ctrls.iter().enumerate() {
            w.d.bd3(*p);
            if weighted {
                w.d.bd(weights.get(i).copied().unwrap_or(1.0));
            }
        }
    }
}

/// Every point with `code`, `code + 10`, `code + 20` in order.
fn points(r: &Rec, code: i32) -> Vec<[f64; 3]> {
    let mut out: Vec<[f64; 3]> = Vec::new();
    for g in r.0 {
        let x = num(&g.value).unwrap_or(0.0);
        if g.code == code {
            out.push([x, 0.0, 0.0]);
        } else if let Some(p) = out.last_mut() {
            if g.code == code + 10 {
                p[1] = x;
            } else if g.code == code + 20 {
                p[2] = x;
            }
        }
    }
    out
}

fn mtext(w: &mut ObjWriter, r: &Rec, names: &Handles) {
    w.d.bd3(r.p(10));
    w.d.bd3(extrusion(r));
    let xdir = if r.f(11).is_some() {
        r.p(11)
    } else {
        let a = rad(r.fd(50, 0.0));
        [a.cos(), a.sin(), 0.0]
    };
    w.d.bd3(xdir);
    w.d.bd(r.fd(41, 0.0));
    if w.at_least(Version::R2007) {
        w.d.bd(r.fd(46, 0.0));
    }
    w.d.bd(r.fd(40, 1.0));
    w.d.bs(r.id(71, 1) as i16);
    w.d.bs(r.id(72, 1) as i16);
    w.d.bd(r.fd(43, 0.0));
    w.d.bd(r.fd(42, 0.0));
    let mut text = String::new();
    for g in r.all(3) {
        if let super::Value::Str(s) = &g.value {
            text.push_str(s);
        }
    }
    text.push_str(r.s(1).unwrap_or(""));
    w.text(&text);
    w.d.bs(r.id(73, 1) as i16);
    w.d.bd(r.fd(44, 1.0));
    w.d.b(false);
    if w.at_least(Version::R2004) {
        let flags = r.id(90, 0) as i32;
        w.d.bl(flags);
        let has = if w.at_least(Version::R2018) { flags & 0x11 != 0 } else { flags & 1 != 0 };
        if has {
            w.d.bd(r.fd(45, 1.5));
            w.d.bs(r.id(63, 256) as i16);
            let rgb = match r.i(421) {
                Some(c) => 0xC200_0000u32 | (c as u32 & 0xFF_FFFF),
                None => 0xC300_0000u32 | (r.id(63, 256) as u32 & 0xFF),
            };
            w.d.bl(rgb as i32);
            w.d.rc(0);
            w.d.bl(r.id(441, 0) as i32);
        }
    }
    // R2018: not annotative, so the context data follows (version 4, the default flag, the
    // attachment, axis, insertion point, the rectangle's and the extents' sizes, no columns)
    // and its application's handle (null), as the files we've seen have it.
    let r2018 = w.at_least(Version::R2018);
    if r2018 {
        w.d.b(true);
        w.d.bs(4);
        w.d.b(true);
        w.d.bl(r.id(71, 1) as i32);
        w.d.bd3(xdir);
        w.d.bd3(r.p(10));
        w.d.bd(r.fd(41, 0.0));
        w.d.bd(r.fd(46, 0.0));
        w.d.bd(r.fd(42, 0.0));
        w.d.bd(r.fd(43, 0.0));
        w.d.bs(0);
    }
    w.href(5, names.get("STYLE", r.s(7).unwrap_or("Standard")));
    if r2018 {
        w.href(5, None);
    }
}

/// The groups of a record read in order, as the decoders write them.
struct Cursor<'a> {
    gs: &'a [super::Group],
    at: usize,
}

impl Cursor<'_> {
    /// The next group's number when it has `code` (and moves past it), else `None`.
    fn f(&mut self, code: i32) -> Option<f64> {
        let g = self.gs.get(self.at).filter(|g| g.code == code)?;
        self.at += 1;
        num(&g.value)
    }
    fn fd(&mut self, code: i32, default: f64) -> f64 {
        self.f(code).filter(|x| x.is_finite()).unwrap_or(default)
    }
    fn i(&mut self, code: i32) -> i64 {
        self.id(code, 0)
    }
    fn id(&mut self, code: i32, default: i64) -> i64 {
        self.f(code).filter(|x| x.is_finite()).map_or(default, |x| x as i64)
    }
    fn p2(&mut self, code: i32) -> [f64; 2] {
        let x = self.fd(code, 0.0);
        let y = self.fd(code + 10, 0.0);
        // A z, when there is one, is skipped.
        self.f(code + 20);
        [x, y]
    }
    fn s(&mut self, code: i32) -> String {
        match self.gs.get(self.at) {
            Some(g) if g.code == code => {
                self.at += 1;
                match &g.value {
                    super::Value::Str(s) => s.clone(),
                    _ => String::new(),
                }
            }
            _ => String::new(),
        }
    }
    fn h(&mut self, code: i32) -> Option<u64> {
        let g = self.gs.get(self.at).filter(|g| g.code == code)?;
        self.at += 1;
        match &g.value {
            super::Value::Handle(h) => Some(*h),
            super::Value::Str(s) => u64::from_str_radix(s.trim(), 16).ok(),
            super::Value::Int(i) => u64::try_from(*i).ok(),
            super::Value::Real(_) => None,
        }
        .filter(|h| *h != 0)
    }
    /// Moves to the first group with `code` (from here on).
    fn seek(&mut self, code: i32) -> bool {
        match self.gs.iter().skip(self.at).position(|g| g.code == code) {
            Some(k) => {
                self.at += k;
                true
            }
            None => false,
        }
    }
}

/// Upper bound on a count read from the groups (each item needs at least one group).
fn cap(n: i64, c: &Cursor) -> usize {
    usize::try_from(n).unwrap_or(0).min(c.gs.len())
}

fn hatch(w: &mut ObjWriter, r: &Rec) {
    let mut c = Cursor { gs: r.0, at: 0 };
    // Gradient (R2004+), from the 450 groups at the end.
    if w.at_least(Version::R2004) {
        let mut g = Cursor { gs: r.0, at: 0 };
        if g.seek(450) {
            w.d.bl(g.i(450) as i32);
            w.d.bl(g.i(451) as i32);
            let (mut angle, mut shift, mut single, mut tint) = (0.0, 0.0, 0, 0.0);
            let mut colors = Vec::new();
            let mut name = String::new();
            while let Some(x) = g.gs.get(g.at) {
                match x.code {
                    460 => angle = g.fd(460, 0.0),
                    461 => shift = g.fd(461, 0.0),
                    452 => single = g.i(452),
                    462 => tint = g.fd(462, 0.0),
                    463 => {
                        let v = g.fd(463, 0.0);
                        // Its colour: 63 (index) and/or 421 (true colour) after it.
                        let mut rgb = None;
                        while let Some(y) = g.gs.get(g.at) {
                            match y.code {
                                63 => {
                                    g.at += 1;
                                }
                                421 => rgb = Some(g.i(421)),
                                _ => break,
                            }
                        }
                        colors.push((v, rgb.unwrap_or(0)));
                    }
                    470 => name = g.s(470),
                    _ => g.at += 1,
                }
            }
            w.d.bd(angle);
            w.d.bd(shift);
            w.d.bl(single as i32);
            w.d.bd(tint);
            w.d.bl(colors.len() as i32);
            for (v, rgb) in &colors {
                w.d.bd(*v);
                w.d.bs(0);
                w.d.bl((0xC200_0000u32 | (*rgb as u32 & 0xFF_FFFF)) as i32);
                w.d.rc(0);
            }
            w.text(&name);
        } else {
            w.d.bl(0);
            w.d.bl(0);
            w.d.bd(0.0);
            w.d.bd(0.0);
            w.d.bl(0);
            w.d.bd(0.0);
            w.d.bl(0);
            w.text("");
        }
    }
    w.d.bd(r.p(10)[2]);
    w.d.bd3(extrusion(r));
    c.seek(2);
    let pattern = c.s(2);
    w.text(&pattern);
    let solid = r.id(70, 0) != 0;
    w.d.b(solid);
    w.d.b(r.id(71, 0) != 0);
    c.seek(91);
    let npaths = cap(c.i(91), &c);
    w.d.bl(npaths as i32);
    let mut sources: Vec<u64> = Vec::new();
    let mut derived = false;
    for _ in 0..npaths {
        c.seek(92);
        let flags = c.i(92);
        if flags & 4 != 0 {
            derived = true;
        }
        w.d.bl(flags as i32);
        if flags & 2 == 0 {
            let n = cap(c.i(93), &c);
            w.d.bl(n as i32);
            for _ in 0..n {
                let kind = c.i(72);
                w.d.rc(kind as u8);
                match kind {
                    1 => {
                        w.d.rd2(c.p2(10));
                        w.d.rd2(c.p2(11));
                    }
                    2 => {
                        w.d.rd2(c.p2(10));
                        w.d.bd(c.fd(40, 0.0));
                        w.d.bd(rad(c.fd(50, 0.0)));
                        w.d.bd(rad(c.fd(51, 360.0)));
                        w.d.b(c.i(73) != 0);
                    }
                    3 => {
                        w.d.rd2(c.p2(10));
                        w.d.rd2(c.p2(11));
                        w.d.bd(c.fd(40, 1.0));
                        w.d.bd(rad(c.fd(50, 0.0)));
                        w.d.bd(rad(c.fd(51, 360.0)));
                        w.d.b(c.i(73) != 0);
                    }
                    _ => {
                        let degree = c.i(94);
                        let rational = c.i(73) != 0;
                        let periodic = c.i(74) != 0;
                        let nk = cap(c.i(95), &c);
                        let nc = cap(c.i(96), &c);
                        w.d.bl(degree as i32);
                        w.d.b(rational);
                        w.d.b(periodic);
                        w.d.bl(nk as i32);
                        w.d.bl(nc as i32);
                        for _ in 0..nk {
                            w.d.bd(c.fd(40, 0.0));
                        }
                        for _ in 0..nc {
                            w.d.rd2(c.p2(10));
                            if rational {
                                w.d.bd(c.fd(42, 1.0));
                            }
                        }
                        if w.at_least(Version::R2010) {
                            let nf = cap(c.i(97), &c);
                            w.d.bl(nf as i32);
                            if nf > 0 {
                                for _ in 0..nf {
                                    w.d.rd2(c.p2(11));
                                }
                                w.d.rd2(c.p2(12));
                                w.d.rd2(c.p2(13));
                            }
                        }
                    }
                }
            }
        } else {
            let bulges = c.i(72) != 0;
            let closed = c.i(73) != 0;
            let n = cap(c.i(93), &c);
            w.d.b(bulges);
            w.d.b(closed);
            w.d.bl(n as i32);
            for _ in 0..n {
                w.d.rd2(c.p2(10));
                if bulges {
                    w.d.bd(c.fd(42, 0.0));
                }
            }
        }
        c.seek(97);
        let nb = cap(c.i(97), &c);
        w.d.bl(nb as i32);
        for _ in 0..nb {
            sources.push(c.h(330).unwrap_or(0));
        }
    }
    c.seek(75);
    w.d.bs(c.i(75) as i16);
    w.d.bs(c.i(76) as i16);
    if !solid {
        w.d.bd(rad(c.fd(52, 0.0)));
        w.d.bd(c.fd(41, 1.0));
        w.d.b(c.i(77) != 0);
        let nlines = cap(c.i(78), &c);
        w.d.bs(nlines as i16);
        for _ in 0..nlines {
            w.d.bd(rad(c.fd(53, 0.0)));
            w.d.bd(c.fd(43, 0.0));
            w.d.bd(c.fd(44, 0.0));
            w.d.bd(c.fd(45, 0.0));
            w.d.bd(c.fd(46, 0.0));
            let nd = cap(c.i(79), &c);
            w.d.bs(nd as i16);
            for _ in 0..nd {
                w.d.bd(c.fd(49, 0.0));
            }
        }
    }
    if derived {
        w.d.bd(r.fd(47, 0.0));
    }
    let seeds = if c.seek(98) {
        let n = cap(c.i(98), &c);
        (0..n).map(|_| c.p2(10)).collect()
    } else {
        Vec::new()
    };
    w.d.bl(seeds.len() as i32);
    for p in seeds {
        w.d.rd2(p);
    }
    for h in sources {
        w.href(4, (h != 0).then_some(h));
    }
}

fn viewport(w: &mut ObjWriter, r: &Rec) {
    w.d.bd3(r.p(10));
    w.d.bd(r.fd(40, 0.0));
    w.d.bd(r.fd(41, 0.0));
    w.d.bd3(r.p(17));
    w.d.bd3(if r.f(16).is_some() { r.p(16) } else { [0.0, 0.0, 1.0] });
    w.d.bd(rad(r.fd(51, 0.0)));
    w.d.bd(r.fd(45, 1.0));
    w.d.bd(r.fd(42, 50.0));
    w.d.bd(r.fd(43, 0.0));
    w.d.bd(r.fd(44, 0.0));
    w.d.bd(rad(r.fd(50, 0.0)));
    w.d.rd2(r.p2(12));
    w.d.rd2(r.p2(13));
    w.d.rd2(if r.f(14).is_some() { r.p2(14) } else { [10.0, 10.0] });
    w.d.rd2(if r.f(15).is_some() { r.p2(15) } else { [10.0, 10.0] });
    w.d.bs(r.id(72, 1000) as i16);
    if w.at_least(Version::R2007) {
        w.d.bs(r.id(61, 5) as i16);
    }
    let frozen: Vec<u64> = r.all(331).filter_map(|g| handle_of(&g.value)).collect();
    w.d.bl(frozen.len() as i32);
    w.d.bl(r.id(90, 0) as i32);
    w.text(r.s(1).unwrap_or(""));
    w.d.rc(r.id(281, 0) as u8);
    w.d.b(r.id(74, 0) != 0);
    w.d.b(r.id(71, 0) != 0);
    w.d.bd3(r.p(110));
    w.d.bd3(if r.f(111).is_some() { r.p(111) } else { [1.0, 0.0, 0.0] });
    w.d.bd3(if r.f(112).is_some() { r.p(112) } else { [0.0, 1.0, 0.0] });
    w.d.bd(r.fd(146, 0.0));
    w.d.bs(r.id(79, 0) as i16);
    if w.at_least(Version::R2004) {
        w.d.bs(r.id(170, 0) as i16);
    }
    if w.at_least(Version::R2007) {
        lights(w, r);
    }
    for h in frozen {
        w.href(4, Some(h));
    }
    w.href(5, r.h(340).filter(|h| *h != 0));
    // Named and base UCS; R2007+ background, visual style, shade plot and sun.
    w.href(5, None);
    w.href(5, None);
    if w.at_least(Version::R2007) {
        w.href(4, None);
        w.href(5, None);
        w.href(4, None);
        w.href(3, None);
    }
}

fn leader(w: &mut ObjWriter, r: &Rec, names: &Handles) {
    let pts = points(r, 10);
    w.d.b(false);
    w.d.bs(r.id(73, 3) as i16);
    w.d.bs(r.id(72, 0) as i16);
    w.d.bl(pts.len() as i32);
    for p in &pts {
        w.d.bd3(*p);
    }
    w.d.bd3(pts.first().copied().unwrap_or([0.0; 3]));
    w.d.bd3(extrusion(r));
    w.d.bd3(if r.f(211).is_some() { r.p(211) } else { [1.0, 0.0, 0.0] });
    w.d.bd3(r.p(212));
    w.d.bd3(r.p(213));
    if !w.at_least(Version::R2010) {
        w.d.bd(r.fd(40, 0.0));
        w.d.bd(r.fd(41, 0.0));
    }
    w.d.b(r.id(74, 0) != 0);
    w.d.b(r.id(71, 1) != 0);
    w.d.bs(0);
    w.d.b(false);
    w.d.b(false);
    w.href(5, r.h(340).filter(|h| *h != 0));
    w.href(5, names.get("DIMSTYLE", r.s(3).unwrap_or("Standard")));
}

fn dimension(w: &mut ObjWriter, r: &Rec, names: &Handles, t: u16) {
    let kind = r.id(70, 0);
    if w.at_least(Version::R2010) {
        w.d.rc(0);
    }
    w.d.bd3(extrusion(r));
    let mid = r.p(11);
    w.d.rd2([mid[0], mid[1]]);
    w.d.bd(mid[2]);
    w.d.rc(if kind & 128 == 0 { 1 } else { 0 });
    w.text(r.s(1).unwrap_or(""));
    w.d.bd(rad(r.fd(53, 0.0)));
    w.d.bd(rad(r.fd(51, 0.0)));
    w.d.bd3([1.0, 1.0, 1.0]);
    w.d.bd(0.0);
    w.d.bs(r.id(71, 5) as i16);
    w.d.bs(r.id(72, 1) as i16);
    w.d.bd(r.fd(41, 1.0));
    w.d.bd(r.fd(42, 0.0));
    if w.at_least(Version::R2007) {
        w.d.b(false);
        w.d.b(false);
        w.d.b(false);
    }
    w.d.rd2(r.p2(12));
    match t {
        types::DIMENSION_ORDINATE => {
            w.d.bd3(r.p(10));
            w.d.bd3(r.p(13));
            w.d.bd3(r.p(14));
            w.d.rc(if kind & 64 != 0 { 1 } else { 0 });
        }
        types::DIMENSION_LINEAR | types::DIMENSION_ALIGNED => {
            w.d.bd3(r.p(13));
            w.d.bd3(r.p(14));
            w.d.bd3(r.p(10));
            w.d.bd(rad(r.fd(52, 0.0)));
            if t == types::DIMENSION_LINEAR {
                w.d.bd(rad(r.fd(50, 0.0)));
            }
        }
        types::DIMENSION_ANG3PT => {
            w.d.bd3(r.p(10));
            w.d.bd3(r.p(13));
            w.d.bd3(r.p(14));
            w.d.bd3(r.p(15));
        }
        types::DIMENSION_ANG2LN => {
            w.d.rd2(r.p2(16));
            w.d.bd3(r.p(13));
            w.d.bd3(r.p(14));
            w.d.bd3(r.p(15));
            w.d.bd3(r.p(10));
        }
        types::DIMENSION_RADIUS => {
            w.d.bd3(r.p(10));
            w.d.bd3(r.p(15));
            w.d.bd(r.fd(40, 0.0));
        }
        _ => {
            w.d.bd3(r.p(15));
            w.d.bd3(r.p(10));
            w.d.bd(r.fd(40, 0.0));
        }
    }
    w.href(5, names.get("DIMSTYLE", r.s(3).unwrap_or("Standard")));
    w.href(5, names.get("BLOCK_RECORD", r.s(2).unwrap_or("")));
}

/// A ByBlock colour as DXF's raw colour value.
const BYBLOCK: i64 = -1_056_964_608;

/// A colour stored as a multileader stores it (spec §2.11 CMC): DXF gives the raw value (method
/// byte and RGB); the index is the method's.
fn cmc_raw(w: &mut ObjWriter, raw: i64) {
    let raw = raw as u32;
    let index = match raw >> 24 {
        0xC0 => 256,
        0xC1 => 0,
        0xC3 => (raw & 0xFF) as i16,
        _ => 0,
    };
    w.d.bs(index);
    if w.at_least(Version::R2004) {
        w.d.bl(raw as i32);
        w.d.rc(0);
    }
}

/// MULTILEADER (spec §20.4.86) from DXF's groups: the context data (CONTEXT_DATA{ … }, its
/// leader roots LEADER{ … } with their lines LEADER_LINE{ … }) and the entity's properties.
fn mleader(w: &mut ObjWriter, r: &Rec, names: &Handles) {
    let r2010 = w.at_least(Version::R2010);
    let start = r.0.iter().position(|g| g.code == 100 && matches!(&g.value, super::Value::Str(s) if s == "AcDbMLeader")).map_or(0, |i| i + 1);
    let gs = r.0.get(start..).unwrap_or(&[]);
    let mut c = Cursor { gs, at: 0 };
    let version = c.i(270);
    if r2010 {
        w.d.bs(if version == 0 { 2 } else { version as i16 });
    }
    c.seek(300);
    c.at += 1;
    // The context's own groups up to the first root, the roots, then the rest.
    let content_from = c.at;
    let content_to = gs.iter().skip(content_from).position(|g| g.code == 302 || g.code == 301).map_or(gs.len(), |k| content_from + k);
    // Leader roots.
    let mut roots: Vec<&[super::Group]> = Vec::new();
    let mut k = content_to;
    while let Some(g) = gs.get(k) {
        if g.code == 301 {
            break;
        }
        if g.code == 302 {
            let end = gs.iter().skip(k).position(|x| x.code == 303).map_or(gs.len(), |e| k + e);
            roots.push(gs.get(k + 1..end).unwrap_or(&[]));
            k = end + 1;
        } else {
            k += 1;
        }
    }
    let tail_from = gs.iter().skip(k).position(|g| g.code == 301).map_or(gs.len(), |e| k + e);
    // 272/273 between the roots and the context's end.
    let between = gs.get(k..tail_from).unwrap_or(&[]);
    let arrow_size = Rec(gs).fd(42, 0.18);
    w.d.bl(roots.len() as i32);
    for root in &roots {
        let mut rc = Cursor { gs: root, at: 0 };
        w.d.b(rc.i(290) != 0);
        w.d.b(rc.i(291) != 0);
        w.d.bd3(cur_p(&mut rc, 10));
        w.d.bd3(cur_p(&mut rc, 11));
        let mut breaks = Vec::new();
        while rc.gs.get(rc.at).is_some_and(|g| g.code == 12) {
            breaks.push((cur_p(&mut rc, 12), cur_p(&mut rc, 13)));
        }
        w.d.bl(breaks.len() as i32);
        for (a, b) in breaks {
            w.d.bd3(a);
            w.d.bd3(b);
        }
        w.d.bl(rc.i(90) as i32);
        w.d.bd(rc.fd(40, 0.0));
        // Leader lines.
        let mut lines: Vec<&[super::Group]> = Vec::new();
        let mut j = rc.at;
        while let Some(g) = root.get(j) {
            if g.code == 304 {
                let end = root.iter().skip(j).position(|x| x.code == 305).map_or(root.len(), |e| j + e);
                lines.push(root.get(j + 1..end).unwrap_or(&[]));
                j = end + 1;
            } else {
                j += 1;
            }
        }
        w.d.bl(lines.len() as i32);
        for line in &lines {
            let mut lc = Cursor { gs: line, at: 0 };
            let mut pts = Vec::new();
            while lc.gs.get(lc.at).is_some_and(|g| g.code == 10) {
                pts.push(cur_p(&mut lc, 10));
            }
            w.d.bl(pts.len() as i32);
            for p in pts {
                w.d.bd3(p);
            }
            if lc.gs.get(lc.at).is_some_and(|g| g.code == 90) {
                w.d.bl(1);
                w.d.bl(lc.i(90) as i32);
                let mut pairs = Vec::new();
                while lc.gs.get(lc.at).is_some_and(|g| g.code == 11) {
                    pairs.push((cur_p(&mut lc, 11), cur_p(&mut lc, 12)));
                }
                w.d.bl(pairs.len() as i32);
                for (a, b) in pairs {
                    w.d.bd3(a);
                    w.d.bd3(b);
                }
            } else {
                w.d.bl(0);
            }
            w.d.bl(lc.i(91) as i32);
            if r2010 {
                // What DXF leaves out: the entity's line type, ByBlock colour and weight, its
                // arrow size.
                w.d.bs(lc.id(170, 1) as i16);
                cmc_raw(w, lc.id(92, BYBLOCK));
                w.href(5, lc.h(340));
                w.d.bl(lc.id(171, -1) as i32);
                w.d.bd(lc.fd(40, arrow_size));
                w.href(5, lc.h(341));
                w.d.bl(lc.i(93) as i32);
            }
        }
        if r2010 {
            let at = root.iter().rposition(|g| g.code == 271);
            w.d.bs(at.and_then(|i| root.get(i)).and_then(|g| num(&g.value)).unwrap_or(0.0) as i16);
        }
    }
    // The content.
    let content = gs.get(content_from..content_to).unwrap_or(&[]);
    let mut cc = Cursor { gs: content, at: 0 };
    w.d.bd(cc.fd(40, 1.0));
    w.d.bd3(cur_p(&mut cc, 10));
    w.d.bd(cc.fd(41, 0.0));
    w.d.bd(cc.fd(140, 0.0));
    w.d.bd(cc.fd(145, 0.0));
    for code in [174, 175, 176, 177] {
        w.d.bs(cc.i(code) as i16);
    }
    let has_text = cc.i(290) != 0;
    w.d.b(has_text);
    if has_text {
        let label = cc.s(304);
        w.text(&label);
        w.d.bd3(cur_p(&mut cc, 11));
        w.href(5, cc.h(340));
        w.d.bd3(cur_p(&mut cc, 12));
        w.d.bd3(cur_p(&mut cc, 13));
        for code in [42, 43, 44, 45] {
            w.d.bd(cc.fd(code, 0.0));
        }
        w.d.bs(cc.i(170) as i16);
        cmc_raw(w, cc.i(90));
        w.d.bs(cc.i(171) as i16);
        w.d.bs(cc.i(172) as i16);
        cmc_raw(w, cc.i(91));
        w.d.bd(cc.fd(141, 0.0));
        w.d.bl(cc.i(92) as i32);
        w.d.b(cc.i(291) != 0);
        w.d.b(cc.i(292) != 0);
        w.d.bs(cc.i(173) as i16);
        w.d.b(cc.i(293) != 0);
        w.d.bd(cc.fd(142, 0.0));
        w.d.bd(cc.fd(143, 0.0));
        w.d.b(cc.i(294) != 0);
        let mut heights = Vec::new();
        while cc.gs.get(cc.at).is_some_and(|g| g.code == 144) {
            heights.push(cc.fd(144, 0.0));
        }
        w.d.bl(heights.len() as i32);
        for h in heights {
            w.d.bd(h);
        }
        w.d.b(cc.i(295) != 0);
    }
    let has_block = cc.i(296) != 0;
    w.d.b(has_block);
    if has_block {
        w.href(5, cc.h(341));
        w.d.bd3(cur_p(&mut cc, 14));
        w.d.bd3(cur_p(&mut cc, 15));
        w.d.bd3(cur_p(&mut cc, 16));
        w.d.bd(cc.fd(46, 0.0));
        cmc_raw(w, cc.i(93));
        for _ in 0..16 {
            w.d.bd(cc.fd(47, 0.0));
        }
    }
    w.d.bd3(cur_p(&mut cc, 110));
    w.d.bd3(cur_p(&mut cc, 111));
    w.d.bd3(cur_p(&mut cc, 112));
    w.d.b(cc.i(297) != 0);
    if r2010 {
        let mut bc = Cursor { gs: between, at: 0 };
        bc.seek(272);
        let bottom = bc.id(272, 9);
        let top = bc.id(273, 9);
        w.d.bs(top as i16);
        w.d.bs(bottom as i16);
    }
    // The entity's properties.
    let props = gs.get(tail_from + 1..).unwrap_or(&[]);
    let mut pc = Cursor { gs: props, at: 0 };
    w.href(5, pc.h(340));
    w.d.bl(pc.i(90) as i32);
    w.d.bs(pc.i(170) as i16);
    cmc_raw(w, pc.i(91));
    // No linetype override is ByBlock (a null handle reads as ByLayer).
    w.href(5, pc.h(341).or_else(|| names.get("LTYPE", "ByBlock")));
    w.d.bl(pc.i(171) as i32);
    w.d.b(pc.i(290) != 0);
    w.d.b(pc.i(291) != 0);
    w.d.bd(pc.fd(41, 0.0));
    w.href(5, pc.h(342));
    w.d.bd(pc.fd(42, 0.0));
    w.d.bs(pc.i(172) as i16);
    w.href(5, pc.h(343));
    w.d.bs(pc.i(173) as i16);
    w.d.bs(pc.i(95) as i16);
    w.d.bs(pc.i(174) as i16);
    w.d.bs(pc.i(175) as i16);
    cmc_raw(w, pc.i(92));
    w.d.b(pc.i(292) != 0);
    w.href(5, pc.h(344));
    cmc_raw(w, pc.id(93, BYBLOCK));
    w.d.bd3(if pc.gs.get(pc.at).is_some_and(|g| g.code == 10) { cur_p(&mut pc, 10) } else { [1.0; 3] });
    w.d.bd(pc.fd(43, 0.0));
    let scale = pc.fd(45, 1.0);
    w.d.bs(pc.i(176) as i16);
    w.d.b(pc.i(293) != 0);
    if !r2010 {
        let mut arrows = Vec::new();
        while pc.gs.get(pc.at).is_some_and(|g| g.code == 94) {
            arrows.push((pc.i(94) != 0, pc.h(345)));
        }
        w.d.bl(arrows.len() as i32);
        for (b, h) in arrows {
            w.d.b(b);
            w.href(5, h);
        }
    }
    let mut labels = Vec::new();
    while pc.gs.get(pc.at).is_some_and(|g| g.code == 330) {
        let h = pc.h(330);
        let index = pc.i(177);
        let width = pc.fd(44, 0.0);
        let text = pc.s(302);
        labels.push((h, text, index, width));
    }
    w.d.bl(labels.len() as i32);
    for (h, text, index, width) in labels {
        w.href(5, h);
        w.text(&text);
        w.d.bs(index as i16);
        w.d.bd(width);
    }
    w.d.b(pc.i(294) != 0);
    w.d.bs(pc.i(178) as i16);
    w.d.bs(pc.i(179) as i16);
    w.d.bd(scale);
    if r2010 {
        w.d.bs(pc.i(271) as i16);
        let bottom = pc.id(272, 9);
        let top = pc.id(273, 9);
        w.d.bs(top as i16);
        w.d.bs(bottom as i16);
    }
    if w.at_least(Version::R2013) {
        w.d.b(pc.i(295) != 0);
    }
}

/// A point from the cursor (x, y and z groups).
fn cur_p(c: &mut Cursor, code: i32) -> [f64; 3] {
    let x = c.fd(code, 0.0);
    let y = c.fd(code + 10, 0.0);
    let z = c.fd(code + 20, 0.0);
    [x, y, z]
}

fn num(v: &super::Value) -> Option<f64> {
    match v {
        super::Value::Real(f) => Some(*f),
        super::Value::Int(i) => Some(*i as f64),
        super::Value::Str(s) => s.trim().parse().ok(),
        super::Value::Handle(_) => None,
    }
}

/// A table's control object (spec §20.4.48 …): its entries' handles (and the special entries
/// some tables keep apart).
pub fn control(v: Version, type_code: u16, handle: u64, entries: &[u64], special: &[u64]) -> RawObject {
    let mut w = ObjWriter::new(v, type_code, handle);
    object_common(&mut w, None, &[], None, &Rec(&[]), &Handles::default());
    w.d.bl(entries.len() as i32);
    if type_code == types::DIMSTYLE_CONTROL {
        // R2000+: the count of something unknown (spec §20.4.68), 0.
        w.d.rc(0);
    }
    for h in entries {
        w.href(2, Some(*h));
    }
    for h in special {
        w.href(3, Some(*h));
    }
    w.finish()
}

/// A table record (LAYER, LTYPE, STYLE, APPID) from its groups, or `None` for one not encoded.
pub fn table_record(v: Version, name: &str, handle: u64, owner: Option<u64>, r: &Rec, names: &Handles) -> Option<RawObject> {
    let t = match name {
        "LAYER" => types::LAYER,
        "LTYPE" => types::LTYPE,
        "STYLE" => types::STYLE,
        "APPID" => types::APPID,
        "DIMSTYLE" => types::DIMSTYLE,
        "VPORT" => types::VPORT,
        "VIEW" => types::VIEW,
        "UCS" => types::UCS,
        _ => return None,
    };
    let mut w = ObjWriter::new(v, t, handle);
    object_common(&mut w, owner, &[], None, r, names);
    let flags = r.id(70, 0);
    table_entry(&mut w, r.s(2).unwrap_or(""), flags);
    match name {
        "LAYER" => {
            let color = r.id(62, 7);
            let mut values = 0i16;
            if flags & 1 != 0 {
                values |= 1;
            }
            if color < 0 {
                values |= 2;
            }
            if flags & 2 != 0 {
                values |= 4;
            }
            if flags & 4 != 0 {
                values |= 8;
            }
            if r.id(290, 1) != 0 {
                values |= 16;
            }
            values |= i16::from(super::encode::lineweight_index(r.id(370, -3))) << 5;
            w.d.bs(values);
            let index = color.abs();
            if w.at_least(Version::R2004) {
                w.d.bs(0);
                let raw = match r.i(420) {
                    Some(c) => 0xC200_0000u32 | (c as u32 & 0xFF_FFFF),
                    None => 0xC300_0000u32 | (index as u32 & 0xFF),
                };
                w.d.bl(raw as i32);
                w.d.rc(0);
            } else {
                w.d.bs(index as i16);
            }
            w.href(5, None);
            if w.at_least(Version::R2007) {
                w.href(5, None);
            }
            w.href(5, names.get("LTYPE", r.s(6).unwrap_or("Continuous")));
            // From R2013 one more (observed null in the files we've seen).
            if w.at_least(Version::R2013) {
                w.href(5, None);
            }
        }
        "LTYPE" => {
            w.text(r.s(3).unwrap_or(""));
            w.d.bd(r.fd(40, 0.0));
            w.d.rc(r.id(72, 65) as u8);
            let dashes: Vec<f64> = r.all(49).filter_map(|g| num(&g.value)).collect();
            w.d.rc(dashes.len().min(255) as u8);
            for d in dashes.iter().take(255) {
                w.d.bd(*d);
                w.d.bs(0);
                w.d.rd(0.0);
                w.d.rd(0.0);
                w.d.bd(1.0);
                w.d.bd(0.0);
                w.d.bs(0);
            }
            if !w.at_least(Version::R2007) {
                w.d.bytes(&[0u8; 256]);
            }
            for _ in dashes.iter().take(255) {
                w.href(5, None);
            }
        }
        "STYLE" => {
            w.d.b(flags & 4 != 0);
            w.d.b(flags & 1 != 0);
            w.d.bd(r.fd(40, 0.0));
            w.d.bd(r.fd(41, 1.0));
            w.d.bd(rad(r.fd(50, 0.0)));
            w.d.rc(r.id(71, 0) as u8);
            w.d.bd(r.fd(42, 2.5));
            w.text(r.s(3).unwrap_or(""));
            w.text(r.s(4).unwrap_or(""));
        }
        "DIMSTYLE" => dimstyle(&mut w, r, names),
        "VPORT" => vport(&mut w, r),
        "VIEW" => view(&mut w, r),
        "UCS" => {
            w.d.bd3(r.p(10));
            w.d.bd3(r.p(11));
            w.d.bd3(r.p(12));
            w.d.bd(r.fd(146, 0.0));
            w.d.bs(r.id(79, 0) as i16);
            w.d.bs(r.id(71, 0) as i16);
            w.href(5, None);
            w.href(5, None);
        }
        _ => w.d.rc(0),
    }
    Some(w.finish())
}

/// The second group with `code` (records where a code appears twice, like DIMSTYLE's 70).
fn second(r: &Rec, code: i32) -> Option<f64> {
    r.all(code).nth(1).and_then(|g| num(&g.value))
}

fn dimstyle(w: &mut ObjWriter, r: &Rec, names: &Handles) {
    let r2007 = w.at_least(Version::R2007);
    let r2010 = w.at_least(Version::R2010);
    let post = r.s(3).unwrap_or("");
    w.text(post);
    w.text(r.s(4).unwrap_or(""));
    let defaults = [(40, 1.0), (41, 0.18), (42, 0.0625), (43, 0.38), (44, 0.18), (45, 0.0), (46, 0.0), (47, 0.0), (48, 0.0)];
    for (code, d) in defaults {
        w.d.bd(r.fd(code, d));
    }
    if r2007 {
        w.d.bd(r.fd(49, 1.0));
        w.d.bd(rad(r.fd(50, 0.0)));
        w.d.bs(r.id(69, 0) as i16);
        cmc(w, second(r, 70).map_or(0, |x| x as i64), None);
    }
    for code in [71, 72, 73, 74, 75, 76] {
        w.d.b(r.id(code, 0) != 0);
    }
    for code in [77, 78, 79] {
        w.d.bs(r.id(code, 0) as i16);
    }
    if r2007 {
        w.d.bs(r.id(90, 0) as i16);
    }
    let defaults = [(140, 0.18), (141, 0.09), (142, 0.0), (143, 25.4), (144, 1.0), (145, 0.0), (146, 1.0), (147, 0.09), (148, 0.0)];
    for (code, d) in defaults {
        w.d.bd(r.fd(code, d));
    }
    w.d.b(r.id(170, 0) != 0);
    w.d.bs(r.id(171, 2) as i16);
    for code in [172, 173, 174, 175] {
        w.d.b(r.id(code, 0) != 0);
    }
    for code in [176, 177, 178] {
        cmc(w, r.id(code, 0), None);
    }
    w.d.bs(r.id(179, 0) as i16);
    let defaults = [(271, 4), (272, 4), (273, 2), (274, 2), (275, 0), (276, 0), (277, 2), (278, 46), (279, 0), (280, 0)];
    for (code, d) in defaults {
        w.d.bs(r.id(code, d) as i16);
    }
    w.d.b(r.id(281, 0) != 0);
    w.d.b(r.id(282, 0) != 0);
    for code in [283, 284, 285, 286] {
        w.d.bs(r.id(code, 0) as i16);
    }
    w.d.b(r.id(288, 0) != 0);
    w.d.bs(r.id(289, 3) as i16);
    if r2007 {
        w.d.b(r.id(290, 0) != 0);
    }
    if r2010 {
        w.d.b(r.id(294, 0) != 0);
        w.d.bd(0.0);
        w.text("");
        w.d.bd(0.0);
        w.text("");
    }
    w.d.bs(r.id(371, -2) as i16);
    w.d.bs(r.id(372, -2) as i16);
    w.d.b(false);
    let block = |code: i32| r.h(code).filter(|h| *h != 0);
    w.href(5, block(340).or_else(|| names.get("STYLE", "Standard")));
    for code in [341, 342, 343, 344] {
        w.href(5, block(code));
    }
    if r2007 {
        for code in [345, 346, 347] {
            w.href(5, block(code));
        }
    }
}

/// The view mode's four bits (spec §20.4.61, from DXF 71): perspective, front clip, back clip,
/// front clip at eye.
fn view_mode(w: &mut ObjWriter, vm: i64) {
    for bit in [1, 2, 4, 16] {
        w.d.b(vm & bit != 0);
    }
}

fn lights(w: &mut ObjWriter, r: &Rec) {
    w.d.b(r.id(292, 1) != 0);
    w.d.rc(r.id(282, 1) as u8);
    w.d.bd(r.fd(141, 0.0));
    w.d.bd(r.fd(142, 0.0));
    cmc(w, r.id(63, 250), None);
}

fn vport(w: &mut ObjWriter, r: &Rec) {
    let height = r.fd(40, 1.0);
    w.d.bd(height);
    w.d.bd(r.fd(41, 1.0) * height);
    w.d.rd2(r.p2(12));
    w.d.bd3(r.p(17));
    w.d.bd3(if r.f(16).is_some() { r.p(16) } else { [0.0, 0.0, 1.0] });
    w.d.bd(rad(r.fd(51, 0.0)));
    w.d.bd(r.fd(42, 50.0));
    w.d.bd(r.fd(43, 0.0));
    w.d.bd(r.fd(44, 0.0));
    let vm = r.id(71, 0);
    view_mode(w, vm);
    w.d.rc(r.id(281, 0) as u8);
    if w.at_least(Version::R2007) {
        lights(w, r);
    }
    w.d.rd2(r.p2(10));
    w.d.rd2(if r.f(11).is_some() { r.p2(11) } else { [1.0, 1.0] });
    w.d.b(vm & 8 != 0);
    w.d.bs(r.id(72, 1000) as i16);
    w.d.b(r.id(73, 1) != 0);
    w.d.bb(r.id(74, 3) as u8);
    w.d.b(r.id(76, 0) != 0);
    w.d.rd2(r.p2(15));
    w.d.b(r.id(75, 0) != 0);
    w.d.b(r.id(77, 0) != 0);
    w.d.bs(r.id(78, 0) as i16);
    w.d.bd(rad(r.fd(50, 0.0)));
    w.d.rd2(r.p2(13));
    w.d.rd2(r.p2(14));
    w.d.b(false);
    w.d.b(r.id(65, 1) != 0);
    w.d.bd3(r.p(110));
    w.d.bd3(if r.f(111).is_some() { r.p(111) } else { [1.0, 0.0, 0.0] });
    w.d.bd3(if r.f(112).is_some() { r.p(112) } else { [0.0, 1.0, 0.0] });
    w.d.bd(r.fd(146, 0.0));
    w.d.bs(r.id(79, 0) as i16);
    if w.at_least(Version::R2007) {
        w.d.bs(r.id(60, 0) as i16);
        w.d.bs(r.id(61, 5) as i16);
    }
    // Background, visual style and sun (R2007+), named and base UCS.
    if w.at_least(Version::R2007) {
        w.href(4, None);
        w.href(5, None);
        w.href(3, None);
    }
    w.href(5, None);
    w.href(5, None);
}

fn view(w: &mut ObjWriter, r: &Rec) {
    w.d.bd(r.fd(40, 1.0));
    w.d.bd(r.fd(41, 1.0));
    w.d.rd2(r.p2(10));
    w.d.bd3(r.p(12));
    w.d.bd3(if r.f(11).is_some() { r.p(11) } else { [0.0, 0.0, 1.0] });
    w.d.bd(rad(r.fd(50, 0.0)));
    w.d.bd(r.fd(42, 50.0));
    w.d.bd(r.fd(43, 0.0));
    w.d.bd(r.fd(44, 0.0));
    view_mode(w, r.id(71, 0));
    w.d.rc(r.id(281, 0) as u8);
    if w.at_least(Version::R2007) {
        lights(w, r);
    }
    w.d.b(r.id(70, 0) & 1 != 0);
    let assoc = r.id(72, 0) != 0;
    w.d.b(assoc);
    if assoc {
        w.d.bd3(r.p(110));
        w.d.bd3(r.p(111));
        w.d.bd3(r.p(112));
        w.d.bd(r.fd(146, 0.0));
        w.d.bs(r.id(79, 0) as i16);
    }
    if w.at_least(Version::R2007) {
        w.d.b(false);
    }
    if w.at_least(Version::R2007) {
        w.href(4, None);
        w.href(5, None);
        w.href(3, None);
    }
    if assoc {
        w.href(5, None);
        w.href(5, None);
    }
    if w.at_least(Version::R2007) {
        w.href(5, None);
    }
}

/// A BLOCK_HEADER (spec §20.4.49) for a block record, with its BLOCK and ENDBLK entities and the
/// entities it holds.
pub fn block_header(
    v: Version,
    handle: u64,
    owner: Option<u64>,
    r: &Rec,
    block: Option<u64>,
    endblk: Option<u64>,
    entities: &[u64],
    layout: Option<u64>,
    flags70: i64,
    base: [f64; 3],
    xref_path: &str,
    names: &Handles,
) -> RawObject {
    let mut w = ObjWriter::new(v, types::BLOCK_HEADER, handle);
    object_common(&mut w, owner, &[], None, r, names);
    table_entry(&mut w, r.s(2).unwrap_or(""), 0);
    let (xref, overlay) = (flags70 & 4 != 0, flags70 & 8 != 0);
    w.d.b(flags70 & 1 != 0);
    w.d.b(flags70 & 2 != 0);
    w.d.b(xref);
    w.d.b(overlay);
    // Loaded (R2000+): an external reference is, as far as the file goes.
    w.d.b(xref);
    if w.at_least(Version::R2004) && !xref && !overlay {
        w.d.bl(entities.len() as i32);
    }
    w.d.bd3(base);
    w.text(xref_path);
    // No insert counts.
    w.d.rc(0);
    w.text("");
    // No preview.
    w.d.bl(0);
    if w.at_least(Version::R2007) {
        w.d.bs(r.id(70, 0) as i16);
        w.d.b(r.id(280, 1) != 0);
        w.d.rc(r.id(281, 0) as u8);
    }
    w.href(3, block);
    if xref || overlay {
        // An external reference's block holds no entities of this file.
    } else if w.at_least(Version::R2004) {
        for h in entities {
            w.href(3, Some(*h));
        }
    } else {
        w.href(4, entities.first().copied());
        w.href(4, entities.last().copied());
    }
    w.href(3, endblk);
    w.href(5, layout);
    w.finish()
}

/// An object from its groups (`owner`, reactors and extension dictionary given apart), or `None`
/// for one not encoded. `viewports` are a LAYOUT's viewport entities.
#[allow(clippy::too_many_arguments)]
pub fn object(
    v: Version,
    name: &str,
    handle: u64,
    owner: Option<u64>,
    reactors: &[u64],
    xdict: Option<u64>,
    r: &Rec,
    names: &Handles,
    viewports: &[u64],
) -> Option<RawObject> {
    let t = match name {
        "DICTIONARY" => types::DICTIONARY,
        "ACDBPLACEHOLDER" => types::PLACEHOLDER,
        "LAYOUT" => types::LAYOUT,
        "GROUP" => types::GROUP,
        "MLINESTYLE" => types::MLINESTYLE,
        "XRECORD" => types::XRECORD,
        "ACDBDICTIONARYWDFLT" | "PLOTSETTINGS" | "SCALE" | "DICTIONARYVAR" | "IMAGEDEF" | "IMAGEDEF_REACTOR" | "MLEADERSTYLE" | "RASTERVARIABLES" => {
            names.class(name)?
        }
        _ => return None,
    };
    let mut w = ObjWriter::new(v, t, handle);
    object_common(&mut w, owner, reactors, xdict, r, names);
    match name {
        "DICTIONARY" | "ACDBDICTIONARYWDFLT" => {
            let hard = r.id(280, 0) != 0;
            let keys: Vec<String> = r.all(3).map(|g| if let super::Value::Str(s) = &g.value { s.clone() } else { String::new() }).collect();
            let values: Vec<Option<u64>> =
                r.0.iter()
                    .filter(|g| g.code == 350 || g.code == 360)
                    .map(|g| match &g.value {
                        super::Value::Handle(h) => Some(*h),
                        super::Value::Str(s) => u64::from_str_radix(s.trim(), 16).ok(),
                        _ => None,
                    })
                    .collect();
            let n = keys.len().min(values.len());
            w.d.bl(n as i32);
            w.d.bs(r.id(281, 1) as i16);
            w.d.rc(u8::from(hard));
            for k in keys.iter().take(n) {
                w.text(k);
            }
            for h in values.iter().take(n) {
                w.href(if hard { 3 } else { 2 }, h.filter(|h| *h != 0));
            }
            if name == "ACDBDICTIONARYWDFLT" {
                w.href(5, r.h(340).filter(|h| *h != 0));
            }
        }
        "ACDBPLACEHOLDER" => {}
        "LAYOUT" => {
            plot_settings(&mut w, r);
            layout_data(&mut w, r, r.all(330).nth(1).and_then(|g| handle_of(&g.value)), viewports);
        }
        "PLOTSETTINGS" => plot_settings(&mut w, r),
        "GROUP" => {
            w.text(r.s(300).unwrap_or(""));
            w.d.bs(r.id(70, 0) as i16);
            w.d.bs(r.id(71, 1) as i16);
            let members: Vec<u64> = r.all(340).filter_map(|g| handle_of(&g.value)).collect();
            w.d.bl(members.len() as i32);
            for m in members {
                w.href(4, Some(m));
            }
        }
        "MLINESTYLE" => mlinestyle(&mut w, r, names),
        "MLEADERSTYLE" => mleaderstyle(&mut w, r),
        "RASTERVARIABLES" => {
            w.d.bl(r.id(90, 0) as i32);
            w.d.bs(r.id(70, 1) as i16);
            w.d.bs(r.id(71, 1) as i16);
            w.d.bs(r.id(72, 0) as i16);
        }
        "XRECORD" => xrecord(&mut w, r),
        "SCALE" => {
            w.d.bs(r.id(70, 0) as i16);
            w.text(r.s(300).unwrap_or(""));
            w.d.bd(r.fd(140, 1.0));
            w.d.bd(r.fd(141, 1.0));
            w.d.b(r.id(290, 0) != 0);
        }
        "DICTIONARYVAR" => {
            w.d.rc(r.id(280, 0) as u8);
            w.text(r.s(1).unwrap_or(""));
        }
        "IMAGEDEF" => {
            w.d.bl(r.id(90, 0) as i32);
            w.d.rd2(r.p2(10));
            w.text(r.s(1).unwrap_or(""));
            w.d.b(r.id(280, 1) != 0);
            w.d.rc(r.id(281, 0) as u8);
            w.d.rd2(r.p2(11));
        }
        _ => w.d.bl(r.id(90, 2) as i32),
    }
    Some(w.finish())
}

/// MLEADERSTYLE (spec §20.4.87) from its groups (angles in radians, as DXF has them).
fn mleaderstyle(w: &mut ObjWriter, r: &Rec) {
    let r2010 = w.at_least(Version::R2010);
    if r2010 {
        w.d.bs(r.id(179, 2) as i16);
    }
    w.d.bs(r.id(170, 2) as i16);
    w.d.bs(r.id(171, 1) as i16);
    w.d.bs(r.id(172, 0) as i16);
    w.d.bl(r.id(90, 2) as i32);
    w.d.bd(r.fd(40, 0.0));
    w.d.bd(r.fd(41, 0.0));
    w.d.bs(r.id(173, 1) as i16);
    cmc_raw(w, r.id(91, BYBLOCK));
    w.href(5, r.h(340).filter(|h| *h != 0));
    w.d.bl(r.id(92, -2) as i32);
    w.d.b(r.id(290, 1) != 0);
    w.d.bd(r.fd(42, 0.09));
    w.d.b(r.id(291, 1) != 0);
    w.d.bd(r.fd(43, 0.36));
    w.text(r.s(3).unwrap_or(""));
    w.href(5, r.h(341).filter(|h| *h != 0));
    w.d.bd(r.fd(44, 0.18));
    w.text(r.s(300).unwrap_or(""));
    w.href(5, r.h(342).filter(|h| *h != 0));
    w.d.bs(r.id(174, 1) as i16);
    w.d.bs(r.id(178, 1) as i16);
    w.d.bs(r.id(175, 1) as i16);
    w.d.bs(r.id(176, 0) as i16);
    cmc_raw(w, r.id(93, BYBLOCK));
    w.d.bd(r.fd(45, 0.18));
    w.d.b(r.id(292, 0) != 0);
    w.d.b(r.id(297, 0) != 0);
    w.d.bd(r.fd(46, 4.0));
    w.href(5, r.h(343).filter(|h| *h != 0));
    cmc_raw(w, r.id(94, BYBLOCK));
    w.d.bd3([r.fd(47, 1.0), r.fd(49, 1.0), r.fd(140, 1.0)]);
    w.d.b(r.id(293, 0) != 0);
    w.d.bd(r.fd(141, 0.0));
    w.d.b(r.id(294, 0) != 0);
    w.d.bs(r.id(177, 0) as i16);
    w.d.bd(r.fd(142, 1.0));
    w.d.b(r.id(295, 0) != 0);
    w.d.b(r.id(296, 0) != 0);
    w.d.bd(r.fd(143, 0.125));
    if r2010 {
        w.d.bs(r.id(271, 0) as i16);
        w.d.bs(r.id(273, 9) as i16);
        w.d.bs(r.id(272, 9) as i16);
    }
    // From R2013 one more bit (observed: the spec doesn't list it, but readers expect it).
    if w.at_least(Version::R2013) {
        w.d.b(r.id(298, 0) != 0);
    }
}

fn mline(w: &mut ObjWriter, r: &Rec, names: &Handles) {
    w.d.bd(r.fd(40, 1.0));
    w.d.rc(r.id(70, 0) as u8);
    w.d.bd3(r.p(10));
    w.d.bd3(extrusion(r));
    w.d.bs(r.id(71, 1) as i16);
    let lines = r.id(73, 0).clamp(0, 255) as usize;
    w.d.rc(lines as u8);
    // The vertices: 11, 12, 13, then per line 74 (41…) and 75 (42…).
    let start = r.0.iter().position(|g| g.code == 11).unwrap_or(r.0.len());
    let mut c = Cursor { gs: r.0.get(start..).unwrap_or(&[]), at: 0 };
    let mut verts = Vec::new();
    while c.gs.get(c.at).is_some_and(|g| g.code == 11) {
        let p = cur_p(&mut c, 11);
        let dir = cur_p(&mut c, 12);
        let miter = cur_p(&mut c, 13);
        let mut elems = Vec::new();
        for _ in 0..lines {
            let n = cap(c.i(74), &c);
            let params: Vec<f64> = (0..n).map(|_| c.fd(41, 0.0)).collect();
            let nf = cap(c.i(75), &c);
            let fills: Vec<f64> = (0..nf).map(|_| c.fd(42, 0.0)).collect();
            elems.push((params, fills));
        }
        verts.push((p, dir, miter, elems));
    }
    w.d.bs(verts.len() as i16);
    for (p, dir, miter, elems) in verts {
        w.d.bd3(p);
        w.d.bd3(dir);
        w.d.bd3(miter);
        for (params, fills) in elems {
            w.d.bs(params.len() as i16);
            for x in params {
                w.d.bd(x);
            }
            w.d.bs(fills.len() as i16);
            for x in fills {
                w.d.bd(x);
            }
        }
    }
    w.href(5, r.h(340).filter(|h| *h != 0).or_else(|| names.get("MLINESTYLE", r.s(2).unwrap_or("Standard"))));
}

/// IMAGE and WIPEOUT (the same data).
fn image(w: &mut ObjWriter, r: &Rec) {
    w.d.bl(r.id(90, 0) as i32);
    w.d.bd3(r.p(10));
    w.d.bd3(r.p(11));
    w.d.bd3(r.p(12));
    w.d.rd2(r.p2(13));
    w.d.bs(r.id(70, 7) as i16);
    w.d.b(r.id(280, 0) != 0);
    w.d.rc(r.id(281, 50) as u8);
    w.d.rc(r.id(282, 50) as u8);
    w.d.rc(r.id(283, 0) as u8);
    if w.at_least(Version::R2010) {
        w.d.b(r.id(290, 0) != 0);
    }
    let clip_type = r.id(71, 1);
    let pts = points2(r, 14);
    w.d.bs(clip_type as i16);
    if clip_type == 1 {
        w.d.rd2(pts.first().copied().unwrap_or([0.0; 2]));
        w.d.rd2(pts.get(1).copied().unwrap_or([0.0; 2]));
    } else {
        w.d.bl(pts.len() as i32);
        for p in pts {
            w.d.rd2(p);
        }
    }
    w.href(5, r.h(340).filter(|h| *h != 0));
    w.href(3, r.h(360).filter(|h| *h != 0));
}

/// Every 2D point with `code`, `code + 10` in order.
fn points2(r: &Rec, code: i32) -> Vec<[f64; 2]> {
    points(r, code).into_iter().map(|p| [p[0], p[1]]).collect()
}

fn handle_of(v: &super::Value) -> Option<u64> {
    match v {
        super::Value::Handle(h) => Some(*h),
        super::Value::Str(s) => u64::from_str_radix(s.trim(), 16).ok(),
        super::Value::Int(i) => u64::try_from(*i).ok(),
        super::Value::Real(_) => None,
    }
    .filter(|h| *h != 0)
}

fn mlinestyle(w: &mut ObjWriter, r: &Rec, names: &Handles) {
    w.text(r.s(2).unwrap_or(""));
    w.text(r.s(3).unwrap_or(""));
    w.d.bs(r.id(70, 0) as i16);
    let colors: Vec<i64> = r.all(62).filter_map(|g| num(&g.value)).map(|x| x as i64).collect();
    cmc(w, colors.first().copied().unwrap_or(256), None);
    w.d.bd(rad(r.fd(51, 90.0)));
    w.d.bd(rad(r.fd(52, 90.0)));
    let offsets: Vec<f64> = r.all(49).filter_map(|g| num(&g.value)).collect();
    let ltypes: Vec<String> = r.all(6).map(|g| if let super::Value::Str(s) = &g.value { s.clone() } else { String::new() }).collect();
    let n = offsets.len().min(255);
    w.d.rc(n as u8);
    for (i, off) in offsets.iter().take(n).enumerate() {
        w.d.bd(*off);
        cmc(w, colors.get(i + 1).copied().unwrap_or(256), None);
        let lt = ltypes.get(i).map(String::as_str).unwrap_or("BYLAYER");
        if w.at_least(Version::R2018) {
            w.href(5, names.get("LTYPE", lt));
        } else {
            w.d.bs(match lt.to_ascii_uppercase().as_str() {
                "BYLAYER" => 32767,
                "BYBLOCK" => 32766,
                _ => 0,
            });
        }
    }
}

/// XRECORD: the groups after the cloning flag (280) as its data.
fn xrecord(w: &mut ObjWriter, r: &Rec) {
    let at = r.0.iter().position(|g| g.code == 280).map_or(r.0.len(), |i| i + 1);
    let data_groups = r.0.get(at..).unwrap_or(&[]);
    let wide = w.at_least(Version::R2007);
    let mut d: Vec<u8> = Vec::new();
    let mut k = 0;
    while let Some(g) = data_groups.get(k) {
        k += 1;
        let code = g.code;
        let x = num(&g.value).unwrap_or(0.0);
        let s = match &g.value {
            super::Value::Str(s) => s.clone(),
            _ => String::new(),
        };
        match code {
            1..=9 | 100..=109 | 300..=309 | 410..=419 | 430..=439 | 470..=479 | 999 | 1000..=1003 | 1006..=1009 => {
                d.extend_from_slice(&(code as i16).to_le_bytes());
                if wide {
                    let u: Vec<u16> = s.encode_utf16().take(u16::MAX as usize).collect();
                    d.extend_from_slice(&(u.len() as u16).to_le_bytes());
                    for c in u {
                        d.extend_from_slice(&c.to_le_bytes());
                    }
                } else {
                    let b: Vec<u8> = s.chars().map(|c| u8::try_from(u32::from(c)).unwrap_or(b'?')).take(u16::MAX as usize).collect();
                    d.extend_from_slice(&(b.len() as u16).to_le_bytes());
                    d.push(0);
                    d.extend_from_slice(&b);
                }
            }
            10..=17 | 1010..=1019 => {
                d.extend_from_slice(&(code as i16).to_le_bytes());
                let y = data_groups.get(k).filter(|g| g.code == code + 10).and_then(|g| num(&g.value));
                if y.is_some() {
                    k += 1;
                }
                let z = data_groups.get(k).filter(|g| g.code == code + 20).and_then(|g| num(&g.value));
                if z.is_some() {
                    k += 1;
                }
                for c in [x, y.unwrap_or(0.0), z.unwrap_or(0.0)] {
                    d.extend_from_slice(&c.to_le_bytes());
                }
            }
            18..=59 | 110..=149 | 210..=239 | 460..=469 | 1020..=1059 => {
                d.extend_from_slice(&(code as i16).to_le_bytes());
                d.extend_from_slice(&x.to_le_bytes());
            }
            60..=79 | 170..=179 | 270..=289 | 370..=389 | 400..=409 | 1060..=1070 => {
                d.extend_from_slice(&(code as i16).to_le_bytes());
                d.extend_from_slice(&(x as i16).to_le_bytes());
            }
            90..=99 | 420..=429 | 440..=449 | 450..=459 | 1071 => {
                d.extend_from_slice(&(code as i16).to_le_bytes());
                d.extend_from_slice(&(x as i32).to_le_bytes());
            }
            160..=169 => {
                d.extend_from_slice(&(code as i16).to_le_bytes());
                d.extend_from_slice(&(x as i64).to_le_bytes());
            }
            290..=299 => {
                d.extend_from_slice(&(code as i16).to_le_bytes());
                d.push(u8::from(x != 0.0));
            }
            310..=319 | 1004 => {
                let bytes: Vec<u8> =
                    (0..s.len() / 2).filter_map(|i| s.get(2 * i..2 * i + 2).and_then(|h| u8::from_str_radix(h, 16).ok())).take(255).collect();
                d.extend_from_slice(&(code as i16).to_le_bytes());
                d.push(bytes.len() as u8);
                d.extend_from_slice(&bytes);
            }
            320..=369 | 390..=399 | 480..=481 | 1005 => {
                d.extend_from_slice(&(code as i16).to_le_bytes());
                d.extend_from_slice(&handle_of(&g.value).unwrap_or(0).to_le_bytes());
            }
            // Codes with no stored form are left out.
            _ => {}
        }
    }
    w.d.bl(d.len() as i32);
    w.d.bytes(&d);
    w.d.bs(r.id(280, 1) as i16);
}

/// The plot settings every layout starts with (spec §20.4.88 PLOTSETTINGS).
fn plot_settings(w: &mut ObjWriter, r: &Rec) {
    w.text(r.s(1).unwrap_or(""));
    w.text(r.s(2).unwrap_or(""));
    w.d.bs(r.id(70, 0) as i16);
    for code in [40, 41, 42, 43, 44, 45] {
        w.d.bd(r.fd(code, 0.0));
    }
    w.text(r.s(4).unwrap_or(""));
    w.d.bd(r.fd(46, 0.0));
    w.d.bd(r.fd(47, 0.0));
    for code in [72, 73, 74] {
        w.d.bs(r.id(code, 0) as i16);
    }
    for code in [48, 49, 140, 141] {
        w.d.bd(r.fd(code, 0.0));
    }
    if !w.at_least(Version::R2004) {
        w.text(r.s(6).unwrap_or(""));
    }
    w.d.bd(r.fd(142, 1.0));
    w.d.bd(r.fd(143, 1.0));
    w.text(r.s(7).unwrap_or(""));
    w.d.bs(r.id(75, 0) as i16);
    w.d.bd(r.fd(147, 1.0));
    w.d.bd(r.fd(148, 0.0));
    w.d.bd(r.fd(149, 0.0));
    if w.at_least(Version::R2004) {
        for code in [76, 77, 78] {
            w.d.bs(r.id(code, 0) as i16);
        }
        w.href(5, None);
    }
    if w.at_least(Version::R2007) {
        w.href(4, None);
    }
}

/// The layout's own data after its plot settings (spec §20.4.82).
fn layout_data(w: &mut ObjWriter, r: &Rec, block: Option<u64>, viewports: &[u64]) {
    // The DXF Reference gives the layout's 1/70/71 after AcDbLayout; the plot settings' own 1/70
    // come first, so the second occurrences are the layout's.
    let second_s = |code: i32| r.all(code).nth(1).and_then(|g| if let super::Value::Str(s) = &g.value { Some(s.clone()) } else { None });
    w.text(&second_s(1).unwrap_or_default());
    w.d.bs(r.id(71, 0) as i16);
    w.d.bs(second(r, 70).unwrap_or(0.0) as i16);
    w.d.bd3(r.p(13));
    w.d.rd2(r.p2(10));
    w.d.rd2(r.p2(11));
    w.d.bd3(r.p(12));
    w.d.bd3(r.p(16));
    w.d.bd3(r.p(17));
    w.d.bd(r.fd(146, 0.0));
    w.d.bs(r.id(76, 0) as i16);
    w.d.bd3(r.p(14));
    w.d.bd3(r.p(15));
    if w.at_least(Version::R2004) {
        w.d.bl(viewports.len() as i32);
    }
    w.href(4, block);
    w.href(4, r.h(331).filter(|h| *h != 0));
    w.href(5, r.h(346).filter(|h| *h != 0));
    w.href(5, r.h(345).filter(|h| *h != 0));
    if w.at_least(Version::R2004) {
        for vp in viewports {
            w.href(4, Some(*vp));
        }
    }
}
