//! SVG import, the rest of the static subset (gaps FF14): CSS classes in `<style>`, `use`
//! references, fills as solid and gradient hatches, text as TEXT/MTEXT, embedded and linked
//! images as IMAGE (an embedded one written out beside the SVG), and rounded rectangle corners.

use cadaddy_color::{Color, Rgb};
use cadaddy_doc::*;
use cadaddy_geom::Vec2;
use cadaddy_io::read;

/// A 20mm×10mm page whose user units are millimetres; the reader flips Y, so SVG y lands at
/// `10 - y` in the drawing.
const PAGE: &str =
    r#"<svg xmlns="http://www.w3.org/2000/svg" xmlns:xlink="http://www.w3.org/1999/xlink" width="20mm" height="10mm" viewBox="0 0 20 10">"#;

fn svg(body: &str) -> Drawing {
    read(format!("{PAGE}{body}</svg>").as_bytes(), "t.svg").unwrap()
}

fn kinds(d: &Drawing) -> Vec<&'static str> {
    d.model.iter().map(|e| e.kind.type_name()).collect()
}

fn all<T>(d: &Drawing, f: impl Fn(&Entity) -> Option<T>) -> Vec<T> {
    d.model.iter().filter_map(|e| f(e)).collect()
}

fn hatches(d: &Drawing) -> Vec<(Hatch, Color)> {
    all(d, |e| if let EntityKind::Hatch(h) = &e.kind { Some((h.clone(), e.common.color)) } else { None })
}

fn near(a: Vec2, b: Vec2) -> bool {
    a.dist(b) < 1e-9
}

#[test]
fn css_classes_and_fills() {
    let d = svg(r#"<style><![CDATA[ .red { fill: #f00 } /* note */ rect.wide { fill: blue } #g { fill: lime } .none { fill: none } ]]></style>
        <rect class="red" x="1" y="1" width="4" height="3"/>
        <rect class="red wide" x="6" y="1" width="4" height="3"/>
        <rect id="g" class="red" x="11" y="1" width="2" height="2"/>
        <rect x="1" y="6" width="2" height="2"/>
        <rect class="none" fill="red" x="4" y="6" width="2" height="2"/>
        <circle cx="10" cy="7" r="2" style="fill:rgb(0,128,255)"/>
        <g fill="yellow"><polygon points="14,6 18,6 16,9"/></g>"#);
    // A hatch first, then its outline; unstyled and `fill: none` shapes stay outlines only.
    assert_eq!(
        kinds(&d),
        ["Hatch", "Polyline", "Hatch", "Polyline", "Hatch", "Polyline", "Polyline", "Polyline", "Hatch", "Circle", "Hatch", "Polyline"]
    );
    let colors: Vec<Color> = hatches(&d).iter().map(|(_, c)| *c).collect();
    assert_eq!(
        colors,
        [Rgb(255, 0, 0), Rgb(0, 0, 255), Rgb(0, 255, 0), Rgb(0, 128, 255), Rgb(255, 255, 0)].map(Color::True),
        "class, compound selector, id; inline style; inherited from a group"
    );
    let (h, _) = &hatches(&d)[0];
    assert!(h.solid && h.gradient.is_none() && h.loops.len() == 1);
    let pts: Vec<Vec2> = h.loops[0].vertices.iter().map(|v| v.p).collect();
    assert_eq!(pts.len(), 4);
    assert!(near(pts[0], Vec2::new(1.0, 9.0)) && near(pts[2], Vec2::new(5.0, 6.0)), "{pts:?}");
    // The circle's fill is two half-circle bulges.
    let (c, _) = &hatches(&d)[3];
    let v = &c.loops[0].vertices;
    assert_eq!(v.len(), 2);
    assert!(v.iter().all(|x| (x.bulge.abs() - 1.0).abs() < 1e-9), "{v:?}");
}

#[test]
fn path_fills_keep_their_holes() {
    let d = svg(r#"<path fill="black" d="M1 1 H9 V9 H1 Z M3 3 H7 V7 H3 Z"/><path fill="black" d="M10 1 C14 1 14 5 10 5"/>"#);
    let hs = hatches(&d);
    assert_eq!(hs.len(), 2);
    assert_eq!(hs[0].0.loops.len(), 2, "outer boundary and hole");
    assert!(hs[0].0.loops[0].outer && !hs[0].0.loops[1].outer);
    // An open curve is filled as if closed, the curve flattened.
    assert!(hs[1].0.loops[0].vertices.len() > 4);
}

#[test]
fn use_references_with_their_positions_and_transforms() {
    let d = svg(r##"<defs><circle id="dot" cx="1" cy="1" r="0.5"/>
        <symbol id="sym" viewBox="0 0 10 10"><rect x="0" y="0" width="10" height="10"/></symbol></defs>
        <use href="#dot" x="4"/>
        <use xlink:href="#dot" x="1" transform="translate(10,2)"/>
        <g transform="scale(2)"><use href="#dot"/></g>
        <use href="#sym" x="2" y="2" width="4" height="2"/>
        <use href="#nothing"/>"##);
    let circles = all(&d, |e| if let EntityKind::Circle(c) = &e.kind { Some((c.center.xy(), c.radius)) } else { None });
    assert_eq!(circles.len(), 3);
    assert!(near(circles[0].0, Vec2::new(5.0, 9.0)), "{circles:?}");
    assert!(near(circles[1].0, Vec2::new(12.0, 7.0)), "use's x inside its transform: {circles:?}");
    assert!(near(circles[2].0, Vec2::new(2.0, 8.0)) && (circles[2].1 - 1.0).abs() < 1e-9, "{circles:?}");
    // The symbol's 10×10 viewBox fits 4×2 at 2,2: a 2×2 square, centred across the width.
    let sq = all(&d, |e| if let EntityKind::LwPolyline(p) = &e.kind { Some(p.vertices.iter().map(|v| v.p).collect::<Vec<_>>()) } else { None });
    assert_eq!(sq.len(), 1);
    assert!(near(sq[0][0], Vec2::new(3.0, 8.0)) && near(sq[0][2], Vec2::new(5.0, 6.0)), "{:?}", sq[0]);
}

#[test]
fn use_inherits_the_referencing_style() {
    let d = svg(r##"<defs><rect id="r" width="2" height="2"/></defs><use href="#r" fill="#123456"/><use href="#r" x="5"/>"##);
    let hs = hatches(&d);
    assert_eq!(hs.len(), 1);
    assert_eq!(hs[0].1, Color::True(Rgb(0x12, 0x34, 0x56)));
}

#[test]
fn reference_cycles_and_fan_outs_end() {
    // A use that contains itself.
    let d = read(format!(r##"{PAGE}<g id="a"><circle r="1"/><use href="#a"/></g></svg>"##).as_bytes(), "c.svg").unwrap();
    assert!(!kinds(&d).is_empty());
    assert!(d.read_notes.iter().any(|n| n.contains("refers back")), "{:?}", d.read_notes);
    // Ten levels of ten uses each would be 10^10 circles: refused, not drawn for ever.
    let mut body = String::from(r#"<defs><circle id="l0" r="1"/>"#);
    for i in 1..=10 {
        body.push_str(&format!("<g id=\"l{i}\">"));
        for _ in 0..10 {
            body.push_str(&format!("<use href=\"#l{}\"/>", i - 1));
        }
        body.push_str("</g>");
    }
    body.push_str(r##"</defs><use href="#l10"/>"##);
    assert!(read(format!("{PAGE}{body}</svg>").as_bytes(), "bomb.svg").is_err());
}

#[test]
fn rounded_rectangle_corners() {
    let d = svg(r#"<rect x="1" y="1" width="6" height="4" rx="1"/><rect x="10" y="1" width="2" height="2" ry="5"/>
        <rect x="14" y="1" width="4" height="4" rx="2" ry="1"/>"#);
    let polys = all(&d, |e| if let EntityKind::LwPolyline(p) = &e.kind { Some(p.clone()) } else { None });
    assert_eq!(polys.len(), 3);
    // Four straight sides and four quarter circles (bulge tan 22.5°), closed.
    let p = &polys[0];
    assert!(p.closed);
    assert_eq!(p.vertices.len(), 8, "{p:?}");
    let q = (std::f64::consts::PI / 8.0).tan();
    assert_eq!(p.vertices.iter().filter(|v| (v.bulge.abs() - q).abs() < 1e-9).count(), 4);
    assert!(near(p.vertices[0].p, Vec2::new(2.0, 9.0)), "{:?}", p.vertices[0]);
    // ry alone sets both radii, cut to half the side: a circle of four arcs.
    let c = &polys[1];
    assert_eq!(c.vertices.len(), 4, "{c:?}");
    assert!(c.vertices.iter().all(|v| (v.bulge.abs() - q).abs() < 1e-9));
    // Elliptical corners are flattened.
    let e = &polys[2];
    assert!(e.vertices.len() > 8 && e.vertices.iter().all(|v| v.bulge == 0.0), "{e:?}");
}

#[test]
fn elliptical_arcs_in_paths_are_not_read_as_circles() {
    let d = svg(r#"<path d="M0 5 A4 2 0 0 1 8 5"/>"#);
    let p = all(&d, |e| if let EntityKind::LwPolyline(p) = &e.kind { Some(p.clone()) } else { None });
    assert!(p[0].vertices.len() > 3 && p[0].vertices.iter().all(|v| v.bulge == 0.0), "{p:?}");
    // The top of the half-ellipse is 2 above the ends (SVG y down → drawing y up).
    let top = p[0].vertices.iter().map(|v| v.p.y).fold(f64::MIN, f64::max);
    assert!((top - 7.0).abs() < 1e-6, "{top}");
}

#[test]
fn gradients_become_gradient_hatches() {
    let d = svg(r##"<defs>
        <linearGradient id="lin"><stop offset="0" stop-color="red"/><stop offset="0.5" stop-color="lime"/><stop offset="1" style="stop-color:#00f"/></linearGradient>
        <linearGradient id="down" x1="0" y1="0" x2="0" y2="1" href="#lin"/>
        <radialGradient id="rad"><stop offset="0" stop-color="white"/><stop offset="1" stop-color="black"/></radialGradient>
        <radialGradient id="off" cx="0.1" cy="0.1"><stop offset="0" stop-color="white"/><stop offset="1" stop-color="black"/></radialGradient>
        <linearGradient id="one"><stop stop-color="#808080"/></linearGradient></defs>
        <rect width="4" height="4" fill="url(#lin)"/>
        <rect width="4" height="4" fill="url(#down)"/>
        <circle cx="10" cy="5" r="3" fill="url(#rad)"/>
        <circle cx="10" cy="5" r="3" fill="url(#off)"/>
        <rect width="4" height="4" fill="url(#one)"/>
        <rect width="4" height="4" fill="url(#missing) #0f0"/>
        <rect width="4" height="4" fill="url(#missing)"/>"##);
    let hs = hatches(&d);
    assert_eq!(hs.len(), 6);
    let g = hs[0].0.gradient.clone().unwrap();
    assert_eq!((g.name.as_str(), g.color1, g.color2), ("LINEAR", Color::True(Rgb(255, 0, 0)), Color::True(Rgb(0, 0, 255))));
    assert!(g.angle.abs() < 1e-9 && g.centered);
    // Stops come through href; down the page is down the drawing.
    let g = hs[1].0.gradient.clone().unwrap();
    assert_eq!(g.color1, Color::True(Rgb(255, 0, 0)));
    assert!((g.angle + std::f64::consts::FRAC_PI_2).abs() < 1e-9, "{}", g.angle);
    // Radial: the centre colour is the gradient's middle colour (colour 2).
    let g = hs[2].0.gradient.clone().unwrap();
    assert_eq!((g.name.as_str(), g.color1, g.color2, g.centered), ("SPHERICAL", Color::True(Rgb(0, 0, 0)), Color::True(Rgb(255, 255, 255)), true));
    assert!(!hs[3].0.gradient.clone().unwrap().centered);
    // One stop is a solid fill; a missing gradient uses its fallback colour, else no fill.
    assert_eq!((hs[4].0.gradient.is_none(), hs[4].1), (true, Color::True(Rgb(128, 128, 128))));
    assert_eq!((hs[5].0.gradient.is_none(), hs[5].1), (true, Color::True(Rgb(0, 255, 0))));
}

#[test]
fn text_and_tspans() {
    let d = svg(r#"<text x="2" y="5" font-size="5" text-anchor="middle" fill="red">Hello <tspan font-weight="bold">big</tspan>
            world &amp; co</text>
        <text x="1" y="8" style="font-size:2px" transform="rotate(-90 1 8)">Up</text>
        <style>.t { font-size: 10px }</style>
        <text class="t" x="0" y="2"><tspan x="0" y="2">one {a}</tspan><tspan x="0" y="4">two</tspan></text>
        <text x="0" y="0">   </text>"#);
    let texts = all(&d, |e| if let EntityKind::Text(t) = &e.kind { Some((t.clone(), e.common.color)) } else { None });
    assert_eq!(texts.len(), 2);
    let (t, color) = &texts[0];
    assert_eq!(t.value, "Hello big world & co");
    assert!((t.height - 3.5).abs() < 1e-9, "capital height of a 5mm font: {}", t.height);
    assert_eq!(t.halign, HAlign::Center);
    assert!(near(t.align_pt.unwrap().xy(), Vec2::new(2.0, 5.0)));
    assert_eq!(*color, Color::True(Rgb(255, 0, 0)));
    let (up, _) = &texts[1];
    assert!((up.rotation - std::f64::consts::FRAC_PI_2).abs() < 1e-9, "{}", up.rotation);
    assert!((up.height - 1.4).abs() < 1e-9 && near(up.insert.xy(), Vec2::new(1.0, 2.0)));
    // Lines placed by their own tspans make one MTEXT hanging from the first line's top.
    let m = all(&d, |e| if let EntityKind::MText(m) = &e.kind { Some(m.clone()) } else { None });
    assert_eq!(m.len(), 1);
    assert_eq!(m[0].contents, "one \\{a\\}\\Ptwo");
    assert_eq!(m[0].attach, 1);
    assert!((m[0].height - 7.0).abs() < 1e-9);
    assert!(near(m[0].insert.xy(), Vec2::new(0.0, 15.0)), "{:?}", m[0].insert);
    assert!((m[0].line_spacing - 2.0 / (7.0 * 5.0 / 3.0)).abs() < 1e-9 || m[0].line_spacing == 0.25, "{}", m[0].line_spacing);
}

/// A 4×2 opaque PNG, and its base64.
fn png() -> (Vec<u8>, String) {
    let rgba: Vec<u8> = (0..8).flat_map(|i| [i * 30, 255 - i * 30, 0, 255]).collect();
    let bytes = cadaddy_render::images::Raster::new(4, 2, rgba).unwrap().encode_png().unwrap();
    let b64 = base64(&bytes);
    (bytes, b64)
}

fn base64(b: &[u8]) -> String {
    const A: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for c in b.chunks(3) {
        let n = (u32::from(c[0]) << 16) | (u32::from(*c.get(1).unwrap_or(&0)) << 8) | u32::from(*c.get(2).unwrap_or(&0));
        for k in 0..4 {
            if k <= c.len() {
                out.push(A[((n >> (18 - 6 * k)) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

fn images(d: &Drawing) -> Vec<Image> {
    all(d, |e| if let EntityKind::Image(i) = &e.kind { Some(i.clone()) } else { None })
}

#[test]
fn embedded_images_are_written_beside_the_svg() {
    let (bytes, b64) = png();
    let dir = std::env::temp_dir().join(format!("cadaddy-svg-images-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let body = format!(
        r#"{PAGE}<image x="2" y="2" width="8" height="4" xlink:href="data:image/png;base64,{b64}"/>
        <image x="12" y="2" width="4" height="4" href="data:image/png;base64,{b64}"/></svg>"#
    );
    let path = dir.join("Logo.svg");
    let d = read(body.as_bytes(), &path.to_string_lossy()).unwrap();
    let ims = images(&d);
    assert_eq!(ims.len(), 2);
    let file = dir.join("Logo-image-1.png");
    assert_eq!(std::fs::read(&file).unwrap(), bytes);
    assert!(ims.iter().all(|i| i.path == file.to_string_lossy()), "the same image is written once");
    let i = &ims[0];
    assert_eq!(i.size, Vec2::new(4.0, 2.0));
    // Bottom-left corner at 2, 10-6; a pixel 2 wide and 2 high.
    assert!(near(i.insert.xy(), Vec2::new(2.0, 4.0)) && near(i.u.xy(), Vec2::new(2.0, 0.0)) && near(i.v.xy(), Vec2::new(0.0, 2.0)), "{i:?}");
    // Fitted into a square box: centred, half as tall as wide.
    let j = &ims[1];
    assert!(near(j.insert.xy(), Vec2::new(12.0, 5.0)) && near(j.u.xy(), Vec2::new(1.0, 0.0)), "{j:?}");
    assert!(cadaddy_render::images::get(&i.path).is_some());
    // Read again: the file already there is the same image and is used again.
    let d = read(body.as_bytes(), &path.to_string_lossy()).unwrap();
    assert_eq!(images(&d)[0].path, file.to_string_lossy());
    assert!(!dir.join("Logo-image-1-2.png").exists());
    // A different file of that name is kept; the image goes beside it.
    std::fs::write(&file, b"someone else's").unwrap();
    let d = read(body.as_bytes(), &path.to_string_lossy()).unwrap();
    assert_eq!(images(&d)[0].path, dir.join("Logo-image-1-2.png").to_string_lossy());
    assert_eq!(std::fs::read(&file).unwrap(), b"someone else's");
    // A linked image is used where it lies.
    let linked = format!(r#"{PAGE}<image width="4" height="2" href="Logo-image-1-2.png"/><image href="missing.png"/></svg>"#);
    let d = read(linked.as_bytes(), &path.to_string_lossy()).unwrap();
    assert_eq!(images(&d).len(), 1);
    assert_eq!(images(&d)[0].path, dir.join("Logo-image-1-2.png").to_string_lossy());
    assert!(d.read_notes.iter().any(|n| n.contains("missing.png")), "{:?}", d.read_notes);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn without_a_folder_images_show_in_this_session() {
    let (_, b64) = png();
    let d = read(
        format!(r#"{PAGE}<image width="4" height="4" preserveAspectRatio="xMinYMin slice" href="data:image/png;base64,{b64}"/></svg>"#).as_bytes(),
        "Mem.svg",
    )
    .unwrap();
    let i = &images(&d)[0];
    assert_eq!(i.path, "svg-image:Mem-image-1.png");
    assert!(cadaddy_render::images::get(&i.path).is_some());
    // Sliced: scaled to cover the box (a pixel 2 wide), the overflow clipped off.
    assert!(near(i.u.xy(), Vec2::new(2.0, 0.0)) && i.clipping && i.display & 4 != 0, "{i:?}");
    assert_eq!(i.clip, [Vec2::new(0.0, 0.0), Vec2::new(2.0, 2.0)]);
}

#[test]
fn hostile_input_is_refused_or_skipped() {
    for body in [
        r#"<image href="data:image/png;base64,!!!!"/>"#,
        r#"<image href="data:image/png;base64,AAAA"/>"#,
        r#"<image href="https://example.com/a.png"/>"#,
        r#"<style>{{{ .a { fill: red } @media { x { </style><rect class="a" width="1" height="1"/>"#,
        r#"<rect width="1" height="1" fill="url(#"/>"#,
        r#"<rect width="1" height="1" rx="-3" ry="nan"/>"#,
        r#"<text font-size="1e308" x="0" y="0">big</text><text font-size="-2">neg</text>"#,
        r#"<text transform="scale(0)">gone</text>"#,
        r##"<use/><use href="#"/><use href="data:x"/>"##,
        r##"<linearGradient id="a" href="#b"/><linearGradient id="b" href="#a"/><rect width="1" height="1" fill="url(#a)"/>"##,
        r#"<image href="data:image/png;base64,"/>"#,
    ] {
        let _ = read(format!("{PAGE}{body}</svg>").as_bytes(), "h.svg");
    }
    let deep = format!("{PAGE}{}{}</svg>", "<g>".repeat(2000), "</g>".repeat(2000));
    assert!(read(deep.as_bytes(), "deep.svg").is_err());
}

#[test]
fn a_shapes_own_transform_applies_to_it() {
    let d = svg(r#"<line x1="0" y1="0" x2="2" y2="0" transform="translate(3,1)"/><line x1="0" y1="0" x2="2" y2="0"/>"#);
    let ls = all(&d, |e| if let EntityKind::Line(l) = &e.kind { Some((l.a.xy(), l.b.xy())) } else { None });
    assert!(near(ls[0].0, Vec2::new(3.0, 9.0)) && near(ls[0].1, Vec2::new(5.0, 9.0)), "{ls:?}");
    assert!(near(ls[1].0, Vec2::new(0.0, 10.0)), "and to nothing after it: {ls:?}");
}
