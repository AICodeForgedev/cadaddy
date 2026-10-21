//! AutoCAD Release 12 DXF (`$ACADVER` AC1009), per the R12 DXF Reference: HEADER, TABLES,
//! BLOCKS and ENTITIES only (no CLASSES or OBJECTS section), no handles (`$HANDLING` 0), colours
//! as ACI numbers, lightweight polylines written as heavy POLYLINE/VERTEX/SEQEND.
//!
//! The drawing is expected to hold only what R12 has (the engine brings it down first:
//! `cmd::file::r12`); anything else is skipped here. Model space is written, then the first
//! layout's objects as paper space (group 67), without its viewports, which R12 keeps in
//! extended data CADaddy doesn't write.

use cadaddy_color::Color;
use cadaddy_doc::{Attrib, Drawing, Entity, EntityKind, HAlign, HVal, MeshKind, Text, VAlign};
use cadaddy_dxf::Tag;
use cadaddy_geom::Vec3;

struct W {
    t: Vec<Tag>,
}

impl W {
    fn s(&mut self, c: i32, v: impl Into<String>) {
        self.t.push(Tag::s(c, v));
    }
    fn f(&mut self, c: i32, v: f64) {
        self.t.push(Tag::f(c, if v.is_finite() { v } else { 0.0 }));
    }
    fn i(&mut self, c: i32, v: i64) {
        self.t.push(Tag::i(c, v));
    }
    fn p(&mut self, c: i32, p: Vec3) {
        self.f(c, p.x);
        self.f(c + 10, p.y);
        self.f(c + 20, p.z);
    }
}

/// The R12 DXF tags of `d`.
pub(crate) fn tags(d: &Drawing) -> Vec<Tag> {
    let mut w = W { t: Vec::new() };
    header(&mut w, d);
    tables(&mut w, d);
    w.s(0, "SECTION");
    w.s(2, "BLOCKS");
    for (name, b) in &d.blocks {
        let lower = name.to_ascii_lowercase();
        if lower.starts_with("*model_space") || lower.starts_with("*paper_space") {
            continue;
        }
        w.s(0, "BLOCK");
        w.s(8, "0");
        w.s(2, name);
        let mut flags = 0;
        if name.starts_with('*') {
            flags |= 1;
        }
        if b.entities.iter().any(|e| matches!(e.kind, EntityKind::AttDef(_))) {
            flags |= 2;
        }
        if b.xref_path.is_some() {
            flags |= 4;
        }
        w.i(70, flags);
        w.p(10, b.base);
        w.s(3, name);
        if let Some(path) = &b.xref_path {
            w.s(1, path);
        } else {
            for e in b.entities.iter() {
                entity(&mut w, e, false);
            }
        }
        w.s(0, "ENDBLK");
        w.s(8, "0");
    }
    w.s(0, "ENDSEC");
    w.s(0, "SECTION");
    w.s(2, "ENTITIES");
    for e in d.model.iter() {
        entity(&mut w, e, false);
    }
    if let Some(l) = d.layouts.first() {
        for e in l.entities.iter() {
            entity(&mut w, e, true);
        }
    }
    w.s(0, "ENDSEC");
    w.s(0, "EOF");
    w.t
}

fn header(w: &mut W, d: &Drawing) {
    w.s(0, "SECTION");
    w.s(2, "HEADER");
    w.s(9, "$ACADVER");
    w.s(1, "AC1009");
    w.s(9, "$DWGCODEPAGE");
    w.s(3, crate::dxf_write::code_page(d, cadaddy_dxf::Version::R12));
    let point = |name: &str| d.header.get(name).and_then(|v| if let HVal::Point(p) = v { Some(*p) } else { None });
    for (name, default) in [("INSBASE", Vec3::ZERO), ("EXTMIN", Vec3::ZERO), ("EXTMAX", Vec3::ZERO)] {
        w.s(9, format!("${name}"));
        w.p(10, point(name).unwrap_or(default));
    }
    for name in ["CLAYER", "CELTYPE", "TEXTSTYLE"] {
        if let Some(v) = d.header.get(name).and_then(HVal::as_str).filter(|v| !v.is_empty()) {
            w.s(9, format!("${name}"));
            w.s(
                if name == "CLAYER" {
                    8
                } else if name == "CELTYPE" {
                    6
                } else {
                    7
                },
                v,
            );
        }
    }
    for name in ["LTSCALE", "TEXTSIZE", "PDSIZE"] {
        if let Some(v) = d.header.get(name).and_then(HVal::as_f64) {
            w.s(9, format!("${name}"));
            w.f(40, v);
        }
    }
    if let Some(v) = d.header.get("PDMODE").and_then(HVal::as_f64) {
        w.s(9, "$PDMODE");
        w.i(70, v as i64);
    }
    w.s(9, "$HANDLING");
    w.i(70, 0);
    w.s(0, "ENDSEC");
}

fn table(w: &mut W, name: &str, count: usize) {
    w.s(0, "TABLE");
    w.s(2, name);
    w.i(70, i64::try_from(count).unwrap_or(i64::MAX));
}

fn tables(w: &mut W, d: &Drawing) {
    w.s(0, "SECTION");
    w.s(2, "TABLES");
    table(w, "VPORT", 0);
    w.s(0, "ENDTAB");
    // Continuous is always there; ByLayer and ByBlock are not table entries in R12.
    let lts: Vec<&cadaddy_doc::Linetype> =
        d.linetypes.iter().filter(|l| !matches!(l.name.to_ascii_lowercase().as_str(), "bylayer" | "byblock" | "continuous")).collect();
    table(w, "LTYPE", lts.len() + 1);
    w.s(0, "LTYPE");
    w.s(2, "CONTINUOUS");
    w.i(70, 0);
    w.s(3, "Solid line");
    w.i(72, 65);
    w.i(73, 0);
    w.f(40, 0.0);
    for l in lts {
        // R12 dashes are lengths only: shapes and text in a pattern are left out.
        let dashes: Vec<f64> = l.pattern.iter().map(|e| e.length).collect();
        w.s(0, "LTYPE");
        w.s(2, &l.name);
        w.i(70, 0);
        w.s(3, &l.description);
        w.i(72, 65);
        w.i(73, i64::try_from(dashes.len()).unwrap_or(0));
        w.f(40, dashes.iter().map(|x| x.abs()).sum());
        for x in dashes {
            w.f(49, x);
        }
    }
    w.s(0, "ENDTAB");
    table(w, "LAYER", d.layers.len());
    for l in &d.layers {
        w.s(0, "LAYER");
        w.s(2, &l.name);
        w.i(70, i64::from(l.frozen) | (i64::from(l.locked) << 2));
        let aci = match l.color.to_aci() {
            0 | 256 => 7,
            c => c,
        };
        // A layer that is off has a negative colour.
        w.i(62, i64::from(if l.on { aci } else { -aci }));
        w.s(6, if l.linetype.is_empty() { "CONTINUOUS" } else { l.linetype.as_str() });
    }
    w.s(0, "ENDTAB");
    table(w, "STYLE", d.text_styles.len());
    for s in &d.text_styles {
        w.s(0, "STYLE");
        w.s(2, &s.name);
        w.i(70, 0);
        w.f(40, s.height);
        w.f(41, if s.width_factor > 0.0 { s.width_factor } else { 1.0 });
        w.f(50, s.oblique.to_degrees());
        w.i(71, (i64::from(s.backwards) * 2) | (i64::from(s.upside_down) * 4));
        w.f(42, if s.height > 0.0 { s.height } else { 2.5 });
        // The built-in font goes out as `txt`, which every reader has (as in the other DXF
        // versions); R12 keeps no xdata to carry CADaddy's own name.
        let builtin = s.font.is_empty() || s.font.eq_ignore_ascii_case("CADaddy Stroke");
        w.s(3, if builtin { "txt" } else { s.font.as_str() });
        w.s(4, &s.big_font);
    }
    w.s(0, "ENDTAB");
    for name in ["VIEW", "UCS"] {
        table(w, name, 0);
        w.s(0, "ENDTAB");
    }
    table(w, "APPID", 1);
    w.s(0, "APPID");
    w.s(2, "ACAD");
    w.i(70, 0);
    w.s(0, "ENDTAB");
    table(w, "DIMSTYLE", 0);
    w.s(0, "ENDTAB");
    w.s(0, "ENDSEC");
}

fn common(w: &mut W, e: &Entity, kind: &str, paper: bool) {
    w.s(0, kind);
    w.s(8, if e.common.layer.is_empty() { "0" } else { e.common.layer.as_str() });
    let lt = e.common.linetype.as_str();
    if !lt.is_empty() && !lt.eq_ignore_ascii_case("bylayer") {
        w.s(6, lt);
    }
    if e.common.color != Color::ByLayer {
        w.i(62, i64::from(e.common.color.to_aci()));
    }
    if paper {
        w.i(67, 1);
    }
}

fn halign(a: HAlign) -> i64 {
    match a {
        HAlign::Left => 0,
        HAlign::Center => 1,
        HAlign::Right => 2,
        HAlign::Aligned => 3,
        HAlign::Middle => 4,
        HAlign::Fit => 5,
    }
}

fn valign(a: VAlign) -> i64 {
    match a {
        VAlign::Baseline => 0,
        VAlign::Bottom => 1,
        VAlign::Middle => 2,
        VAlign::Top => 3,
    }
}

/// The groups TEXT, ATTDEF and ATTRIB share before their own; returns the alignment codes.
fn text_body(w: &mut W, t: &Text, value: &str) -> (i64, i64) {
    w.p(10, t.insert);
    w.f(40, t.height);
    w.s(1, control_codes(value));
    if t.rotation != 0.0 {
        w.f(50, t.rotation.to_degrees());
    }
    if (t.width_factor - 1.0).abs() > 1e-12 {
        w.f(41, t.width_factor);
    }
    if t.oblique != 0.0 {
        w.f(51, t.oblique.to_degrees());
    }
    w.s(7, &t.style);
    (halign(t.halign), valign(t.valign))
}

/// The degree, plus/minus and diameter signs as R12's `%%d`, `%%p` and `%%c` (R12 DXF Reference,
/// TEXT): a dimension's text brought down to TEXT holds the characters, which R12 programs would
/// otherwise get as `\U+` escapes they don't read.
fn control_codes(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for c in value.chars() {
        match c {
            '\u{00B0}' => out.push_str("%%d"),
            '\u{00B1}' => out.push_str("%%p"),
            '\u{2300}' | '\u{2205}' => out.push_str("%%c"),
            c => out.push(c),
        }
    }
    out
}

/// The alignment groups; the vertical one is 73 for TEXT and 74 for ATTDEF and ATTRIB (R12 DXF
/// Reference).
fn text_end(w: &mut W, t: &Text, h: i64, v: i64, valign_code: i32) {
    if h != 0 {
        w.i(72, h);
    }
    if let Some(a) = t.align_pt.filter(|_| h != 0 || v != 0) {
        w.p(11, a);
    }
    if v != 0 {
        w.i(valign_code, v);
    }
}

fn attrib(w: &mut W, e: &Entity, a: &Attrib, def: bool, paper: bool) {
    common(w, e, if def { "ATTDEF" } else { "ATTRIB" }, paper);
    let (h, v) = text_body(w, &a.text, &a.text.value);
    if def {
        w.s(3, &a.prompt);
    }
    w.s(2, &a.tag);
    w.i(70, i64::from(a.invisible) | (i64::from(a.constant) << 1));
    text_end(w, &a.text, h, v, 74);
}

fn vertex(w: &mut W, e: &Entity, p: Vec3, flags: i64, paper: bool) {
    common(w, e, "VERTEX", paper);
    w.p(10, p);
    if flags != 0 {
        w.i(70, flags);
    }
}

fn seqend(w: &mut W, e: &Entity, paper: bool) {
    common(w, e, "SEQEND", paper);
}

fn entity(w: &mut W, e: &Entity, paper: bool) {
    match &e.kind {
        EntityKind::Line(l) => {
            common(w, e, "LINE", paper);
            w.p(10, l.a);
            w.p(11, l.b);
        }
        EntityKind::Point(p) => {
            common(w, e, "POINT", paper);
            w.p(10, p.p);
        }
        EntityKind::Circle(c) => {
            common(w, e, "CIRCLE", paper);
            w.p(10, c.center);
            w.f(40, c.radius);
        }
        EntityKind::Arc(a) => {
            common(w, e, "ARC", paper);
            w.p(10, a.center);
            w.f(40, a.radius);
            w.f(50, a.start.to_degrees());
            w.f(51, a.end.to_degrees());
        }
        EntityKind::Text(t) => {
            common(w, e, "TEXT", paper);
            let (h, v) = text_body(w, t, &t.value);
            text_end(w, t, h, v, 73);
        }
        EntityKind::AttDef(a) => attrib(w, e, a, true, paper),
        EntityKind::Insert(ins) => {
            common(w, e, "INSERT", paper);
            if !ins.attribs.is_empty() {
                w.i(66, 1);
            }
            w.s(2, &ins.block);
            w.p(10, ins.insert);
            for (c, v) in [(41, ins.scale.x), (42, ins.scale.y), (43, ins.scale.z)] {
                if (v - 1.0).abs() > 1e-12 {
                    w.f(c, v);
                }
            }
            if ins.rotation != 0.0 {
                w.f(50, ins.rotation.to_degrees());
            }
            if ins.cols > 1 || ins.rows > 1 {
                w.i(70, i64::from(ins.cols.max(1)));
                w.i(71, i64::from(ins.rows.max(1)));
                w.f(44, ins.col_spacing);
                w.f(45, ins.row_spacing);
            }
            if !ins.attribs.is_empty() {
                for a in &ins.attribs {
                    attrib(w, e, a, false, paper);
                }
                seqend(w, e, paper);
            }
        }
        EntityKind::Solid(s) | EntityKind::Trace(s) => {
            common(w, e, if matches!(e.kind, EntityKind::Solid(_)) { "SOLID" } else { "TRACE" }, paper);
            for (i, p) in s.corners.iter().enumerate() {
                w.p(10 + i as i32, *p);
            }
        }
        EntityKind::Face3d(f) => {
            common(w, e, "3DFACE", paper);
            for (i, p) in f.corners.iter().enumerate() {
                w.p(10 + i as i32, *p);
            }
            if f.hidden_edges != 0 {
                w.i(70, i64::from(f.hidden_edges & 15));
            }
        }
        EntityKind::LwPolyline(p) => {
            common(w, e, "POLYLINE", paper);
            w.i(66, 1);
            w.p(10, Vec3::new(0.0, 0.0, p.elevation));
            w.i(70, i64::from(p.closed) | if p.plinegen { 128 } else { 0 });
            if p.const_width != 0.0 {
                w.f(40, p.const_width);
                w.f(41, p.const_width);
            }
            for v in &p.vertices {
                common(w, e, "VERTEX", paper);
                w.p(10, v.p.to3(p.elevation));
                if v.start_width != 0.0 || v.end_width != 0.0 {
                    w.f(40, v.start_width);
                    w.f(41, v.end_width);
                }
                if v.bulge != 0.0 {
                    w.f(42, v.bulge);
                }
            }
            seqend(w, e, paper);
        }
        EntityKind::Polyline3d(p) => {
            common(w, e, "POLYLINE", paper);
            w.i(66, 1);
            w.p(10, Vec3::ZERO);
            w.i(70, 8 | i64::from(p.closed));
            for v in &p.points {
                vertex(w, e, *v, 32, paper);
            }
            seqend(w, e, paper);
        }
        EntityKind::Mesh(m) => match m.kind {
            MeshKind::Polyface => {
                common(w, e, "POLYLINE", paper);
                w.i(66, 1);
                w.p(10, Vec3::ZERO);
                w.i(70, 64);
                w.i(71, i64::try_from(m.vertices.len()).unwrap_or(0));
                w.i(72, i64::try_from(m.faces.len()).unwrap_or(0));
                for v in &m.vertices {
                    vertex(w, e, *v, 192, paper);
                }
                for f in &m.faces {
                    common(w, e, "VERTEX", paper);
                    w.p(10, Vec3::ZERO);
                    w.i(70, 128);
                    // At most four corners; a hidden edge's index is negative.
                    for (i, idx) in f.v.iter().take(4).enumerate() {
                        let n = i64::from(*idx) + 1;
                        w.i(71 + i as i32, if (f.hidden >> i) & 1 == 1 { -n } else { n });
                    }
                }
                seqend(w, e, paper);
            }
            MeshKind::Polygon { m: rows, n: cols, closed_m, closed_n } => {
                common(w, e, "POLYLINE", paper);
                w.i(66, 1);
                w.p(10, Vec3::ZERO);
                w.i(70, 16 | i64::from(closed_m) | (i64::from(closed_n) << 5));
                w.i(71, i64::from(rows));
                w.i(72, i64::from(cols));
                for v in &m.vertices {
                    vertex(w, e, *v, 64, paper);
                }
                seqend(w, e, paper);
            }
            MeshKind::Subd => {}
        },
        // Nothing else has an R12 form (the engine converts it before writing).
        _ => {}
    }
}
