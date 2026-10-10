use crate::raster::{RasterOptions, View, render};
use crate::*;
use cadcraft_doc::{Common, Drawing, EntityKind, Hatch, HatchLoop, Space};
use cadcraft_geom::{PolyVertex, Vec3};

fn sample() -> Drawing {
    let mut d = Drawing::new_imperial();
    d.add(&Space::Model, Common::default(), EntityKind::Line(cadcraft_doc::Line { a: Vec3::new(0.0, 0.0, 0.0), b: Vec3::new(10.0, 0.0, 0.0) }))
        .unwrap();
    d.add(
        &Space::Model,
        Common { color: Color::Index(1), ..Default::default() },
        EntityKind::Circle(cadcraft_doc::Circle { center: Vec3::new(5.0, 5.0, 0.0), radius: 3.0 }),
    )
    .unwrap();
    d
}

#[test]
fn builds_lines_and_circles() {
    let d = sample();
    let l = build(&d, &Space::Model, &Options { tolerance: 0.01, ..Default::default() });
    assert_eq!(l.prims.len(), 2);
    assert!(l.prims[1].len > 20);
    assert_eq!(l.prims[1].color, Rgb(255, 0, 0));
}

#[test]
fn linetype_dashes_line() {
    let mut d = sample();
    d.linetypes.extend(cadcraft_doc::library::standard_linetypes());
    d.layer_mut("0").unwrap().linetype = "DASHED".into();
    let l = build(&d, &Space::Model, &Options::default());
    assert!(l.prims.len() > 10);
}

#[test]
fn frozen_layer_hidden() {
    let mut d = sample();
    d.layer_mut("0").unwrap().frozen = true;
    assert!(build(&d, &Space::Model, &Options::default()).prims.is_empty());
}

#[test]
fn text_produces_strokes() {
    let mut d = Drawing::new_imperial();
    d.add(
        &Space::Model,
        Common::default(),
        EntityKind::Text(cadcraft_doc::Text {
            insert: Vec3::ZERO,
            align_pt: None,
            height: 1.0,
            value: "CAD".into(),
            rotation: 0.0,
            width_factor: 1.0,
            oblique: 0.0,
            style: "Standard".into(),
            halign: Default::default(),
            valign: Default::default(),
        }),
    )
    .unwrap();
    let l = build(&d, &Space::Model, &Options::default());
    assert!(l.prims.len() >= 3);
}

#[test]
fn hatch_pattern_and_solid() {
    let mut d = Drawing::new_imperial();
    let sq = vec![
        PolyVertex::new(Vec2::new(0.0, 0.0)),
        PolyVertex::new(Vec2::new(4.0, 0.0)),
        PolyVertex::new(Vec2::new(4.0, 4.0)),
        PolyVertex::new(Vec2::new(0.0, 4.0)),
    ];
    let h = Hatch {
        pattern: "ANSI31".into(),
        solid: false,
        loops: vec![HatchLoop { vertices: sq.clone(), outer: true }],
        scale: 1.0,
        angle: 0.0,
        associative: false,
        style: 0,
        elevation: 0.0,
        gradient: None,
        origin: Vec2::ZERO,
        background: None,
        pattern_lines: Vec::new(),
    };
    d.add(&Space::Model, Common::default(), EntityKind::Hatch(h.clone())).unwrap();
    let l = build(&d, &Space::Model, &Options::default());
    // 45° lines at 0.125 spacing across a 4x4 square: about 4*sqrt(2)/0.125 ≈ 45 lines.
    assert!(l.prims.len() > 35 && l.prims.len() < 60, "{}", l.prims.len());
    let mut d2 = Drawing::new_imperial();
    d2.add(&Space::Model, Common::default(), EntityKind::Hatch(Hatch { solid: true, pattern: "SOLID".into(), ..h })).unwrap();
    let l2 = build(&d2, &Space::Model, &Options::default());
    assert_eq!(l2.prims.len(), 1);
    assert_eq!(l2.prims[0].kind, Kind::Tris);
}

#[test]
fn raster_draws_pixels() {
    let d = sample();
    let l = build(&d, &Space::Model, &Options::default());
    let v = View::fit(&l.bounds, 200, 200, 0.1);
    let pm = render(&l, &v, &RasterOptions::default()).unwrap();
    let bg = pm.pixels().iter().filter(|p| p.red() == 33).count();
    assert!(bg < 200 * 200);
    // A red pixel exists from the circle.
    assert!(pm.pixels().iter().any(|p| p.red() > 200 && p.green() < 60));
}

#[test]
fn raster_rejects_bad_size() {
    let l = DisplayList::default();
    assert!(render(&l, &View { center: Vec2::ZERO, scale: 1.0, width: 0, height: 10 }, &RasterOptions::default()).is_none());
}

fn layout_with_viewport(vp: cadcraft_doc::Viewport) -> Drawing {
    let mut d = sample(); // line (0,0)-(10,0) and red circle at (5,5) r3
    let paper = Space::Paper("Layout1".into());
    d.add(&paper, Common::default(), EntityKind::Viewport(vp)).unwrap();
    d
}

fn vp(center: Vec2, w: f64, h: f64, view_center: Vec2, view_height: f64, id: u32) -> cadcraft_doc::Viewport {
    cadcraft_doc::Viewport {
        center: center.to3(0.0),
        width: w,
        height: h,
        view_center,
        view_height,
        id,
        locked: false,
        frozen_layers: Vec::new(),
        layer_colors: Vec::new(),
    }
}

#[test]
fn layout_viewport_shows_model_clipped() {
    // Viewport 4x4 paper units centred at (5,4), showing model window centred at (5,5), 8 high:
    // scale 0.5, so model (5,5) → paper (5,4); the visible model window is (1,1)..(9,9).
    let d = layout_with_viewport(vp(Vec2::new(5.0, 4.0), 4.0, 4.0, Vec2::new(5.0, 5.0), 8.0, 2));
    let l = build(&d, &Space::Paper("Layout1".into()), &Options { tolerance: 0.01, ..Default::default() });
    let sheet = l.sheet.unwrap();
    assert!(sheet.size.x > sheet.size.y, "landscape sheet");
    let rect = Bounds2::new(Vec2::new(3.0, 2.0), Vec2::new(7.0, 6.0)).expand(1e-9);
    let content: Vec<&DPrim> = l.prims.iter().filter(|p| p.handle == VIEWPORT_CONTENT).collect();
    assert!(!content.is_empty(), "model geometry is drawn in the viewport");
    for p in &content {
        for q in l.points(p) {
            assert!(rect.contains(*q), "{q:?} outside the viewport");
        }
    }
    // The circle (red) is visible, scaled by 0.5 about the view centre: radius 1.5 around (5,4).
    let red: Vec<Vec2> = content.iter().filter(|p| p.color == Rgb(255, 0, 0)).flat_map(|p| l.points(p).to_vec()).collect();
    assert!(!red.is_empty());
    assert!(red.iter().all(|q| (q.dist(Vec2::new(5.0, 4.0)) - 1.5).abs() < 0.02));
    // The line y=0 maps to paper y=1.5, below the viewport: clipped away entirely.
    assert!(content.iter().filter(|p| p.color != Rgb(255, 0, 0)).all(|p| l.points(p).iter().all(|q| q.y > 1.9)));
    // The border is drawn with the viewport's own handle.
    assert!(l.prims.iter().any(|p| p.handle != VIEWPORT_CONTENT));
}

#[test]
fn layout_viewport_frozen_layer_and_id1() {
    let mut v = vp(Vec2::new(5.0, 4.0), 20.0, 20.0, Vec2::new(5.0, 5.0), 20.0, 2);
    v.frozen_layers = vec!["0".into()];
    let d = layout_with_viewport(v);
    let l = build(&d, &Space::Paper("Layout1".into()), &Options::default());
    assert!(l.prims.iter().all(|p| p.handle != VIEWPORT_CONTENT), "layer 0 is frozen in this viewport");
    // Viewport id 1 (the paper-space view) draws nothing.
    let d = layout_with_viewport(vp(Vec2::new(5.0, 4.0), 20.0, 20.0, Vec2::new(5.0, 5.0), 20.0, 1));
    assert!(build(&d, &Space::Paper("Layout1".into()), &Options::default()).prims.is_empty());
    // Model space never draws viewport contents; plotting skips non-plot layers.
    let mut d = sample();
    d.layer_mut("0").unwrap().plot = false;
    assert!(build_plot(&d, &Space::Model, &Options::default()).prims.is_empty());
    assert!(!build(&d, &Space::Model, &Options::default()).prims.is_empty());
}

#[test]
fn hostile_viewports_do_not_panic() {
    for v in [
        vp(Vec2::new(f64::NAN, 0.0), 1.0, 1.0, Vec2::ZERO, 1.0, 2),
        vp(Vec2::ZERO, -1.0, 1.0, Vec2::ZERO, 1.0, 2),
        vp(Vec2::ZERO, 1.0, 1.0, Vec2::ZERO, 0.0, 2),
        vp(Vec2::ZERO, 1e308, 1e308, Vec2::new(1e308, -1e308), 1e-308, 2),
        vp(Vec2::ZERO, 1.0, 1.0, Vec2::new(f64::INFINITY, 0.0), 1.0, 2),
    ] {
        let d = layout_with_viewport(v);
        let _ = build(&d, &Space::Paper("Layout1".into()), &Options::default());
    }
}

fn text_doc(style_font: &str, value: &str) -> Drawing {
    let mut d = Drawing::new_imperial();
    d.text_styles.push(cadcraft_doc::TextStyle { name: "TT".into(), font: style_font.into(), ..Default::default() });
    d.add(
        &Space::Model,
        Common::default(),
        EntityKind::Text(cadcraft_doc::Text {
            insert: Vec3::ZERO,
            align_pt: None,
            height: 1.0,
            value: value.into(),
            rotation: 0.0,
            width_factor: 1.0,
            oblique: 0.0,
            style: "TT".into(),
            halign: Default::default(),
            valign: Default::default(),
        }),
    )
    .unwrap();
    d
}

/// The first common system TrueType font, if any is installed (tests skip otherwise).
fn system_font() -> Option<&'static str> {
    ["Arial", "Helvetica", "DejaVuSans", "Verdana", "LiberationSans-Regular"].into_iter().find(|n| cadcraft_fonts::ttf::find(n).is_some())
}

#[test]
fn truetype_text_is_filled_or_outlined() {
    let Some(font) = system_font() else { return };
    let mut d = text_doc(font, "OH");
    let l = build(&d, &Space::Model, &Options::default());
    assert!(l.prims.iter().any(|p| p.kind == Kind::Tris), "TEXTFILL=1 fills glyphs");
    // The O's counter is a hole: filled area is well below the glyph box area.
    let area: f64 = l
        .prims
        .iter()
        .filter(|p| p.kind == Kind::Tris)
        .flat_map(|p| l.points(p).chunks(3).map(|t| ((t[1] - t[0]).cross(t[2] - t[0]) / 2.0).abs()).collect::<Vec<_>>())
        .sum();
    assert!(area > 0.1 && area < l.bounds.width() * l.bounds.height() * 0.8, "area {area}");
    assert!((l.bounds.max.y - 1.0).abs() < 0.1, "cap height = text height");
    d.header.set_i64("TEXTFILL", 0);
    let l = build(&d, &Space::Model, &Options::default());
    assert!(l.prims.iter().all(|p| p.kind == Kind::Polyline), "TEXTFILL=0 outlines");
    assert!(l.prims.len() >= 3, "O has two contours, H one");
}

#[test]
fn missing_font_falls_back_to_stroke() {
    let d = text_doc("NoSuchFontAnywhere.ttf", "OH");
    let l = build(&d, &Space::Model, &Options::default());
    assert!(!l.prims.is_empty() && l.prims.iter().all(|p| p.kind == Kind::Polyline));
}

#[test]
fn backwards_and_upside_down_mirror_text() {
    let mut d = text_doc("CADCraft Stroke", "ABC");
    let normal = build(&d, &Space::Model, &Options::default()).bounds;
    assert!(normal.min.x >= -1e-9 && normal.min.y >= -1e-9);
    d.text_styles.iter_mut().for_each(|s| s.backwards = true);
    let back = build(&d, &Space::Model, &Options::default()).bounds;
    assert!(back.max.x <= 1e-9 && (back.width() - normal.width()).abs() < 1e-9, "mirrored to the left");
    d.text_styles.iter_mut().for_each(|s| {
        s.backwards = false;
        s.upside_down = true;
    });
    let up = build(&d, &Space::Model, &Options::default()).bounds;
    assert!(up.max.y <= 1e-9 && up.min.y < -0.9, "mirrored below the baseline");
}

#[test]
fn truetype_mtext_and_dimension_text() {
    let Some(font) = system_font() else { return };
    let mut d = Drawing::new_imperial();
    d.text_styles.iter_mut().for_each(|s| s.font = font.into());
    d.add(
        &Space::Model,
        Common::default(),
        EntityKind::MText(cadcraft_doc::MText {
            insert: Vec3::ZERO,
            height: 1.0,
            width: 0.0,
            attach: 1,
            rotation: 0.0,
            style: "Standard".into(),
            contents: "A{\\C1;B}\\P\\S1/2;".into(),
            line_spacing: 1.0,
        }),
    )
    .unwrap();
    let l = build(&d, &Space::Model, &Options::default());
    assert!(l.prims.iter().any(|p| p.kind == Kind::Tris && p.color == Rgb(255, 0, 0)), "red B");
    assert!(l.prims.iter().any(|p| p.kind == Kind::Tris && p.color != Rgb(255, 0, 0)));
    let mut d = Drawing::new_imperial();
    d.text_styles.iter_mut().for_each(|s| s.font = font.into());
    d.add(&Space::Model, Common::default(), EntityKind::Dimension(lin_dim())).unwrap();
    let l = build(&d, &Space::Model, &Options::default());
    assert!(l.prims.iter().filter(|p| p.kind == Kind::Tris).count() > 2, "arrows plus filled glyphs");
}

fn lin_dim() -> cadcraft_doc::Dimension {
    cadcraft_doc::Dimension {
        kind: cadcraft_doc::DimKind::Linear { rotation: 0.0 },
        defpt: Vec3::new(0.0, 2.0, 0.0),
        text_mid: Vec3::ZERO,
        p13: Vec3::ZERO,
        p14: Vec3::new(10.0, 0.0, 0.0),
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
    }
}

#[test]
fn dimension_colours_by_role() {
    let mut d = Drawing::new_imperial();
    if let Some(st) = d.dim_styles.first_mut() {
        st.dim_line_color = Color::Index(1);
        st.ext_line_color = Color::Index(3);
        st.text_color = Color::Index(5);
    }
    d.add(&Space::Model, Common::default(), EntityKind::Dimension(lin_dim())).unwrap();
    let l = build(&d, &Space::Model, &Options::default());
    let has = |c: Rgb, k: Kind| l.prims.iter().any(|p| p.color == c && p.kind == k);
    assert!(has(Rgb(255, 0, 0), Kind::Polyline) && has(Rgb(255, 0, 0), Kind::Tris), "dimension line and arrows red");
    assert!(has(Rgb(0, 255, 0), Kind::Polyline), "extension lines green");
    assert!(has(Rgb(0, 0, 255), Kind::Polyline), "text blue");
    // ByBlock (the default) takes the dimension's own colour.
    let mut d = Drawing::new_imperial();
    d.add(&Space::Model, Common { color: Color::Index(6), ..Default::default() }, EntityKind::Dimension(lin_dim())).unwrap();
    let l = build(&d, &Space::Model, &Options::default());
    assert!(l.prims.iter().all(|p| p.color == Rgb(255, 0, 255)));
}

#[test]
fn table_title_is_centred_and_merged() {
    let mut d = Drawing::new_imperial();
    let cell = |t: &str| cadcraft_doc::TableCell { text: t.into(), merged: None };
    let mut title = vec![cell("TITLE"), cell(""), cell("")];
    title[0].merged = Some((1, 3));
    let t = cadcraft_doc::Table {
        insert: Vec3::new(0.0, 0.0, 0.0),
        col_widths: vec![2.0, 2.0, 2.0],
        row_heights: vec![1.0, 1.0],
        cells: vec![title, vec![cell("a"), cell("b"), cell("c")]],
        style: "Standard".into(),
        text_height: 0.2,
        title: true,
        header: false,
    };
    d.add(&Space::Model, Common::default(), EntityKind::Table(t)).unwrap();
    let l = build(&d, &Space::Model, &Options::default());
    // No vertical cell borders cross the merged title row (y in 0..-1) except the outer ones.
    let inner_vertical_in_title = l.prims.iter().filter(|p| p.kind == Kind::Polyline).any(|p| {
        l.points(p).windows(2).any(|w| {
            (w[0].x - w[1].x).abs() < 1e-12
                && (w[0].y - w[1].y).abs() > 0.5
                && w[0].x > 0.1
                && w[0].x < 5.9
                && w[0].y.max(w[1].y) > -0.99
                && w[0].y.min(w[1].y) < -0.01
        })
    });
    assert!(!inner_vertical_in_title);
    // Title text centred on the table width.
    let text: Vec<Vec2> = l
        .prims
        .iter()
        .filter(|p| p.kind == Kind::Polyline)
        .flat_map(|p| l.points(p).to_vec())
        .filter(|q| q.y > -0.95 && q.y < -0.05 && q.x > 0.05 && q.x < 5.95)
        .collect();
    let b = Bounds2::from_points(text);
    assert!((b.center().x - 3.0).abs() < 0.05, "{b:?}");
}
