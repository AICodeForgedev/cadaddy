//! CPU rasteriser: draws a display list with tiny-skia (PNG export, plot preview, tests).

use cadaddy_color::Rgb;
use cadaddy_geom::{Bounds2, Vec2};
use tiny_skia::{FillRule, LineCap, LineJoin, Paint, PathBuilder, Pixmap, Stroke, Transform};

use crate::{DisplayList, Kind};

/// A 2D view: world point at the image centre and pixels per drawing unit.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct View {
    pub center: Vec2,
    pub scale: f64,
    pub width: u32,
    pub height: u32,
}

impl View {
    pub fn to_screen(&self, p: Vec2) -> (f32, f32) {
        let x = (p.x - self.center.x) * self.scale + f64::from(self.width) / 2.0;
        let y = f64::from(self.height) / 2.0 - (p.y - self.center.y) * self.scale;
        (x as f32, y as f32)
    }
    /// Fit `b` with a margin fraction.
    pub fn fit(b: &Bounds2, width: u32, height: u32, margin: f64) -> View {
        if b.is_empty() {
            return View { center: Vec2::ZERO, scale: 1.0, width, height };
        }
        let w = b.width().max(1e-9);
        let h = b.height().max(1e-9);
        let s = (f64::from(width) / w).min(f64::from(height) / h) * (1.0 - margin);
        View { center: b.center(), scale: if s.is_finite() && s > 0.0 { s } else { 1.0 }, width, height }
    }
    /// The view of `window` (drawing units), filling the image: `width` × `height` pixels when both
    /// are given; otherwise the side given (default 1600 wide) with the other matching the window's
    /// shape, each side 16..=8192.
    pub fn of_window(window: &Bounds2, width: Option<u32>, height: Option<u32>) -> View {
        let aspect = if window.width() > 1e-12 && window.height() > 1e-12 { window.width() / window.height() } else { 1.0 };
        let side = |v: f64| v.round().clamp(16.0, 8192.0) as u32;
        let (w, h) = match (width, height) {
            (Some(w), Some(h)) => (side(f64::from(w)), side(f64::from(h))),
            (None, Some(h)) => (side(f64::from(h) * aspect), side(f64::from(h))),
            (w, None) => {
                let w = f64::from(w.unwrap_or(1600));
                (side(w), side(w / aspect))
            }
        };
        View::fit(window, w, h, 0.0)
    }
    pub fn world_bounds(&self) -> Bounds2 {
        let hw = f64::from(self.width) / 2.0 / self.scale;
        let hh = f64::from(self.height) / 2.0 / self.scale;
        Bounds2::new(self.center - Vec2::new(hw, hh), self.center + Vec2::new(hw, hh))
    }
}

#[derive(Clone, Copy, Debug)]
pub struct RasterOptions {
    pub background: Rgb,
    /// Line width in pixels for zero-lineweight lines.
    pub hairline: f32,
    /// Pixels per mm for lineweights (0 = ignore lineweights).
    pub px_per_mm: f32,
    pub antialias: bool,
}

impl Default for RasterOptions {
    fn default() -> Self {
        RasterOptions { background: Rgb(33, 40, 48), hairline: 1.0, px_per_mm: 0.0, antialias: true }
    }
}

/// Render to a pixmap. Returns `None` for zero or absurd sizes.
pub fn render(list: &DisplayList, view: &View, o: &RasterOptions) -> Option<Pixmap> {
    if view.width == 0 || view.height == 0 || view.width > 16384 || view.height > 16384 {
        return None;
    }
    let mut pm = Pixmap::new(view.width, view.height)?;
    pm.fill(tiny_skia::Color::from_rgba8(o.background.0, o.background.1, o.background.2, 255));
    let vis = view.world_bounds();
    // each image's pixels as a premultiplied pixmap, made once per render
    let mut pixmaps: std::collections::HashMap<*const crate::images::Raster, Option<Pixmap>> = std::collections::HashMap::new();
    for p in &list.prims {
        let c = p.display_rgb(o.background);
        let mut paint = Paint::default();
        // A transparent object blends with what is drawn under it.
        paint.set_color_rgba8(c.0, c.1, c.2, p.alpha());
        paint.anti_alias = o.antialias;
        let pts = list.points(p);
        match p.kind {
            Kind::Polyline => {
                let bb = Bounds2::from_points(pts.iter().copied());
                if !bb.intersects(&vis) {
                    continue;
                }
                let mut pb = PathBuilder::new();
                for (i, q) in pts.iter().enumerate() {
                    let (x, y) = view.to_screen(*q);
                    if i == 0 {
                        pb.move_to(x, y);
                    } else {
                        pb.line_to(x, y);
                    }
                }
                if let Some(path) = pb.finish() {
                    let w = if o.px_per_mm > 0.0 && p.lw > 0.0 { (p.lw * o.px_per_mm).max(o.hairline) } else { o.hairline };
                    let stroke = Stroke { width: w, line_cap: LineCap::Round, line_join: LineJoin::Round, ..Default::default() };
                    pm.stroke_path(&path, &paint, &stroke, Transform::identity(), None);
                }
            }
            Kind::Tris | Kind::Mask => {
                if p.kind == Kind::Mask {
                    paint.set_color_rgba8(o.background.0, o.background.1, o.background.2, 255);
                }
                let mut pb = PathBuilder::new();
                for t in pts.chunks(3) {
                    if let [a, b, c] = t {
                        let (ax, ay) = view.to_screen(*a);
                        let (bx, by) = view.to_screen(*b);
                        let (cx, cy) = view.to_screen(*c);
                        pb.move_to(ax, ay);
                        pb.line_to(bx, by);
                        pb.line_to(cx, cy);
                        pb.close();
                    }
                }
                if let Some(path) = pb.finish() {
                    pm.fill_path(&path, &paint, FillRule::Winding, Transform::identity(), None);
                }
            }
            Kind::Image(_) => {
                let Some(img) = list.image(p) else { continue };
                let shown = img.raster_on(o.background);
                let key = std::sync::Arc::as_ptr(&shown);
                let Some(src) = pixmaps.entry(key).or_insert_with(|| pixmap_of(&shown)).as_ref() else { continue };
                let [a, b, c, d, e, f] = img.pixel_to_world();
                let s = view.scale;
                let (hw, hh) = (f64::from(view.width) / 2.0, f64::from(view.height) / 2.0);
                let xf = Transform::from_row(
                    (s * a) as f32,
                    (-s * b) as f32,
                    (s * c) as f32,
                    (-s * d) as f32,
                    (s * (e - view.center.x) + hw) as f32,
                    (hh - s * (f - view.center.y)) as f32,
                );
                let opacity = f32::from(img.alpha) / 255.0 * f32::from(p.alpha()) / 255.0;
                let paint = Paint {
                    anti_alias: o.antialias,
                    shader: tiny_skia::Pattern::new(src.as_ref(), tiny_skia::SpreadMode::Pad, tiny_skia::FilterQuality::Bilinear, opacity, xf),
                    ..Default::default()
                };
                if let Some(path) = tri_path(view, pts) {
                    pm.fill_path(&path, &paint, FillRule::Winding, Transform::identity(), None);
                }
            }
            Kind::Point => {
                if let Some(q) = pts.first() {
                    let (x, y) = view.to_screen(*q);
                    // Filled as a path: tiny-skia's anti-aliased `fill_rect` trips a debug
                    // assertion on rects under two pixels wide whose sides both fall mid-pixel.
                    if let Some(r) = tiny_skia::Rect::from_xywh(x - 0.5, y - 0.5, 1.5, 1.5) {
                        pm.fill_path(&PathBuilder::from_rect(r), &paint, FillRule::Winding, Transform::identity(), None);
                    }
                }
            }
            Kind::Infinite { ray } => {
                let (Some(base), Some(dir)) = (pts.first(), pts.get(1)) else { continue };
                let far = (vis.width() + vis.height()) * 2.0 + base.dist(vis.center());
                let a = if ray { *base } else { *base - *dir * far };
                let b = *base + *dir * far;
                let mut pb = PathBuilder::new();
                let (ax, ay) = view.to_screen(a);
                let (bx, by) = view.to_screen(b);
                pb.move_to(ax, ay);
                pb.line_to(bx, by);
                if let Some(path) = pb.finish() {
                    pm.stroke_path(&path, &paint, &Stroke { width: o.hairline, ..Default::default() }, Transform::identity(), None);
                }
            }
        }
    }
    Some(pm)
}

/// The triangles `pts` (three points each) on screen as one path.
fn tri_path(view: &View, pts: &[Vec2]) -> Option<tiny_skia::Path> {
    let mut pb = PathBuilder::new();
    for t in pts.chunks(3) {
        if let [a, b, c] = t {
            let (ax, ay) = view.to_screen(*a);
            let (bx, by) = view.to_screen(*b);
            let (cx, cy) = view.to_screen(*c);
            pb.move_to(ax, ay);
            pb.line_to(bx, by);
            pb.line_to(cx, cy);
            pb.close();
        }
    }
    pb.finish()
}

/// Image pixels as a tiny-skia pixmap (premultiplied).
fn pixmap_of(r: &crate::images::Raster) -> Option<Pixmap> {
    let mut pm = Pixmap::new(r.width, r.height)?;
    for (dst, src) in pm.pixels_mut().iter_mut().zip(r.rgba.as_chunks::<4>().0.iter()) {
        let [red, g, b, a] = *src;
        *dst = tiny_skia::ColorU8::from_rgba(red, g, b, a).premultiply();
    }
    Some(pm)
}

/// Render and encode as PNG.
pub fn render_png(list: &DisplayList, view: &View, o: &RasterOptions) -> Option<Vec<u8>> {
    render(list, view, o)?.encode_png().ok()
}

/// [`render_png`], turned 180° when `upside_down` (a plot's "Plot upside down").
pub fn render_png_turned(list: &DisplayList, view: &View, o: &RasterOptions, upside_down: bool) -> Option<Vec<u8>> {
    let mut pm = render(list, view, o)?;
    if upside_down {
        // Reversing the pixel order turns the image half a turn.
        pm.data_mut().as_chunks_mut::<4>().0.reverse();
    }
    pm.encode_png().ok()
}
