//! EPS export (Encapsulated PostScript, vector): the same display list the SVG and PDF exports
//! draw, fitted into a box of at most 720 points on its longer side. Lines keep their colours and
//! lineweights (paper millimetres as points), filled areas fill, wipeouts paint white, raster
//! images are embedded (RGB, clipped to what shows). Text is drawn as its strokes, as on screen.

use std::fmt::Write as _;

use cadaddy_color::Rgb;
use cadaddy_doc::{Drawing, Space};
use cadaddy_geom::Bounds2;
use cadaddy_render::Kind;

/// The longer side of the picture, in points.
const SIZE_PT: f64 = 720.0;
/// Largest image embedded (pixels on a side; larger ones are sampled down).
const MAX_IMAGE_PX: u32 = 1024;

/// A space as EPS, showing `window` (or the extents with a 2% margin).
pub fn export(d: &Drawing, space: &Space, window: Option<Bounds2>) -> String {
    let list = cadaddy_render::build(d, space, &cadaddy_render::Options::default());
    export_list(&list, window)
}

fn rgb(s: &mut String, c: Rgb) {
    let _ = writeln!(s, "{:.4} {:.4} {:.4} setrgbcolor", f64::from(c.0) / 255.0, f64::from(c.1) / 255.0, f64::from(c.2) / 255.0);
}

/// Write a display list as EPS.
pub fn export_list(list: &cadaddy_render::DisplayList, window: Option<Bounds2>) -> String {
    let window = window.filter(|w| !w.is_empty());
    let b = window.unwrap_or(list.bounds);
    let (w, h) = if b.is_empty() { (1.0, 1.0) } else { (b.width().max(1e-9), b.height().max(1e-9)) };
    let m = if window.is_some() { 0.0 } else { w.max(h) * 0.02 };
    let (x0, y0) = if b.is_empty() { (0.0, 0.0) } else { (b.min.x - m, b.min.y - m) };
    let (vw, vh) = (w + 2.0 * m, h + 2.0 * m);
    // Points per drawing unit.
    let k = SIZE_PT / vw.max(vh);
    let (pw, ph) = ((vw * k).ceil().max(1.0), (vh * k).ceil().max(1.0));
    let tx = |x: f64| (x - x0) * k;
    let ty = |y: f64| (y - y0) * k;
    let mut s = String::new();
    s.push_str("%!PS-Adobe-3.0 EPSF-3.0\n");
    let _ = writeln!(s, "%%BoundingBox: 0 0 {} {}", pw as i64, ph as i64);
    let _ = writeln!(s, "%%HiResBoundingBox: 0 0 {:.3} {:.3}", vw * k, vh * k);
    s.push_str("%%Creator: CADaddy\n%%LanguageLevel: 2\n%%Pages: 0\n%%EndComments\n");
    s.push_str("%%BeginProlog\n/m {moveto} bind def /l {lineto} bind def /t {moveto lineto lineto closepath fill} bind def\n%%EndProlog\n");
    s.push_str("gsave\n1 setlinecap 1 setlinejoin\n");
    // The white paper, then a clip to the picture.
    let _ = writeln!(s, "1 1 1 setrgbcolor 0 0 {pw:.3} {ph:.3} rectfill");
    let _ = writeln!(s, "0 0 {pw:.3} {ph:.3} rectclip");
    let white = Rgb(255, 255, 255);
    let thin = 0.25;
    let mut last: Option<(Rgb, f64)> = None;
    for p in &list.prims {
        let c = if p.kind == Kind::Mask { white } else { p.display_rgb(white) };
        let lw = if p.lw > 0.0 { (f64::from(p.lw) * 72.0 / 25.4).max(thin) } else { thin };
        if last != Some((c, lw)) {
            rgb(&mut s, c);
            let _ = writeln!(s, "{lw:.3} setlinewidth");
            last = Some((c, lw));
        }
        let pts = list.points(p);
        match p.kind {
            Kind::Polyline => {
                let mut first = true;
                for (i, q) in pts.iter().enumerate() {
                    let _ = write!(s, "{:.3} {:.3} {}", tx(q.x), ty(q.y), if first { "m" } else { "l" });
                    first = false;
                    s.push(if i % 8 == 7 { '\n' } else { ' ' });
                }
                if pts.len() > 1 {
                    s.push_str("stroke\n");
                } else {
                    s.push_str("newpath\n");
                }
            }
            Kind::Tris | Kind::Mask => {
                for t in pts.chunks(3) {
                    if let [a, b2, cc] = t {
                        let _ = writeln!(s, "{:.3} {:.3} {:.3} {:.3} {:.3} {:.3} t", tx(cc.x), ty(cc.y), tx(b2.x), ty(b2.y), tx(a.x), ty(a.y));
                    }
                }
            }
            Kind::Point => {
                if let Some(q) = pts.first() {
                    let _ = writeln!(s, "newpath {:.3} {:.3} {:.3} 0 360 arc fill", tx(q.x), ty(q.y), lw.max(0.5));
                }
            }
            Kind::Infinite { ray } => {
                if let (Some(bp), Some(dir)) = (pts.first(), pts.get(1)) {
                    let far = (vw + vh) * 2.0 + bp.dist(b.center());
                    let a = if ray { *bp } else { *bp - *dir * far };
                    let e = *bp + *dir * far;
                    let _ = writeln!(s, "{:.3} {:.3} m {:.3} {:.3} l stroke", tx(a.x), ty(a.y), tx(e.x), ty(e.y));
                }
            }
            Kind::Image(_) => {
                let Some(img) = list.image(p) else { continue };
                image(&mut s, img, pts, &tx, &ty, k);
                last = None;
            }
        }
    }
    s.push_str("grestore\n%%EOF\n");
    s
}

/// An image: clipped to its visible triangles, its pixels (RGB over white) placed by its
/// pixel-to-world map.
fn image(s: &mut String, img: &cadaddy_render::ImageRef, tris: &[cadaddy_geom::Vec2], tx: &dyn Fn(f64) -> f64, ty: &dyn Fn(f64) -> f64, k: f64) {
    let r = &img.raster;
    let (w, h) = (r.width, r.height);
    if w == 0 || h == 0 || (w as usize) * (h as usize) * 4 > r.rgba.len() + 1 {
        return;
    }
    // Sampled down to at most MAX_IMAGE_PX on a side.
    let step = w.max(h).div_ceil(MAX_IMAGE_PX).max(1);
    let (sw, sh) = (w.div_ceil(step), h.div_ceil(step));
    s.push_str("gsave newpath\n");
    for t in tris.chunks(3) {
        if let [a, b, c] = t {
            let _ = writeln!(s, "{:.3} {:.3} m {:.3} {:.3} l {:.3} {:.3} l closepath", tx(a.x), ty(a.y), tx(b.x), ty(b.y), tx(c.x), ty(c.y));
        }
    }
    s.push_str("clip newpath\n");
    // Pixel space (sampled) → page: x' = a·x + c·y + e, y' = b·x + d·y + f, scaled into points.
    let [a, b, c, d, e, f] = img.pixel_to_world();
    let sf = f64::from(step);
    let _ = writeln!(s, "[{:.9} {:.9} {:.9} {:.9} {:.6} {:.6}] concat", a * k * sf, b * k * sf, c * k * sf, d * k * sf, tx(e), ty(f));
    let _ = writeln!(s, "/row {} string def", sw * 3);
    let _ = writeln!(s, "{sw} {sh} 8 [1 0 0 1 0 0] {{currentfile row readhexstring pop}} false 3 colorimage");
    let alpha = f64::from(img.alpha) / 255.0;
    for y in 0..sh {
        let mut line = String::with_capacity((sw * 6 + 1) as usize);
        for x in 0..sw {
            let i = (((y * step) * w + x * step) * 4) as usize;
            let [pr, pg, pb, pa] = r.rgba.get(i..i + 4).and_then(|p| <[u8; 4]>::try_from(p).ok()).unwrap_or([255, 255, 255, 0]);
            // Over white, by the pixel's and the image's opacity.
            let a2 = f64::from(pa) / 255.0 * alpha;
            for ch in [pr, pg, pb] {
                let v = (f64::from(ch) * a2 + 255.0 * (1.0 - a2)).round().clamp(0.0, 255.0) as u8;
                let _ = write!(line, "{v:02x}");
            }
        }
        s.push_str(&line);
        s.push('\n');
    }
    s.push_str("grestore\n");
}
