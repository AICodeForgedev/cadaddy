//! Sample drawings generated in code (original CADaddy content, used for demos and tests).

use cadaddy_color::Color;
use cadaddy_doc::{Common, DimKind, Dimension, Drawing, EntityKind, Hatch, HatchLoop, Layer, Lineweight, MText, Space, Text};
use cadaddy_geom::{PolyVertex, Vec2, Vec3, arc_to_bulge};

use crate::cmd::helpers::{arc, circle, line, lwpoly, rect_vertices, v3};

fn layer(name: &str, color: u8, lt: &str, lw: u16) -> Layer {
    Layer { name: name.into(), color: Color::Index(color), linetype: lt.into(), lineweight: Lineweight::Mm100(lw), ..Layer::default() }
}

fn on(layer: &str) -> Common {
    Common { layer: layer.into(), ..Common::default() }
}

fn dim(kind: DimKind, p13: Vec2, p14: Vec2, defpt: Vec2) -> EntityKind {
    EntityKind::Dimension(Box::new(Dimension {
        kind,
        defpt: v3(defpt),
        text_mid: Vec3::ZERO,
        p13: v3(p13),
        p14: v3(p14),
        p15: Vec3::ZERO,
        p16: Vec3::ZERO,
        text: String::new(),
        style: "Standard".into(),
        measurement: 0.0,
        text_rotation: 0.0,
        user_text_pos: false,
        block: None,
        overrides: Default::default(),
        assoc: Vec::new(),
        marks: Default::default(),
    }))
}

fn text(at: Vec2, h: f64, s: &str) -> EntityKind {
    EntityKind::Text(Text {
        insert: v3(at),
        align_pt: None,
        height: h,
        value: s.into(),
        rotation: 0.0,
        width_factor: 1.0,
        oblique: 0.0,
        style: "Standard".into(),
        halign: Default::default(),
        valign: Default::default(),
    })
}

/// A mechanical part drawing: a flanged mounting bracket in two views with dimensions,
/// centre lines, hidden lines, a section hatch and a title block. Units: inches.
pub fn bracket() -> Drawing {
    let mut d = Drawing::new_imperial();
    d.linetypes.extend(cadaddy_doc::library::standard_linetypes());
    d.layers.extend([
        layer("Outline", 7, "Continuous", 50),
        layer("Center", 1, "CENTER", 18),
        layer("Hidden", 2, "HIDDEN", 25),
        layer("Dimensions", 4, "Continuous", 18),
        layer("Hatch", 8, "Continuous", 13),
        layer("Notes", 3, "Continuous", 18),
        layer("Title", 7, "Continuous", 35),
    ]);
    d.header.set_f64("LTSCALE", 0.5);
    d.header.set_str("CLAYER", "Outline");
    if let Some(ds) = d.dim_styles.first_mut() {
        ds.arrow_size = 0.12;
        ds.text_height = 0.14;
        ds.decimals = 2;
        ds.text_above = 1;
        ds.text_gap = 0.05;
        ds.ext_offset = 0.06;
        ds.ext_extend = 0.1;
    }
    let m = Space::Model;
    let mut add = |c: Common, k: EntityKind| {
        let _ = d.add(&m, c, k);
    };

    // ---- Front view: plate 8 x 5 with filleted corners, origin (1, 1).
    let (x0, y0, w, h, r) = (1.0, 1.0, 8.0, 5.0, 0.5);
    let b = arc_to_bulge(std::f64::consts::FRAC_PI_2);
    let outline = vec![
        PolyVertex::new(Vec2::new(x0 + r, y0)),
        PolyVertex::with_bulge(Vec2::new(x0 + w - r, y0), b),
        PolyVertex::new(Vec2::new(x0 + w, y0 + r)),
        PolyVertex::with_bulge(Vec2::new(x0 + w, y0 + h - r), b),
        PolyVertex::new(Vec2::new(x0 + w - r, y0 + h)),
        PolyVertex::with_bulge(Vec2::new(x0 + r, y0 + h), b),
        PolyVertex::new(Vec2::new(x0, y0 + h - r)),
        PolyVertex::with_bulge(Vec2::new(x0, y0 + r), b),
    ];
    add(on("Outline"), lwpoly(outline, true));
    // Bolt holes.
    let holes = [Vec2::new(2.0, 2.0), Vec2::new(8.0, 2.0), Vec2::new(2.0, 5.0), Vec2::new(8.0, 5.0)];
    for hc in holes {
        add(on("Outline"), circle(hc, 0.3125));
        add(on("Center"), line(hc - Vec2::X * 0.55, hc + Vec2::X * 0.55));
        add(on("Center"), line(hc - Vec2::Y * 0.55, hc + Vec2::Y * 0.55));
    }
    // Central boss with bore.
    let c = Vec2::new(5.0, 3.5);
    add(on("Outline"), circle(c, 1.5));
    add(on("Outline"), circle(c, 0.875));
    add(on("Hidden"), circle(c, 1.125));
    add(on("Center"), line(c - Vec2::X * 1.9, c + Vec2::X * 1.9));
    add(on("Center"), line(c - Vec2::Y * 1.9, c + Vec2::Y * 1.9));
    // Slot.
    let slot = vec![
        PolyVertex::new(Vec2::new(3.25, 1.6)),
        PolyVertex::with_bulge(Vec2::new(6.75, 1.6), 1.0),
        PolyVertex::new(Vec2::new(6.75, 1.1) + Vec2::Y * 0.0),
    ];
    let _ = slot;
    let slot = vec![
        PolyVertex::new(Vec2::new(4.0, 1.5)),
        PolyVertex::with_bulge(Vec2::new(6.0, 1.5), 1.0),
        PolyVertex::new(Vec2::new(6.0, 1.9)),
        PolyVertex::with_bulge(Vec2::new(4.0, 1.9), 1.0),
    ];
    add(on("Outline"), lwpoly(slot, true));

    // ---- Side view (section A-A) to the right: plate 0.5 thick + boss 1.5 tall.
    let sx = 11.0;
    let sec_plate = rect_vertices(Vec2::new(sx, y0), Vec2::new(sx + 0.5, y0 + h));
    add(on("Outline"), lwpoly(sec_plate.clone(), true));
    let boss = rect_vertices(Vec2::new(sx + 0.5, c.y - 1.5), Vec2::new(sx + 2.0, c.y + 1.5));
    add(on("Outline"), lwpoly(boss.clone(), true));
    // Bore (hidden in side view → shown as section void).
    add(on("Outline"), line(Vec2::new(sx, c.y - 0.875), Vec2::new(sx + 2.0, c.y - 0.875)));
    add(on("Outline"), line(Vec2::new(sx, c.y + 0.875), Vec2::new(sx + 2.0, c.y + 0.875)));
    add(on("Center"), line(Vec2::new(sx - 0.4, c.y), Vec2::new(sx + 2.4, c.y)));
    // Section hatch on the solid parts (two regions around the bore).
    let hatch = |a: Vec2, bb: Vec2| {
        EntityKind::Hatch(Box::new(Hatch {
            pattern: "ANSI31".into(),
            solid: false,
            loops: vec![HatchLoop { curves: Vec::new(), vertices: rect_vertices(a, bb), outer: true, sources: Vec::new() }],
            scale: 1.0,
            angle: 0.0,
            associative: false,
            style: 0,
            elevation: 0.0,
            gradient: None,
            origin: Vec2::ZERO,
            background: None,
            pattern_lines: Vec::new(),
        }))
    };
    add(on("Hatch"), hatch(Vec2::new(sx, c.y + 0.875), Vec2::new(sx + 0.5, y0 + h)));
    add(on("Hatch"), hatch(Vec2::new(sx, y0), Vec2::new(sx + 0.5, c.y - 0.875)));
    add(on("Hatch"), hatch(Vec2::new(sx + 0.5, c.y + 0.875), Vec2::new(sx + 2.0, c.y + 1.5)));
    add(on("Hatch"), hatch(Vec2::new(sx + 0.5, c.y - 1.5), Vec2::new(sx + 2.0, c.y - 0.875)));

    // ---- Dimensions.
    add(on("Dimensions"), dim(DimKind::Linear { rotation: 0.0 }, Vec2::new(x0, y0), Vec2::new(x0 + w, y0), Vec2::new(5.0, 0.35)));
    add(
        on("Dimensions"),
        dim(DimKind::Linear { rotation: std::f64::consts::FRAC_PI_2 }, Vec2::new(x0, y0), Vec2::new(x0, y0 + h), Vec2::new(0.3, 3.5)),
    );
    add(on("Dimensions"), dim(DimKind::Linear { rotation: 0.0 }, Vec2::new(2.0, 5.0), Vec2::new(8.0, 5.0), Vec2::new(5.0, 6.6)));
    add(on("Dimensions"), dim(DimKind::Linear { rotation: 0.0 }, Vec2::new(sx, y0 + h), Vec2::new(sx + 2.0, c.y + 1.5), Vec2::new(sx + 1.0, 6.6)));
    add(
        on("Dimensions"),
        EntityKind::Dimension(Box::new(Dimension {
            kind: DimKind::Diameter,
            defpt: v3(c + Vec2::from_angle(0.8) * 1.5),
            text_mid: Vec3::ZERO,
            p13: Vec3::ZERO,
            p14: Vec3::ZERO,
            p15: v3(c - Vec2::from_angle(0.8) * 1.5),
            p16: Vec3::ZERO,
            text: String::new(),
            style: "Standard".into(),
            measurement: 0.0,
            text_rotation: 0.0,
            user_text_pos: false,
            block: None,
            overrides: Default::default(),
            assoc: Vec::new(),
            marks: Default::default(),
        })),
    );
    add(
        on("Dimensions"),
        EntityKind::Dimension(Box::new(Dimension {
            kind: DimKind::Radius,
            defpt: v3(Vec2::new(8.0, 2.0)),
            text_mid: Vec3::ZERO,
            p13: Vec3::ZERO,
            p14: Vec3::ZERO,
            p15: v3(Vec2::new(8.0, 2.0) + Vec2::from_angle(-0.7) * 0.3125),
            p16: Vec3::ZERO,
            text: "4X <>".into(),
            style: "Standard".into(),
            measurement: 0.0,
            text_rotation: 0.0,
            user_text_pos: false,
            block: None,
            overrides: Default::default(),
            assoc: Vec::new(),
            marks: Default::default(),
        })),
    );

    // ---- Notes and title block.
    add(on("Notes"), text(Vec2::new(sx - 0.2, -0.3), 0.16, "SECTION A-A"));
    add(on("Notes"), text(Vec2::new(4.1, -0.3), 0.16, "FRONT VIEW"));
    add(
        on("Notes"),
        EntityKind::MText(MText { background: None, insert: v3(Vec2::new(14.2, 6.2)), height: 0.12, width: 3.4, attach: 1, rotation: 0.0, style: "Standard".into(), contents: "NOTES:\\P1. MATERIAL: 6061-T6 ALUMINUM.\\P2. BREAK ALL SHARP EDGES 0.02 MAX.\\P3. BORE %%c1.750 THRU, H7.\\P4. FINISH: CLEAR ANODIZE.".into(), line_spacing: 1.0, line_spacing_exact: false, columns: None }),
    );
    let tb0 = Vec2::new(14.0, -0.5);
    add(on("Title"), lwpoly(rect_vertices(tb0, tb0 + Vec2::new(4.0, 2.0)), true));
    add(on("Title"), line(tb0 + Vec2::new(0.0, 1.2), tb0 + Vec2::new(4.0, 1.2)));
    add(on("Title"), line(tb0 + Vec2::new(0.0, 0.6), tb0 + Vec2::new(4.0, 0.6)));
    add(on("Title"), line(tb0 + Vec2::new(2.0, 0.0), tb0 + Vec2::new(2.0, 0.6)));
    add(on("Title"), text(tb0 + Vec2::new(0.15, 1.5), 0.22, "MOUNTING BRACKET"));
    add(on("Title"), text(tb0 + Vec2::new(0.15, 0.82), 0.12, "CADADDY SAMPLE  DWG NO. CD-0001"));
    add(on("Title"), text(tb0 + Vec2::new(0.15, 0.22), 0.12, "SCALE 1:1"));
    add(on("Title"), text(tb0 + Vec2::new(2.15, 0.22), 0.12, "SHEET 1 OF 1"));
    // Drawing border.
    add(on("Title"), lwpoly(rect_vertices(Vec2::new(-0.5, -0.5), Vec2::new(18.0, 7.5)), true));
    let _ = arc;
    d
}

/// A small architectural floor plan (feet-inches, drawn in inches).
pub fn floor_plan() -> Drawing {
    let mut d = Drawing::new_imperial();
    d.linetypes.extend(cadaddy_doc::library::standard_linetypes());
    d.layers.extend([
        layer("A-WALL", 7, "Continuous", 50),
        layer("A-DOOR", 3, "Continuous", 25),
        layer("A-GLAZ", 4, "Continuous", 18),
        layer("A-ANNO", 2, "Continuous", 18),
        layer("A-FURN", 8, "Continuous", 13),
    ]);
    d.header.set_i64("LUNITS", 4);
    d.header.set_i64("LUPREC", 4);
    // Annotation sized for plotting at 1/2" = 1'-0" (24): dimension text 0.18 x 24, notes 4.5".
    d.header.set_f64("DIMSCALE", 24.0);
    d.header.set_f64("TEXTSIZE", 4.5);
    for st in &mut d.dim_styles {
        st.scale = 24.0;
    }
    let m = Space::Model;
    let mut add = |c: Common, k: EntityKind| {
        let _ = d.add(&m, c, k);
    };
    let ft = 12.0;
    let t = 6.0;
    // Outer walls 30' x 20'.
    add(on("A-WALL"), lwpoly(rect_vertices(Vec2::ZERO, Vec2::new(30.0 * ft, 20.0 * ft)), true));
    add(on("A-WALL"), lwpoly(rect_vertices(Vec2::new(t, t), Vec2::new(30.0 * ft - t, 20.0 * ft - t)), true));
    // Interior wall.
    add(on("A-WALL"), lwpoly(rect_vertices(Vec2::new(14.0 * ft, t), Vec2::new(14.0 * ft + 4.0, 20.0 * ft - t)), true));
    // Door swing.
    let hinge = Vec2::new(14.0 * ft + 4.0, 6.0 * ft);
    add(on("A-DOOR"), line(hinge, hinge + Vec2::X * 36.0));
    add(on("A-DOOR"), arc(&cadaddy_geom::Arc::new(hinge, 36.0, 0.0, std::f64::consts::FRAC_PI_2)));
    // Windows.
    for x in [5.0, 20.0] {
        add(on("A-GLAZ"), line(Vec2::new(x * ft, 0.0), Vec2::new(x * ft + 48.0, 0.0)));
        add(on("A-GLAZ"), line(Vec2::new(x * ft, t), Vec2::new(x * ft + 48.0, t)));
        add(on("A-GLAZ"), line(Vec2::new(x * ft, t / 2.0), Vec2::new(x * ft + 48.0, t / 2.0)));
    }
    add(on("A-ANNO"), text(Vec2::new(5.0 * ft, 10.0 * ft), 9.0, "LIVING ROOM"));
    add(on("A-ANNO"), text(Vec2::new(19.0 * ft, 10.0 * ft), 9.0, "BEDROOM"));
    add(on("A-ANNO"), dim(DimKind::Linear { rotation: 0.0 }, Vec2::ZERO, Vec2::new(30.0 * ft, 0.0), Vec2::new(15.0 * ft, -3.0 * ft)));
    // Furniture.
    add(on("A-FURN"), lwpoly(rect_vertices(Vec2::new(20.0 * ft, 14.0 * ft), Vec2::new(25.0 * ft, 19.0 * ft)), true));
    add(on("A-FURN"), circle(Vec2::new(7.0 * ft, 14.0 * ft), 24.0));
    d
}

/// A made-up print set as a job leaves it (Q6416): `sheets` sheets side by side in model space,
/// each page to the right of the one before with a gap of a twentieth of its width, as the job's
/// import lays them, holding about `per_sheet` objects in a plan sheet's mix: a border and title
/// block, wall lines and room outlines, solid and pattern hatches, room tags and notes, and
/// symbol blocks. The job's records say where each sheet is (`sheet_regions`) and what it is
/// (its sheet index: numbers `Q-001`… and titles `Sample Sheet 1`…), in the drawing's agent
/// state. Deterministic; nothing of any real job.
pub fn print_set(sheets: usize, per_sheet: usize) -> Drawing {
    let mut d = Drawing::new_imperial();
    d.layers.extend([
        layer("G-BORD", 7, "Continuous", 50),
        layer("A-WALL", 7, "Continuous", 35),
        layer("A-ROOM", 3, "Continuous", 18),
        layer("A-PATT", 8, "Continuous", 13),
        layer("A-ANNO", 2, "Continuous", 18),
        layer("E-SYMB", 1, "Continuous", 25),
    ]);
    let mut sym = cadaddy_doc::Block::new("SYM-OUTLET");
    for k in [circle(Vec2::ZERO, 4.0), line(Vec2::new(-4.0, 0.0), Vec2::new(4.0, 0.0)), line(Vec2::new(0.0, 4.0), Vec2::new(0.0, 9.0))] {
        sym.entities.push(cadaddy_doc::Entity::new(d.new_handle(), k));
    }
    d.blocks.insert(sym.name.clone(), std::sync::Arc::new(sym));
    // A page 36" x 24" at 1/4" = 1'-0": 144' x 96' of plan, in inches.
    let (w, h) = (36.0 * 48.0, 24.0 * 48.0);
    let mut seed = 0x2545_F491_4F6C_DD1Du64;
    let mut rnd = move || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        (seed >> 11) as f64 / (1u64 << 53) as f64
    };
    let m = Space::Model;
    let mut regions = Vec::new();
    let mut index = Vec::new();
    for sh in 0..sheets {
        let x0 = sh as f64 * w * 1.05;
        let at = |x: f64, y: f64| Vec2::new(x0 + x, y);
        let number = format!("Q-{:03}", sh + 1);
        let title = format!("Sample Sheet {}", sh + 1);
        let mut ents: Vec<(Common, EntityKind)> = vec![
            (on("G-BORD"), lwpoly(rect_vertices(at(0.0, 0.0), at(w, h)), true)),
            (on("G-BORD"), lwpoly(rect_vertices(at(24.0, 24.0), at(w - 24.0, h - 24.0)), true)),
            (on("G-BORD"), lwpoly(rect_vertices(at(w - 300.0, 24.0), at(w - 24.0, h - 24.0)), true)),
            (on("G-BORD"), text(at(w - 280.0, 60.0), 24.0, &number)),
            (on("G-BORD"), text(at(w - 280.0, 110.0), 12.0, &title)),
        ];
        // The plan area left of the title block.
        let (pw, ph) = (w - 360.0, h - 72.0);
        while ents.len() < per_sheet.max(6) {
            let (x, y) = (48.0 + rnd() * (pw - 96.0), 48.0 + rnd() * (ph - 96.0));
            let k = ents.len() % 20;
            let e = match k {
                0..=8 => {
                    // A wall run: a pair of faces 6" apart.
                    let len = 24.0 + rnd() * 240.0;
                    let across = k.is_multiple_of(2);
                    let (dx, dy) = if across { (len, 0.0) } else { (0.0, len) };
                    let off = if across { Vec2::new(0.0, 6.0) } else { Vec2::new(6.0, 0.0) };
                    let a = at(x, y) + if k > 4 { off } else { Vec2::ZERO };
                    (on("A-WALL"), line(a, a + Vec2::new(dx.min(pw - x), dy.min(ph - y))))
                }
                9..=11 => {
                    let (rw, rh) = (60.0 + rnd() * 180.0, 60.0 + rnd() * 120.0);
                    (on("A-ROOM"), lwpoly(rect_vertices(at(x, y), at((x + rw).min(pw), (y + rh).min(ph))), true))
                }
                12 => {
                    let b = at((x + 30.0 + rnd() * 60.0).min(pw), (y + 30.0 + rnd() * 60.0).min(ph));
                    let solid = rnd() < 0.5;
                    (
                        on("A-PATT"),
                        EntityKind::Hatch(Box::new(Hatch {
                            pattern: if solid { "SOLID".into() } else { "ANSI31".into() },
                            solid,
                            loops: vec![HatchLoop { curves: Vec::new(), vertices: rect_vertices(at(x, y), b), outer: true, sources: Vec::new() }],
                            scale: 12.0,
                            angle: 0.0,
                            associative: false,
                            style: 0,
                            elevation: 0.0,
                            gradient: None,
                            origin: Vec2::ZERO,
                            background: None,
                            pattern_lines: Vec::new(),
                        })),
                    )
                }
                13..=15 => (
                    on("A-ANNO"),
                    text(
                        at(x, y),
                        6.0 + rnd() * 6.0,
                        if k == 13 {
                            "ROOM"
                        } else if k == 14 {
                            "101"
                        } else {
                            "SEE NOTE 4"
                        },
                    ),
                ),
                16 => (on("A-ANNO"), circle(at(x, y), 6.0 + rnd() * 18.0)),
                _ => (
                    on("E-SYMB"),
                    EntityKind::Insert(cadaddy_doc::Insert {
                        block: "SYM-OUTLET".into(),
                        insert: v3(at(x, y)),
                        scale: Vec3::new(1.0, 1.0, 1.0),
                        rotation: if rnd() < 0.5 { 0.0 } else { std::f64::consts::FRAC_PI_2 },
                        attribs: Vec::new(),
                        cols: 1,
                        rows: 1,
                        col_spacing: 0.0,
                        row_spacing: 0.0,
                        clip: None,
                    }),
                ),
            };
            ents.push(e);
        }
        for (c, k) in ents {
            let _ = d.add(&m, c, k);
        }
        regions.push(serde_json::json!({ "number": number, "level": "", "rect": [x0, 0.0, x0 + w, h] }));
        index.push(serde_json::json!({ "number": number, "title": title }));
    }
    d.agent = Some(serde_json::json!({ "guided_job": { "sheet_regions": regions, "index": { "sheets": index } } }).to_string()).into();
    d
}

/// Objects [`vector_print_set`] draws on each sheet (about).
pub const VECTOR_PRINT_SET_OBJECTS_PER_SHEET: usize = 10_000;

/// A synthetic print set: `sheets` vector sheets (at most 400) side by side in model space, as an
/// imported set of plans lies, about [`VECTOR_PRINT_SET_OBJECTS_PER_SHEET`] objects each in a mix like
/// a real plan's: walls and fixtures as short lines, polylines and arcs, solid and pattern
/// hatches, room tags, keynotes, general notes and a schedule as text, device symbols as block
/// references, dimensions. Each sheet is a 36" x 24" sheet at 1/4" = 1'-0" (1728 x 1152 units),
/// ten sheets to a row. Made-up content only; the same sheets every time (for timing the canvas:
/// 100 sheets make about a million objects).
pub fn vector_print_set(sheets: usize) -> Drawing {
    let sheets = sheets.clamp(1, 400);
    let mut d = Drawing::new_imperial();
    d.linetypes.extend(cadaddy_doc::library::standard_linetypes());
    d.layers.extend([
        layer("BORDER", 7, "Continuous", 50),
        layer("WALL", 7, "Continuous", 35),
        layer("DOOR", 3, "Continuous", 18),
        layer("FIXTURE", 8, "Continuous", 13),
        layer("DETAIL", 9, "Continuous", 9),
        layer("HATCH", 252, "Continuous", 0),
        layer("ANNO", 2, "Continuous", 18),
        layer("GRID", 1, "CENTER", 13),
        layer("DEVICE", 5, "Continuous", 18),
        layer("DIM", 4, "Continuous", 13),
    ]);
    d.header.set_f64("DIMSCALE", 48.0);
    for st in &mut d.dim_styles {
        st.scale = 48.0;
    }
    // Device symbols (in paper inches times the plot scale).
    let symbols: [(&str, Vec<EntityKind>); 4] = [
        (
            "SYM-OUTLET",
            vec![circle(Vec2::ZERO, 4.0), line(Vec2::new(-2.0, -1.5), Vec2::new(-2.0, 1.5)), line(Vec2::new(2.0, -1.5), Vec2::new(2.0, 1.5))],
        ),
        (
            "SYM-LIGHT",
            vec![circle(Vec2::ZERO, 6.0), line(Vec2::new(-4.2, -4.2), Vec2::new(4.2, 4.2)), line(Vec2::new(-4.2, 4.2), Vec2::new(4.2, -4.2))],
        ),
        ("SYM-SWITCH", vec![text(Vec2::new(-2.0, -3.0), 6.0, "S"), line(Vec2::new(0.0, -6.0), Vec2::new(0.0, -12.0))]),
        (
            "SYM-DATA",
            vec![lwpoly(
                [Vec2::new(-4.0, -3.0), Vec2::new(4.0, -3.0), Vec2::new(0.0, 4.0)]
                    .into_iter()
                    .map(|p| PolyVertex { p, bulge: 0.0, start_width: 0.0, end_width: 0.0 })
                    .collect(),
                true,
            )],
        ),
    ];
    for (name, kinds) in symbols {
        let mut b = cadaddy_doc::Block::new(name);
        for k in kinds {
            let h = d.new_handle();
            b.entities.push(cadaddy_doc::Entity { handle: h, common: on("0"), kind: k });
        }
        d.blocks.insert(name.into(), std::sync::Arc::new(b));
    }
    let m = Space::Model;
    let mut add = |c: &str, k: EntityKind| {
        let _ = d.add(&m, on(c), k);
    };
    let insert = |block: &str, at: Vec2, rotation: f64| {
        EntityKind::Insert(cadaddy_doc::Insert {
            block: block.into(),
            insert: v3(at),
            scale: Vec3::new(1.0, 1.0, 1.0),
            rotation,
            attribs: Vec::new(),
            cols: 1,
            rows: 1,
            col_spacing: 0.0,
            row_spacing: 0.0,
            clip: None,
        })
    };
    let hatch = |a: Vec2, b: Vec2, solid: bool| {
        EntityKind::Hatch(Box::new(Hatch {
            pattern: if solid { "SOLID".into() } else { "ANSI31".into() },
            solid,
            loops: vec![HatchLoop { curves: Vec::new(), vertices: rect_vertices(a, b), outer: true, sources: Vec::new() }],
            scale: 48.0,
            angle: 0.0,
            associative: false,
            style: 0,
            elevation: 0.0,
            gradient: None,
            origin: Vec2::ZERO,
            background: None,
            pattern_lines: Vec::new(),
        }))
    };
    // A small deterministic generator for the variation between rooms.
    let mut seed = 0x2545_f491_4f6c_dd1du64;
    let mut rnd = move || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        (seed >> 11) as f64 / (1u64 << 53) as f64
    };
    let (w, h) = (36.0 * 48.0, 24.0 * 48.0);
    let (cols, rows) = (10usize, 6usize);
    for sheet in 0..sheets {
        let o = Vec2::new((sheet % 10) as f64 * w * 1.125, -((sheet / 10) as f64) * h * 1.125);
        let at = |x: f64, y: f64| o + Vec2::new(x, y);
        // Border and title block.
        add("BORDER", lwpoly(rect_vertices(at(0.0, 0.0), at(w, h)), true));
        add("BORDER", lwpoly(rect_vertices(at(24.0, 24.0), at(w - 24.0, h - 24.0)), true));
        let tb = w - 24.0 - 3.5 * 48.0;
        add("BORDER", lwpoly(rect_vertices(at(tb, 24.0), at(w - 24.0, h - 24.0)), true));
        for i in 1..=12 {
            let y = 24.0 + f64::from(i) * 60.0;
            add("BORDER", line(at(tb, y), at(w - 24.0, y)));
        }
        for i in 0..10 {
            add("ANNO", text(at(tb + 12.0, 40.0 + f64::from(i) * 60.0), 9.0, &format!("TITLE BLOCK FIELD {}", i + 1)));
        }
        add("ANNO", text(at(tb + 12.0, 24.0 + 13.0 * 60.0), 24.0, &format!("SHEET {} OF {sheets}", sheet + 1)));
        // Plan area: a grid of rooms, the notes and schedule beside them.
        let (px, py, pw, ph) = (72.0, 96.0, tb - 72.0 - 380.0, h - 96.0 - 72.0);
        let (rw, rh) = (pw / cols as f64, ph / rows as f64);
        for c in 0..=cols {
            let x = px + c as f64 * rw;
            add("GRID", line(at(x, py - 36.0), at(x, py + ph + 24.0)));
            add("GRID", circle(at(x, py - 48.0), 12.0));
            add("ANNO", text(at(x - 4.0, py - 52.0), 9.0, &(c + 1).to_string()));
        }
        for r in 0..rows {
            for c in 0..cols {
                let (x0, y0) = (px + c as f64 * rw, py + r as f64 * rh);
                let (x1, y1) = (x0 + rw, y0 + rh);
                let t = 6.0;
                // Walls: both faces of each side.
                for (a, b) in [((x0, y0), (x1, y0)), ((x1, y0), (x1, y1)), ((x1, y1), (x0, y1)), ((x0, y1), (x0, y0))] {
                    add("WALL", line(at(a.0, a.1), at(b.0, b.1)));
                }
                for (a, b) in [
                    ((x0 + t, y0 + t), (x1 - t, y0 + t)),
                    ((x1 - t, y0 + t), (x1 - t, y1 - t)),
                    ((x1 - t, y1 - t), (x0 + t, y1 - t)),
                    ((x0 + t, y1 - t), (x0 + t, y0 + t)),
                ] {
                    add("WALL", line(at(a.0, a.1), at(b.0, b.1)));
                }
                // A door: leaf and swing.
                let hinge = at(x0 + t + 6.0, y0 + t);
                add("DOOR", line(hinge, hinge + Vec2::Y * 36.0));
                add("DOOR", arc(&cadaddy_geom::Arc::new(hinge, 36.0, 0.0, std::f64::consts::FRAC_PI_2)));
                // Room tag.
                let mid = at((x0 + x1) / 2.0, (y0 + y1) / 2.0);
                add("ANNO", lwpoly(rect_vertices(mid - Vec2::new(30.0, 14.0), mid + Vec2::new(30.0, 14.0)), true));
                add("ANNO", text(mid + Vec2::new(-26.0, 2.0), 9.0, "ROOM"));
                add("ANNO", text(mid + Vec2::new(-26.0, -11.0), 9.0, &format!("{}{:02}", r + 1, c + 1)));
                // Floor finish: solid, pattern or none.
                match (r * cols + c) % 3 {
                    0 => add("HATCH", hatch(at(x0 + t, y0 + t), at(x0 + rw * 0.4, y0 + rh * 0.4), true)),
                    1 => add("HATCH", hatch(at(x1 - rw * 0.35, y1 - rh * 0.35), at(x1 - t, y1 - t), false)),
                    _ => {}
                }
                // Device symbols along the walls.
                for i in 0..12 {
                    let f = (f64::from(i) + 0.5) / 12.0;
                    let block = ["SYM-OUTLET", "SYM-LIGHT", "SYM-SWITCH", "SYM-DATA"][(i % 4) as usize];
                    let p = if i % 2 == 0 {
                        at(x0 + t + 8.0 + f * (rw - 2.0 * t - 16.0), y0 + t + 8.0)
                    } else {
                        at(x0 + rw * 0.5 + (f - 0.5) * rw * 0.6, y0 + rh * 0.62)
                    };
                    add("DEVICE", insert(block, p, 0.0));
                }
                // Fixtures and furniture: outlines and short lines.
                for _ in 0..6 {
                    let a = at(x0 + 20.0 + rnd() * (rw - 80.0), y0 + 20.0 + rnd() * (rh - 80.0));
                    add("FIXTURE", lwpoly(rect_vertices(a, a + Vec2::new(18.0 + rnd() * 30.0, 18.0 + rnd() * 30.0)), true));
                }
                for _ in 0..3 {
                    add("FIXTURE", circle(at(x0 + 30.0 + rnd() * (rw - 60.0), y0 + 30.0 + rnd() * (rh - 60.0)), 6.0 + rnd() * 10.0));
                }
                for _ in 0..25 {
                    let a = at(x0 + 12.0 + rnd() * (rw - 24.0), y0 + 12.0 + rnd() * (rh - 24.0));
                    add("FIXTURE", line(a, a + Vec2::new(rnd() * 24.0 - 12.0, rnd() * 24.0 - 12.0)));
                }
                // Detail strokes, as a print's fills and textures arrive in an imported set.
                for i in 0..70 {
                    let y = y0 + t + (f64::from(i) + 0.5) * (rh * 0.3 - t) / 70.0;
                    let x = x1 - rw * 0.3;
                    add("DETAIL", line(at(x, y), at(x + rw * 0.3 - t, y + 4.0)));
                }
                // Keynote tags.
                for i in 0..10 {
                    add("ANNO", text(at(x0 + 14.0 + f64::from(i) * (rw - 28.0) / 10.0, y1 - t - 14.0), 6.0, &(i + 1).to_string()));
                }
            }
        }
        // General notes: a paragraph and single lines.
        let nx = px + pw + 24.0;
        add(
            "ANNO",
            EntityKind::MText(MText {
                background: None,
                insert: v3(at(nx, py + ph)),
                height: 9.0,
                width: 340.0,
                attach: 1,
                rotation: 0.0,
                style: "Standard".into(),
                contents: "GENERAL NOTES:\\P1. VERIFY ALL DIMENSIONS IN THE FIELD.\\P2. COORDINATE WITH OTHER TRADES BEFORE ROUGH-IN.\\P3. SEE SCHEDULES FOR TYPES.".into(),
                line_spacing: 1.0,
                line_spacing_exact: false,
                columns: None,
            }),
        );
        for i in 0..300 {
            let (col, row) = (i / 100, i % 100);
            add("ANNO", text(at(nx + f64::from(col) * 115.0, py + ph - 60.0 - f64::from(row) * 5.0), 4.0, &format!("{}. NOTE TEXT LINE", i + 1)));
        }
        // A schedule: its rules and cells.
        let (sx, sy) = (nx, py + 20.0);
        for i in 0..=20 {
            add("BORDER", line(at(sx, sy + f64::from(i) * 10.0), at(sx + 340.0, sy + f64::from(i) * 10.0)));
        }
        for i in 0..=19 {
            add("BORDER", line(at(sx + f64::from(i) * 340.0 / 19.0, sy), at(sx + f64::from(i) * 340.0 / 19.0, sy + 200.0)));
        }
        for r in 0..20 {
            for c in 0..19 {
                add("ANNO", text(at(sx + 2.0 + f64::from(c) * 340.0 / 19.0, sy + 2.5 + f64::from(r) * 10.0), 4.5, &format!("{}", r * 19 + c + 1)));
            }
        }
        // Dimensions along the grid.
        for c in 0..cols {
            let x = px + c as f64 * rw;
            add("DIM", dim(DimKind::Linear { rotation: 0.0 }, at(x, py + ph + 24.0), at(x + rw, py + ph + 24.0), at(x + rw / 2.0, py + ph + 40.0)));
        }
        for r in 0..rows {
            let y = py + r as f64 * rh;
            add(
                "DIM",
                dim(DimKind::Linear { rotation: std::f64::consts::FRAC_PI_2 }, at(px - 36.0, y), at(px - 36.0, y + rh), at(px - 52.0, y + rh / 2.0)),
            );
        }
        // Enlarged detail: dense short strokes.
        for i in 0..900 {
            let a = at(nx + f64::from(i % 30) * 11.0, py + 215.0 + f64::from(i / 30) * 6.5);
            add("DETAIL", line(a, a + Vec2::new(8.0, 5.0)));
        }
    }
    d
}

/// The sample used by `--sample`.
pub fn default_sample() -> Drawing {
    bracket()
}

pub fn _unused() -> EntityKind {
    line(Vec2::ZERO, Vec2::X)
}

#[cfg(test)]
mod vector_print_set_tests {
    use super::*;

    #[test]
    fn a_print_set_has_its_sheets_side_by_side_in_a_plan_mix() {
        let d = vector_print_set(3);
        let n = d.model.len();
        assert!((27_000..33_000).contains(&n), "{n} objects for 3 sheets");
        let count = |f: fn(&EntityKind) -> bool| d.model.iter().filter(|e| f(&e.kind)).count();
        let lines = count(|k| matches!(k, EntityKind::Line(_) | EntityKind::LwPolyline(_) | EntityKind::Arc(_) | EntityKind::Circle(_)));
        let texts = count(|k| matches!(k, EntityKind::Text(_) | EntityKind::MText(_)));
        let hatches = count(|k| matches!(k, EntityKind::Hatch(_)));
        let symbols = count(|k| matches!(k, EntityKind::Insert(_)));
        let dims = count(|k| matches!(k, EntityKind::Dimension(_)));
        assert!(lines > n / 2 && texts > n / 10 && symbols > n / 20, "{lines} lines, {texts} texts, {symbols} symbols of {n}");
        assert!(hatches >= 3 * 40 && dims == 3 * 16, "{hatches} hatches, {dims} dimensions");
        // Every symbol's block is there; the sheets lie side by side, a row of ten.
        assert!(d.model.iter().all(|e| match &e.kind {
            EntityKind::Insert(i) => d.block(&i.block).is_some(),
            _ => true,
        }));
        let ext = d.extents(&Space::Model);
        assert!(ext.width() > 2.9 * 1728.0 && ext.height() < 1.1 * 1152.0, "{ext:?}");
        assert!(vector_print_set(12).extents(&Space::Model).height() > 2.0 * 1152.0);
    }

    #[test]
    fn a_print_set_is_the_same_every_time() {
        let (a, b) = (vector_print_set(1), vector_print_set(1));
        assert_eq!(a.model.len(), b.model.len());
        assert!(a.model.iter().zip(b.model.iter()).all(|(x, y)| x.kind == y.kind));
        // At least one sheet.
        assert_eq!(vector_print_set(0).model.len(), a.model.len());
    }
}
