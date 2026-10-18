//! SVG export of a space (vector, for the web and documents).

use std::fmt::Write as _;

use cadaddy_color::Rgb;
use cadaddy_doc::{Drawing, Space};
use cadaddy_geom::Bounds2;
use cadaddy_render::Kind;

/// Export with a white background (colour 7 prints black), fitted to the extents.
pub fn export(d: &Drawing, space: &Space) -> String {
    export_window(d, space, None)
}

/// Export showing exactly `window` (the `viewBox`; geometry outside it is clipped by the viewer),
/// or the extents with a 2% margin when `window` is `None`. Objects are drawn with their
/// transparency, as on screen.
pub fn export_window(d: &Drawing, space: &Space, window: Option<Bounds2>) -> String {
    export_window_with(d, space, window, true)
}

/// [`export_window`], drawing transparency only when `transparency` is on (plots).
pub fn export_window_with(d: &Drawing, space: &Space, window: Option<Bounds2>, transparency: bool) -> String {
    let list = cadaddy_render::build(d, space, &cadaddy_render::Options { transparency, ..cadaddy_render::Options::default() });
    export_list(&list, window)
}

/// Standard base64 (RFC 4648, padded), for data URLs.
fn base64(data: &[u8]) -> String {
    const A: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for c in data.chunks(3) {
        let n = (u32::from(*c.first().unwrap_or(&0)) << 16) | (u32::from(*c.get(1).unwrap_or(&0)) << 8) | u32::from(*c.get(2).unwrap_or(&0));
        for (i, shift) in [18, 12, 6, 0].into_iter().enumerate() {
            out.push(if i <= c.len() { char::from(A[(n >> shift) as usize & 63]) } else { '=' });
        }
    }
    out
}

/// ` opacity="…"` for a transparent primitive (none for an opaque one or a wipeout's mask).
fn opacity(p: &cadaddy_render::DPrim) -> String {
    if p.kind == Kind::Mask || p.transparency == 0 {
        return String::new();
    }
    format!(r#" opacity="{:.2}""#, f64::from(p.alpha()) / 255.0)
}

/// Write a display list (as [`export_window`] does for a space's own).
pub fn export_list(list: &cadaddy_render::DisplayList, window: Option<Bounds2>) -> String {
    let window = window.filter(|w| !w.is_empty());
    let b = window.unwrap_or(list.bounds);
    let (w, h) = if b.is_empty() { (1.0, 1.0) } else { (b.width().max(1e-9), b.height().max(1e-9)) };
    let m = if window.is_some() { 0.0 } else { (w.max(h)) * 0.02 };
    let (x0, y1) = if b.is_empty() { (0.0, 1.0) } else { (b.min.x - m, b.max.y + m) };
    let vw = w + 2.0 * m;
    let vh = h + 2.0 * m;
    let stroke = vw.max(vh) / 1500.0;
    let mut s = String::new();
    let _ = write!(
        s,
        r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {vw:.6} {vh:.6}" width="{:.0}" height="{:.0}"><rect width="100%" height="100%" fill="white"/>"#,
        1600.0,
        1600.0 * vh / vw
    );
    let white = Rgb(255, 255, 255);
    let tx = |x: f64| x - x0;
    let ty = |y: f64| y1 - y;
    let mut images = 0usize;
    for p in &list.prims {
        let c = p.display_rgb(white).hex();
        let op = opacity(p);
        let pts = list.points(p);
        match p.kind {
            Kind::Polyline => {
                let _ = write!(
                    s,
                    r#"<polyline fill="none" stroke="{c}"{op} stroke-width="{stroke:.6}" stroke-linecap="round" stroke-linejoin="round" points=""#
                );
                for q in pts {
                    let _ = write!(s, "{:.6},{:.6} ", tx(q.x), ty(q.y));
                }
                s.push_str(r#""/>"#);
            }
            Kind::Tris | Kind::Mask => {
                // A wipeout's mask hides what lies under it with the white background.
                let c = if p.kind == Kind::Mask { white.hex() } else { c };
                let _ = write!(s, r#"<path fill="{c}"{op} d=""#);
                for t in pts.chunks(3) {
                    if let [a, b2, cc] = t {
                        let _ = write!(s, "M{:.6} {:.6}L{:.6} {:.6}L{:.6} {:.6}Z", tx(a.x), ty(a.y), tx(b2.x), ty(b2.y), tx(cc.x), ty(cc.y));
                    }
                }
                s.push_str(r#""/>"#);
            }
            Kind::Image(_) => {
                // the pixels as an embedded PNG, clipped to the visible triangles
                let Some(img) = list.image(p) else { continue };
                let Some(png) = img.raster.encode_png() else { continue };
                let id = images;
                images += 1;
                let _ = write!(s, r#"<clipPath id="img{id}"><path d=""#);
                for t in pts.chunks(3) {
                    if let [a, b2, cc] = t {
                        let _ = write!(s, "M{:.6} {:.6}L{:.6} {:.6}L{:.6} {:.6}Z", tx(a.x), ty(a.y), tx(b2.x), ty(b2.y), tx(cc.x), ty(cc.y));
                    }
                }
                // pixel space → the SVG's (y down): the y components flip
                let [a, b2, cc, dd, e, f] = img.pixel_to_world();
                let opacity = f64::from(img.alpha) / 255.0 * f64::from(p.alpha()) / 255.0;
                let _ = write!(
                    s,
                    r#""/></clipPath><image clip-path="url(#img{id})" width="{}" height="{}" preserveAspectRatio="none" opacity="{opacity:.3}" transform="matrix({a:.9} {:.9} {cc:.9} {:.9} {:.6} {:.6})" href="data:image/png;base64,{}"/>"#,
                    img.raster.width,
                    img.raster.height,
                    -b2,
                    -dd,
                    tx(e),
                    ty(f),
                    base64(&png)
                );
            }
            Kind::Point => {
                if let Some(q) = pts.first() {
                    let _ = write!(s, r#"<circle cx="{:.6}" cy="{:.6}" r="{:.6}" fill="{c}"{op}/>"#, tx(q.x), ty(q.y), stroke);
                }
            }
            Kind::Infinite { ray } => {
                if let (Some(bp), Some(dir)) = (pts.first(), pts.get(1)) {
                    // A window can lie far from the base point: reach past it.
                    let far = (vw + vh) * 2.0 + if window.is_some() { bp.dist(b.center()) } else { 0.0 };
                    let a = if ray { *bp } else { *bp - *dir * far };
                    let e = *bp + *dir * far;
                    let _ = write!(
                        s,
                        r#"<line x1="{:.6}" y1="{:.6}" x2="{:.6}" y2="{:.6}" stroke="{c}"{op} stroke-width="{stroke:.6}"/>"#,
                        tx(a.x),
                        ty(a.y),
                        tx(e.x),
                        ty(e.y)
                    );
                }
            }
        }
    }
    s.push_str("</svg>");
    s
}
