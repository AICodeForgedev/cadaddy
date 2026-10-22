//! Annotation parts: north arrows, section and elevation marks, detail callouts, view titles,
//! grid bubbles, level and spot elevation marks, revision deltas, keynote and element tags, weld
//! symbols and drafting marks — drawn at plotted size (inches on the sheet) and placed at the
//! drawing's scale — plus graphic scale bars (drawn at model size for their scale) and title
//! blocks (drawn at sheet size, ANSI, ARCH and ISO).

use super::{ANNO, Args, Catalog, Discipline, Kit, METRIC, REAL, args, args_s};

const D: Discipline = Discipline::Annotation;

/// Text heights on the sheet: small and normal.
const SMALL: f64 = 3.0 / 32.0;
const NORMAL: f64 = 1.0 / 8.0;

pub(crate) fn add(c: &mut Catalog) {
    const NA: &str = "North arrows";
    for (i, label) in [
        "simple arrow",
        "circle and arrow",
        "half-filled arrow",
        "four-point star",
        "eight-point compass",
        "circle with filled pointer",
        "true and plan north",
        "needle",
    ]
    .iter()
    .enumerate()
    {
        c.add(D, NA, &format!("NORTH_ARROW_{}", i + 1), format!("North arrow, {label}"), "compass orientation", north_arrow, args(&[i as f64]), ANNO);
    }
    const MK: &str = "Section and elevation marks";
    for (i, label) in ["circle and pointer", "half-filled circle", "flag", "cut line with tail"].iter().enumerate() {
        c.add(
            D,
            MK,
            &format!("SECTION_MARK_{}", i + 1),
            format!("Section mark, {label}"),
            "cut callout bubble",
            section_mark,
            args(&[i as f64]),
            ANNO,
        );
    }
    for (word, label, n) in [("1", "single", 1.0), ("2", "two-way", 2.0), ("4", "four-way", 4.0)] {
        c.add(
            D,
            MK,
            &format!("ELEVATION_MARK_{word}"),
            format!("Elevation mark, {label}"),
            "interior exterior callout bubble",
            elevation_mark,
            args(&[n]),
            ANNO,
        );
    }
    c.add(D, MK, "DETAIL_CALLOUT", "Detail callout bubble".into(), "detail enlarged", detail_callout, args(&[]), ANNO);
    const TG: &str = "Tags and titles";
    c.add(D, TG, "VIEW_TITLE", "View title".into(), "drawing title label", view_title, args(&[0.0]), ANNO);
    c.add(D, TG, "VIEW_TITLE_SCALE", "View title with scale".into(), "drawing title label", view_title, args(&[1.0]), ANNO);
    for (word, label, r, hex) in [
        ("GRID_BUBBLE_050", "Grid bubble, 1/2\"", 0.25, 0.0),
        ("GRID_BUBBLE_075", "Grid bubble, 3/4\"", 0.375, 0.0),
        ("GRID_BUBBLE_HEX", "Grid bubble, hexagon", 0.25, 1.0),
    ] {
        c.add(D, TG, word, label.into(), "column line grid", grid_bubble, args(&[r, hex]), ANNO);
    }
    c.add(D, TG, "LEVEL_MARK", "Level marker".into(), "datum elevation floor", level_mark, args(&[0.0]), ANNO);
    c.add(D, TG, "SPOT_ELEVATION", "Spot elevation".into(), "grade height", level_mark, args(&[1.0]), ANNO);
    c.add(D, TG, "REVISION_DELTA", "Revision delta".into(), "revision triangle", revision, args(&[]), ANNO);
    for (i, label) in ["hexagon", "square", "diamond", "circle", "ellipse"].iter().enumerate() {
        c.add(
            D,
            TG,
            &format!("KEYNOTE_{}", label.to_ascii_uppercase()),
            format!("Keynote tag, {label}"),
            "keynote reference",
            keynote,
            args(&[i as f64]),
            ANNO,
        );
    }
    for (word, label, k) in [
        ("TAG_DOOR", "Door tag", 0.0),
        ("TAG_WINDOW", "Window tag", 1.0),
        ("TAG_ROOM", "Room tag", 2.0),
        ("TAG_WALL_TYPE", "Wall type tag", 3.0),
        ("TAG_EQUIPMENT", "Equipment tag", 4.0),
        ("TAG_CEILING", "Ceiling height tag", 5.0),
    ] {
        c.add(D, TG, word, label.into(), "label mark", tag, args(&[k]), ANNO);
    }
    const WS: &str = "Weld and drafting symbols";
    for (word, label, k) in [
        ("WELD_FILLET_ARROW", "Weld symbol, fillet arrow side", 0.0),
        ("WELD_FILLET_OTHER", "Weld symbol, fillet other side", 1.0),
        ("WELD_FILLET_BOTH", "Weld symbol, fillet both sides", 2.0),
        ("WELD_GROOVE_V", "Weld symbol, V-groove", 3.0),
        ("WELD_FIELD", "Weld symbol, field fillet", 4.0),
        ("WELD_ALL_AROUND", "Weld symbol, fillet all around", 5.0),
    ] {
        c.add(D, WS, word, label.into(), "aws welding", weld, args(&[k]), ANNO);
    }
    for (word, label, k) in [
        ("CENTERLINE_SYMBOL", "Centerline symbol", 0.0),
        ("PLATE_SYMBOL", "Plate symbol", 1.0),
        ("MATCH_LINE", "Match line", 2.0),
        ("BREAK_MARK", "Break mark", 3.0),
    ] {
        c.add(D, WS, word, label.into(), "drafting", drafting_mark, args(&[k]), ANNO);
    }
    scale_bars(c);
    title_blocks(c);
}

/// A filled triangle pointing from `base` (the middle of its back) toward (`ux`, `uy`).
fn pointer(k: &mut Kit, base: (f64, f64), ux: f64, uy: f64, len: f64, half: f64) {
    let tip = (base.0 + ux * len, base.1 + uy * len);
    k.fill(&[tip, (base.0 - uy * half, base.1 + ux * half), (base.0 + uy * half, base.1 - ux * half)]);
}

fn north_arrow(k: &mut Kit, a: &Args) {
    let style = a.get(0) as i64;
    match style {
        0 => {
            k.line((0.0, -0.4), (0.0, 0.4));
            pointer(k, (0.0, 0.25), 0.0, 1.0, 0.2, 0.08);
            k.text((0.0, 0.58), NORMAL, "N");
        }
        1 => {
            k.circle((0.0, 0.0), 0.35);
            k.poly(&[(0.0, 0.35), (0.12, -0.25), (0.0, -0.15), (-0.12, -0.25)]);
            k.text((0.0, 0.5), NORMAL, "N");
        }
        2 => {
            k.poly(&[(0.0, 0.45), (0.15, -0.3), (0.0, -0.18), (-0.15, -0.3)]);
            k.fill(&[(0.0, 0.45), (0.0, -0.18), (0.15, -0.3)]);
            k.text((0.0, 0.6), NORMAL, "N");
        }
        3 => {
            for (ux, uy) in [(0.0, 1.0), (1.0, 0.0), (0.0, -1.0), (-1.0, 0.0)] {
                let l = if uy > 0.5 { 0.45 } else { 0.3 };
                k.poly(&[(ux * l, uy * l), (0.06 * (ux + uy), 0.06 * (uy - ux)), (0.0, 0.0), (0.06 * (ux - uy), 0.06 * (uy + ux))]);
            }
            pointer(k, (0.0, 0.0), 0.0, 1.0, 0.45, 0.06);
            k.text((0.0, 0.6), NORMAL, "N");
        }
        4 => {
            k.circle((0.0, 0.0), 0.3);
            for i in 0..8 {
                let t = std::f64::consts::FRAC_PI_4 * f64::from(i);
                let l = if i % 2 == 0 { 0.45 } else { 0.25 };
                k.line((0.0, 0.0), (l * t.cos(), l * t.sin()));
            }
            pointer(k, (0.0, 0.0), 0.0, 1.0, 0.45, 0.05);
            for (t, at) in [("N", (0.0, 0.58)), ("E", (0.58, 0.0)), ("S", (0.0, -0.58)), ("W", (-0.58, 0.0))] {
                k.text(at, SMALL, t);
            }
        }
        5 => {
            k.circle((0.0, 0.0), 0.3);
            k.fill(&[(0.0, 0.42), (0.1, 0.0), (-0.1, 0.0)]);
            k.poly(&[(0.1, 0.0), (0.0, -0.42), (-0.1, 0.0)]);
            k.text((0.0, 0.56), NORMAL, "N");
        }
        6 => {
            k.line((0.0, -0.35), (0.0, 0.35));
            pointer(k, (0.0, 0.2), 0.0, 1.0, 0.18, 0.07);
            k.text((-0.07, 0.5), SMALL, "TN");
            let t = 15f64.to_radians();
            let (s, c) = (t.sin(), t.cos());
            k.line((-0.35 * s, -0.35 * c), (0.35 * s, 0.35 * c));
            k.poly(&[(0.35 * s, 0.35 * c), (0.2 * s - 0.06 * c, 0.2 * c + 0.06 * s), (0.2 * s + 0.06 * c, 0.2 * c - 0.06 * s)]);
            k.text((0.4 * s + 0.12, 0.47 * c), SMALL, "PN");
        }
        _ => {
            k.poly(&[(0.0, 0.45), (0.07, 0.0), (0.0, -0.45), (-0.07, 0.0)]);
            k.fill(&[(0.0, 0.45), (0.07, 0.0), (-0.07, 0.0)]);
            k.circle((0.0, 0.0), 0.02);
            k.text((0.0, 0.6), NORMAL, "N");
        }
    }
}

/// The two numbers of a callout: detail above the line, sheet below.
fn callout_text(k: &mut Kit, r: f64) {
    k.attdef_c("DETAIL", "Detail number", "1", (0.0, r * 0.45), SMALL);
    k.attdef_c("SHEET", "Sheet number", "A-501", (0.0, -r * 0.45), SMALL * 0.85);
}

/// A filled wedge on a circle of radius `r` about the origin pointing along (`ux`, `uy`): its base
/// the chord a quarter turn wide, its tip `out` from the centre.
fn wedge(k: &mut Kit, r: f64, ux: f64, uy: f64, out: f64) {
    let c = std::f64::consts::FRAC_1_SQRT_2 * r;
    let a = (ux * c - uy * c, uy * c + ux * c);
    let b = (ux * c + uy * c, uy * c - ux * c);
    k.fill(&[(ux * out, uy * out), a, b]);
}

fn section_mark(k: &mut Kit, a: &Args) {
    let style = a.get(0) as i64;
    let r = 0.25;
    if style == 2 {
        k.rect((-0.35, -0.2), (0.35, 0.2));
        k.line((-0.35, 0.0), (0.35, 0.0));
    } else {
        k.circle((0.0, 0.0), r);
        k.line((-r, 0.0), (r, 0.0));
    }
    if style == 3 {
        k.line((0.0, -r), (0.0, -r - 0.6));
    }
    // Which way the section looks.
    k.visibility("Direction", &["Right", "Up", "Left", "Down"]);
    for (g, (ux, uy)) in [(1.0, 0.0), (0.0, 1.0), (-1.0, 0.0), (0.0, -1.0)].into_iter().enumerate() {
        k.state(Some(g as u8));
        match style {
            // A wedge growing out of the circle toward the view.
            1 => wedge(k, r, ux, uy, r * 1.7),
            2 => pointer(k, (ux * 0.35, uy * 0.2), ux, uy, 0.15, 0.12),
            _ => pointer(k, (ux * r, uy * r), ux, uy, 0.15, 0.12),
        }
    }
    k.state(None);
    callout_text(k, if style == 2 { 0.35 } else { r });
}

fn elevation_mark(k: &mut Kit, a: &Args) {
    let n = a.get(0) as usize;
    let r = if n == 1 { 0.22 } else { 0.3 };
    k.circle((0.0, 0.0), r);
    let dirs = [(0.0, 1.0), (1.0, 0.0), (0.0, -1.0), (-1.0, 0.0)];
    let out = r * 1.6;
    match n {
        1 => {
            k.visibility("Direction", &["Up", "Right", "Down", "Left"]);
            for (g, (ux, uy)) in dirs.into_iter().enumerate() {
                k.state(Some(g as u8));
                wedge(k, r, ux, uy, out);
            }
            k.state(None);
            k.attdef_c("VIEW", "Elevation number", "1", (0.0, 0.0), SMALL);
        }
        2 => {
            k.visibility("Direction", &["Left and right", "Up and down"]);
            k.state(Some(0));
            wedge(k, r, 1.0, 0.0, out);
            wedge(k, r, -1.0, 0.0, out);
            k.state(Some(1));
            wedge(k, r, 0.0, 1.0, out);
            wedge(k, r, 0.0, -1.0, out);
            k.state(None);
            k.attdef_c("SHEET", "Sheet number", "A-401", (0.0, 0.0), SMALL * 0.8);
        }
        _ => {
            for (ux, uy) in dirs {
                wedge(k, r, ux, uy, out);
            }
            k.attdef_c("SHEET", "Sheet number", "A-401", (0.0, 0.0), SMALL * 0.7);
            for (tag, at) in [("UP", (0.0, out + 0.1)), ("RIGHT", (out + 0.12, 0.0)), ("DOWN", (0.0, -out - 0.1)), ("LEFT", (-out - 0.12, 0.0))] {
                k.attdef_c(tag, "Elevation number", "", at, SMALL * 0.8);
            }
        }
    }
}

fn detail_callout(k: &mut Kit, _: &Args) {
    let r = 0.25;
    k.circle((0.0, 0.0), r);
    k.line((-r, 0.0), (r, 0.0));
    k.line((r, 0.0), (r + 0.4, 0.0));
    callout_text(k, r);
}

fn view_title(k: &mut Kit, a: &Args) {
    let with_scale = a.get(0) > 0.5;
    let r = 0.25;
    k.circle((0.0, 0.0), r);
    k.line((-r, 0.0), (r, 0.0));
    callout_text(k, r);
    k.line((r, 0.0), (r + 3.0, 0.0));
    k.attdef("TITLE", "View title", "VIEW TITLE", (r + 0.1, 0.06), NORMAL * 1.25);
    if with_scale {
        k.text_left((r + 0.1, -0.16), SMALL, "SCALE:");
        k.attdef("SCALE", "Scale", "1/4\" = 1'-0\"", (r + 0.75, -0.16), SMALL);
    }
}

fn grid_bubble(k: &mut Kit, a: &Args) {
    let (r, hex) = (a.get(0), a.get(1) > 0.5);
    if hex {
        let pts: Vec<(f64, f64)> = (0..6)
            .map(|i| {
                let t = std::f64::consts::FRAC_PI_3 * f64::from(i);
                (r * 1.1 * t.cos(), r * 1.1 * t.sin())
            })
            .collect();
        k.poly(&pts);
    } else {
        k.circle((0.0, 0.0), r);
    }
    k.attdef_c("GRID", "Grid line", "A", (0.0, 0.0), r * 0.8);
}

fn level_mark(k: &mut Kit, a: &Args) {
    if a.get(0) > 0.5 {
        k.poly(&[(0.0, 0.0), (-0.08, 0.12), (0.08, 0.12)]);
        k.line((0.0, 0.0), (0.0, -0.05));
        k.attdef("ELEV", "Spot elevation", "100.00", (0.12, 0.06), SMALL);
    } else {
        k.circle((0.0, 0.0), 0.12);
        k.fill(&[(0.0, 0.0), (0.12, 0.0), (0.0, 0.12)]);
        k.fill(&[(0.0, 0.0), (-0.12, 0.0), (0.0, -0.12)]);
        k.line((0.12, 0.0), (2.5, 0.0));
        k.attdef("LEVEL", "Level name", "LEVEL 1", (0.2, 0.08), SMALL);
        k.attdef("ELEV", "Elevation", "0'-0\"", (0.2, -0.16), SMALL);
    }
}

fn revision(k: &mut Kit, _: &Args) {
    let h = 0.3;
    k.poly(&[(0.0, h * 0.66), (h * 0.58, -h * 0.33), (-h * 0.58, -h * 0.33)]);
    k.attdef_c("REV", "Revision number", "1", (0.0, -0.02), SMALL);
}

fn keynote(k: &mut Kit, a: &Args) {
    let r = 0.15;
    match a.get(0) as i64 {
        0 => {
            let pts: Vec<(f64, f64)> = (0..6)
                .map(|i| {
                    let t = std::f64::consts::FRAC_PI_3 * f64::from(i);
                    (r * 1.2 * t.cos(), r * 1.2 * t.sin())
                })
                .collect();
            k.poly(&pts);
        }
        1 => k.rect((-r, -r), (r, r)),
        2 => k.poly(&[(0.0, r * 1.3), (r * 1.3, 0.0), (0.0, -r * 1.3), (-r * 1.3, 0.0)]),
        3 => k.circle((0.0, 0.0), r),
        _ => k.ellipse((0.0, 0.0), r * 1.8, r),
    }
    k.attdef_c("KEY", "Keynote", "1", (0.0, 0.0), SMALL);
}

fn tag(k: &mut Kit, a: &Args) {
    match a.get(0) as i64 {
        0 => {
            k.circle((0.0, 0.0), 0.16);
            k.attdef_c("NUMBER", "Door number", "101", (0.0, 0.0), SMALL);
        }
        1 => {
            let pts: Vec<(f64, f64)> = (0..6)
                .map(|i| {
                    let t = std::f64::consts::FRAC_PI_3 * f64::from(i) + std::f64::consts::FRAC_PI_6;
                    (0.18 * t.cos(), 0.18 * t.sin())
                })
                .collect();
            k.poly(&pts);
            k.attdef_c("TYPE", "Window type", "A", (0.0, 0.0), SMALL);
        }
        2 => {
            k.rect((-0.6, -0.2), (0.6, 0.2));
            k.line((-0.6, -0.02), (0.6, -0.02));
            k.attdef_c("NAME", "Room name", "ROOM", (0.0, 0.09), SMALL);
            k.attdef_c("NUMBER", "Room number", "101", (0.0, -0.11), SMALL);
        }
        3 => {
            k.poly(&[(0.0, 0.18), (0.18, 0.0), (0.0, -0.18), (-0.18, 0.0)]);
            k.attdef_c("TYPE", "Wall type", "1", (0.0, 0.0), SMALL);
        }
        4 => {
            k.rect((-0.35, -0.15), (0.35, 0.15));
            k.line((-0.35, 0.0), (0.35, 0.0));
            k.attdef_c("EQUIP", "Equipment type", "AHU", (0.0, 0.075), SMALL * 0.8);
            k.attdef_c("NUMBER", "Equipment number", "1", (0.0, -0.075), SMALL * 0.8);
        }
        _ => {
            k.rect((-0.35, -0.12), (0.35, 0.12));
            k.line((-0.2, -0.12), (-0.2, 0.12));
            k.attdef_c("MATERIAL", "Ceiling type", "ACT", (-0.275, 0.0), SMALL * 0.7);
            k.attdef_c("HEIGHT", "Ceiling height", "9'-0\"", (0.075, 0.0), SMALL * 0.8);
        }
    }
}

/// A welding symbol: arrow (to the joint at the origin), reference line, the weld symbol.
fn weld(k: &mut Kit, a: &Args) {
    let kind = a.get(0) as i64;
    let (x0, y0) = (0.35, 0.35);
    k.line((0.0, 0.0), (x0, y0));
    let d = -std::f64::consts::FRAC_1_SQRT_2;
    pointer(k, (0.06, 0.06), d, d, 0.085, 0.035);
    k.line((x0, y0), (x0 + 0.8, y0));
    k.path(&[(x0 + 0.8, y0), (x0 + 0.95, y0 + 0.12)]);
    k.path(&[(x0 + 0.8, y0), (x0 + 0.95, y0 - 0.12)]);
    let m = x0 + 0.35;
    let fillet_below = |k: &mut Kit| k.poly(&[(m, y0), (m, y0 - 0.12), (m + 0.12, y0)]);
    let fillet_above = |k: &mut Kit| k.poly(&[(m, y0), (m, y0 + 0.12), (m + 0.12, y0)]);
    match kind {
        0 => fillet_below(k),
        1 => fillet_above(k),
        2 => {
            fillet_below(k);
            fillet_above(k);
        }
        3 => k.path(&[(m - 0.07, y0 - 0.12), (m, y0), (m + 0.07, y0 - 0.12)]),
        4 => {
            fillet_below(k);
            k.line((x0, y0), (x0, y0 + 0.2));
            k.fill(&[(x0, y0 + 0.2), (x0 + 0.12, y0 + 0.16), (x0, y0 + 0.12)]);
        }
        _ => {
            fillet_below(k);
            k.circle((x0, y0), 0.05);
        }
    }
    k.attdef("SIZE", "Weld size", "1/4", (m - 0.2, y0 - 0.1), SMALL * 0.8);
}

fn drafting_mark(k: &mut Kit, a: &Args) {
    match a.get(0) as i64 {
        0 => {
            k.text((0.0, 0.0), NORMAL, "C");
            k.line((0.03, -0.08), (0.03, 0.1));
            k.text((0.07, -0.06), SMALL * 0.7, "L");
        }
        1 => {
            k.line((-0.1, 0.0), (0.1, 0.0));
            k.line((0.1, 0.0), (0.1, 0.12));
            k.text((0.0, 0.08), SMALL, "PL");
        }
        2 => {
            k.line((-1.5, 0.0), (1.5, 0.0));
            k.line((-1.5, 0.02), (1.5, 0.02));
            k.attdef("SHEET", "Continued on sheet", "MATCH LINE - SEE A-102", (-1.4, 0.08), SMALL);
        }
        _ => k.path(&[(-0.5, 0.0), (-0.06, 0.0), (-0.03, 0.12), (0.03, -0.12), (0.06, 0.0), (0.5, 0.0)]),
    }
}

// ---------------------------------------------------------------------------------------------
// Scale bars: drawn at model size for their scale (so they plot right at that scale).

fn scale_bars(c: &mut Catalog) {
    const SB: &str = "Scale bars";
    // (label, name, plotted inches per foot).
    for (label, word, ipf) in [
        ("1/16\" = 1'-0\"", "1-16IN", 1.0 / 16.0),
        ("1/8\" = 1'-0\"", "1-8IN", 1.0 / 8.0),
        ("3/16\" = 1'-0\"", "3-16IN", 3.0 / 16.0),
        ("1/4\" = 1'-0\"", "1-4IN", 0.25),
        ("3/8\" = 1'-0\"", "3-8IN", 0.375),
        ("1/2\" = 1'-0\"", "1-2IN", 0.5),
        ("3/4\" = 1'-0\"", "3-4IN", 0.75),
        ("1\" = 1'-0\"", "1IN", 1.0),
        ("1-1/2\" = 1'-0\"", "1-1-2IN", 1.5),
        ("3\" = 1'-0\"", "3IN", 3.0),
    ] {
        c.add(
            D,
            SB,
            &format!("SCALE_BAR_{word}"),
            format!("Scale bar, {label}"),
            "graphic scale imperial",
            scale_bar,
            args_s(label, &[12.0 / ipf, 0.0]),
            REAL,
        );
    }
    for (label, n) in [
        ("1:1", 1.0),
        ("1:5", 5.0),
        ("1:10", 10.0),
        ("1:20", 20.0),
        ("1:50", 50.0),
        ("1:100", 100.0),
        ("1:200", 200.0),
        ("1:500", 500.0),
        ("1:1000", 1000.0),
    ] {
        c.add(D, SB, &format!("SCALE_BAR_1-{n}"), format!("Scale bar, {label}"), "graphic scale metric", scale_bar, args_s(label, &[n, 1.0]), METRIC);
    }
}

/// A bar 4 divisions long. `f` is model units per sheet unit (inches for imperial, mm metric).
fn scale_bar(k: &mut Kit, a: &Args) {
    let (f, metric) = (a.get(0), a.get(1) > 0.5);
    // The division: a round model length near half an inch (12 mm) on the sheet.
    let (target, unit, steps): (f64, f64, &[f64]) = if metric {
        (12.0 * f, 1.0, &[1.0, 2.0, 5.0, 10.0, 20.0, 50.0, 100.0, 200.0, 500.0, 1000.0, 2000.0, 5000.0, 10_000.0])
    } else {
        (0.5 * f, 12.0, &[0.25, 0.5, 1.0, 2.0, 4.0, 5.0, 8.0, 10.0, 16.0, 20.0, 40.0])
    };
    let div = steps.iter().map(|s| s * unit).min_by(|a, b| (a - target).abs().total_cmp(&(b - target).abs())).unwrap_or(target);
    let h = if metric { 3.0 * f } else { 0.1 * f };
    let text = if metric { 2.5 * f } else { SMALL * f };
    for i in 0..4 {
        let x = i as f64 * div;
        if i % 2 == 0 {
            k.fill(&[(x, 0.0), (x + div, 0.0), (x + div, h), (x, h)]);
        }
        k.rect((x, 0.0), (x + div, h));
    }
    // Metric divisions in metres from 1 m up (else millimetres), the unit after the last number.
    let metres = metric && div >= 1000.0;
    for i in 0..=4 {
        let x = i as f64 * div;
        let v = x / unit;
        let label = match (metric, metres) {
            (true, true) => format!("{}", v / 1000.0),
            (true, false) => format!("{v}"),
            _ if v < 1.0 && x > 0.0 => format!("{x}\""),
            _ => format!("{v}'"),
        };
        let label = if metric && i == 4 { format!("{label} {}", if metres { "m" } else { "mm" }) } else { label };
        k.text((x, h + text * 1.2), text, &label);
    }
    k.text_left((0.0, -text * 1.5), text, a.s);
}

// ---------------------------------------------------------------------------------------------
// Title blocks: borders and a title strip at sheet size (inches; ISO in millimetres).

fn title_blocks(c: &mut Catalog) {
    const TB: &str = "Title blocks";
    for (word, w, h) in [("ANSI_A", 11.0, 8.5), ("ANSI_B", 17.0, 11.0), ("ANSI_C", 22.0, 17.0), ("ANSI_D", 34.0, 22.0), ("ANSI_E", 44.0, 34.0)] {
        c.add(
            D,
            TB,
            &format!("TITLE_{word}"),
            format!("Title block, {} {w}\"x{h}\"", word.replace('_', " ")),
            "border sheet landscape",
            title_block,
            args(&[w, h, 0.5]),
            REAL,
        );
    }
    c.add(
        D,
        TB,
        "TITLE_ANSI_A_PORTRAIT",
        "Title block, ANSI A portrait 8.5\"x11\"".into(),
        "border sheet",
        title_block,
        args(&[8.5, 11.0, 0.5]),
        REAL,
    );
    for (word, w, h) in [
        ("ARCH_A", 12.0, 9.0),
        ("ARCH_B", 18.0, 12.0),
        ("ARCH_C", 24.0, 18.0),
        ("ARCH_D", 36.0, 24.0),
        ("ARCH_E1", 42.0, 30.0),
        ("ARCH_E", 48.0, 36.0),
    ] {
        c.add(
            D,
            TB,
            &format!("TITLE_{word}"),
            format!("Title block, {} {w}\"x{h}\"", word.replace('_', " ")),
            "border sheet landscape",
            title_block,
            args(&[w, h, 0.5]),
            REAL,
        );
    }
    for (word, w, h) in
        [("ISO_A4", 297.0, 210.0), ("ISO_A3", 420.0, 297.0), ("ISO_A2", 594.0, 420.0), ("ISO_A1", 841.0, 594.0), ("ISO_A0", 1189.0, 841.0)]
    {
        c.add(
            D,
            TB,
            &format!("TITLE_{word}"),
            format!("Title block, {} {w}x{h} mm", word.replace('_', " ")),
            "border sheet landscape metric",
            title_block,
            args(&[w, h, 10.0]),
            METRIC,
        );
    }
    c.add(
        D,
        TB,
        "TITLE_ISO_A4_PORTRAIT",
        "Title block, ISO A4 portrait 210x297 mm".into(),
        "border sheet metric",
        title_block,
        args(&[210.0, 297.0, 10.0]),
        METRIC,
    );
}

/// A sheet `w` x `h` with a border `m` in (wider at the binding edge), a title box at the bottom
/// right (small sheets) or a strip down the right side, the fields as attributes and zone marks
/// along the border on large sheets.
fn title_block(k: &mut Kit, a: &Args) {
    let (w, h, m) = (a.get(0), a.get(1), a.get(2));
    let metric = m > 5.0;
    let inch = if metric { 25.4 } else { 1.0 };
    let left = if metric { 20.0 } else { m * 2.0 };
    k.rect((0.0, 0.0), (w, h));
    let (x0, y0, x1, y1) = (left, m, w - m, h - m);
    k.rect((x0, y0), (x1, y1));
    let small_sheet = w.max(h) <= 17.5 * inch;
    let t = 0.125 * inch;
    // Fields: (tag, prompt, default) top to bottom.
    let fields: [(&str, &str, &str); 9] = [
        ("FIRM", "Firm name", "FIRM NAME"),
        ("PROJECT", "Project name", "PROJECT NAME"),
        ("SHEET_TITLE", "Sheet title", "SHEET TITLE"),
        ("PROJECT_NO", "Project number", ""),
        ("DATE", "Date", ""),
        ("SCALE", "Scale", "AS NOTED"),
        ("DRAWN", "Drawn by", ""),
        ("CHECKED", "Checked by", ""),
        ("SHEET_NO", "Sheet number", "A-101"),
    ];
    if small_sheet {
        // A box 4" x 2.25" at the bottom right.
        let (bw, bh) = (4.0 * inch, 2.25 * inch);
        let (bx, by) = (x1 - bw, y0);
        k.rect((bx, by), (x1, by + bh));
        let row = bh / fields.len() as f64;
        for (i, (tag, prompt, def)) in fields.iter().enumerate() {
            let y = by + bh - row * (i as f64 + 1.0);
            if i > 0 {
                k.line((bx, y + row), (x1, y + row));
            }
            k.attdef(tag, prompt, def, (bx + t * 0.6, y + row * 0.3), t * 0.75);
        }
    } else {
        // A strip down the right side.
        let sw = if w >= 40.0 * inch { 4.0 * inch } else { 3.0 * inch };
        let sx = x1 - sw;
        k.rect((sx, y0), (x1, y1));
        let rows = [0.12, 0.12, 0.1, 0.05, 0.05, 0.05, 0.05, 0.05, 0.12];
        let mut y = y1;
        let tall = y1 - y0;
        // The revisions block takes what the fields leave, above the sheet number.
        let used: f64 = rows.iter().sum::<f64>() * tall;
        let revisions = (tall - used).max(0.0);
        for (i, ((tag, prompt, def), f)) in fields.iter().zip(rows).enumerate() {
            if i == fields.len() - 1 {
                k.line((sx, y), (x1, y));
                k.text_left((sx + t, y - t * 1.5), t * 0.8, "REVISIONS");
                y -= revisions;
            }
            let rh = f * tall;
            k.line((sx, y), (x1, y));
            let size = if i == fields.len() - 1 {
                t * 3.0
            } else if i < 3 {
                t * 1.4
            } else {
                t
            };
            k.attdef(tag, prompt, def, (sx + t, y - rh * 0.5 - size * 0.4), size);
            y -= rh;
        }
        // Zone marks: numbers along the top and bottom, letters up the sides.
        let cols = ((x1 - x0) / (6.0 * inch)).round().max(2.0) as usize;
        let rows_n = ((y1 - y0) / (6.0 * inch)).round().max(2.0) as usize;
        for i in 0..cols {
            let x = x0 + (x1 - x0) * (i as f64 + 0.5) / cols as f64;
            let label = format!("{}", cols - i);
            k.text((x, (h + y1) / 2.0), t * 1.2, &label);
            k.text((x, y0 / 2.0), t * 1.2, &label);
            if i > 0 {
                let xb = x0 + (x1 - x0) * i as f64 / cols as f64;
                k.line((xb, y1), (xb, h));
                k.line((xb, 0.0), (xb, y0));
            }
        }
        for j in 0..rows_n {
            let y = y0 + (y1 - y0) * (j as f64 + 0.5) / rows_n as f64;
            let letter = char::from(b'A' + (j % 26) as u8).to_string();
            k.text((left / 2.0, y), t * 1.2, &letter);
            k.text(((w + x1) / 2.0, y), t * 1.2, &letter);
        }
    }
}
