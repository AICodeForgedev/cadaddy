//! Shaded and hidden-line views of model space (VSCURRENT's visual styles): the drawing as 3D
//! triangles and lines in world coordinates ([`scene`]), drawn into a raster with a depth buffer
//! ([`rasterize`]). The canvas, PNG export and plots share it.
//!
//! Meshes and 3D faces are surfaces; 3D lines and polylines stay lines; everything else (2D
//! objects, text, block contents) is drawn as in the plan view and lifted to its elevation, so
//! solids, solid hatches and filled text are flat surfaces there and hide what lies behind them.

use cadaddy_color::Rgb;
use cadaddy_doc::{Drawing, EntityKind, Handle, Space};
use cadaddy_geom::view3::Projection;
use cadaddy_geom::{Vec2, Vec3};

use crate::images::Raster;
use crate::{Kind, Options, build};

/// A visual style (VSCURRENT).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum VisualStyle {
    /// Lines and curves as the plan display draws them (no depth).
    #[default]
    Wireframe2d,
    /// Every edge and line of the 3D objects, see-through.
    Wireframe,
    /// Edges and lines, those behind surfaces hidden.
    Hidden,
    /// Surfaces lit (Lambert), no edges.
    Shaded,
    /// Surfaces lit, with their edges.
    ShadedEdges,
    /// Surfaces lit with highlights (Lambert plus specular).
    Realistic,
}

impl VisualStyle {
    pub const ALL: [VisualStyle; 6] = [
        VisualStyle::Wireframe2d,
        VisualStyle::Wireframe,
        VisualStyle::Hidden,
        VisualStyle::Shaded,
        VisualStyle::ShadedEdges,
        VisualStyle::Realistic,
    ];

    /// The name AutoCAD shows (the viewport control, VSCURRENT).
    pub fn name(self) -> &'static str {
        match self {
            VisualStyle::Wireframe2d => "2D Wireframe",
            VisualStyle::Wireframe => "Wireframe",
            VisualStyle::Hidden => "Hidden",
            VisualStyle::Shaded => "Shaded",
            VisualStyle::ShadedEdges => "Shaded with Edges",
            VisualStyle::Realistic => "Realistic",
        }
    }

    /// A style by name or VSCURRENT keyword (any case, spaces ignored).
    pub fn parse(s: &str) -> Option<VisualStyle> {
        let k: String = s.chars().filter(|c| !c.is_whitespace() && *c != '_').collect::<String>().to_ascii_lowercase();
        Some(match k.as_str() {
            "2" | "2d" | "2dwireframe" => VisualStyle::Wireframe2d,
            "w" | "wireframe" | "3dwireframe" => VisualStyle::Wireframe,
            "h" | "hidden" | "3dhidden" => VisualStyle::Hidden,
            "s" | "shaded" => VisualStyle::Shaded,
            "e" | "shadedwithedges" | "shadededges" => VisualStyle::ShadedEdges,
            "r" | "realistic" => VisualStyle::Realistic,
            _ => return None,
        })
    }

    /// Surfaces are filled with light and shade.
    pub fn shaded(self) -> bool {
        matches!(self, VisualStyle::Shaded | VisualStyle::ShadedEdges | VisualStyle::Realistic)
    }
}

/// A triangle of a surface, in world coordinates.
#[derive(Clone, Copy, Debug)]
pub struct Tri {
    pub p: [Vec3; 3],
    pub color: Rgb,
}

/// A line, in world coordinates; `edge`: an edge of a surface (drawn by the styles that show
/// edges) rather than a line object (drawn by every style).
#[derive(Clone, Copy, Debug)]
pub struct Line3 {
    pub a: Vec3,
    pub b: Vec3,
    pub color: Rgb,
    pub edge: bool,
}

/// What a shaded view draws.
#[derive(Clone, Debug, Default)]
pub struct Scene {
    pub tris: Vec<Tri>,
    pub lines: Vec<Line3>,
}

/// Most triangles and lines a scene holds (beyond them the rest is left out).
pub const MAX_SCENE: usize = 20_000_000;

/// The scene of `space`: surfaces and lines in world coordinates, curves tessellated to `tol`
/// world units, colour 7 as it shows on `bg`.
pub fn scene(d: &Drawing, space: &Space, tol: f64, bg: Rgb) -> Scene {
    let Some(store) = d.space(space) else { return Scene::default() };
    let list = build(d, space, &Options { tolerance: tol.max(1e-9), ..Options::default() });
    // Each object's colours (its first primitive's): its lines as drawn on `bg` (colour 7 black on
    // white), its surfaces in their own colour (colour 7 is white there).
    let mut colors: std::collections::HashMap<Handle, (Rgb, Rgb)> = std::collections::HashMap::new();
    for p in &list.prims {
        colors.entry(p.handle).or_insert_with(|| (p.display_rgb(bg), p.color));
    }
    let ink = if bg.luma() > 0.5 { Rgb(0, 0, 0) } else { Rgb(255, 255, 255) };
    let mut out = Scene::default();
    let mut native: std::collections::HashSet<Handle> = std::collections::HashSet::new();
    let mut elevation: std::collections::HashMap<Handle, f64> = std::collections::HashMap::new();
    for e in store.iter() {
        if !d.is_visible(e) {
            continue;
        }
        let (color, surface) = colors.get(&e.handle).copied().unwrap_or((ink, Rgb(255, 255, 255)));
        match &e.kind {
            EntityKind::Mesh(m) => {
                native.insert(e.handle);
                for t in m.triangles() {
                    push_tri(&mut out, Tri { p: t, color: surface });
                }
                for (a, b) in m.edges() {
                    if let (Some(a), Some(b)) = (m.vertices.get(a as usize), m.vertices.get(b as usize)) {
                        push_line(&mut out, Line3 { a: *a, b: *b, color, edge: true });
                    }
                }
            }
            EntityKind::Face3d(f) => {
                native.insert(e.handle);
                let [a, b, c, dd] = f.corners;
                push_tri(&mut out, Tri { p: [a, b, c], color: surface });
                if c != dd {
                    push_tri(&mut out, Tri { p: [a, c, dd], color: surface });
                }
                let corners = if c == dd { vec![a, b, c] } else { vec![a, b, c, dd] };
                let n = corners.len();
                for i in 0..n {
                    if f.hidden_edges & (1 << i) == 0
                        && let (Some(p), Some(q)) = (corners.get(i), corners.get((i + 1) % n))
                    {
                        push_line(&mut out, Line3 { a: *p, b: *q, color, edge: true });
                    }
                }
            }
            k => {
                if let Some((pts, closed)) = k.points3d() {
                    native.insert(e.handle);
                    let n = pts.len();
                    let segs = if closed { n } else { n.saturating_sub(1) };
                    for i in 0..segs {
                        if let (Some(p), Some(q)) = (pts.get(i), pts.get((i + 1) % n)) {
                            push_line(&mut out, Line3 { a: *p, b: *q, color, edge: false });
                        }
                    }
                } else {
                    let z = match k {
                        EntityKind::Insert(i) => i.insert.z,
                        _ => k.elevation(),
                    };
                    elevation.insert(e.handle, if z.is_finite() { z } else { 0.0 });
                }
            }
        }
    }
    // Everything else, as the plan display draws it, at its elevation.
    for p in &list.prims {
        if native.contains(&p.handle) {
            continue;
        }
        let z = elevation.get(&p.handle).copied().unwrap_or(0.0);
        let color = p.display_rgb(bg);
        let pts = list.points(p);
        let lift = |q: &Vec2| Vec3::new(q.x, q.y, z);
        match p.kind {
            Kind::Polyline => {
                for w in pts.windows(2) {
                    if let [a, b] = w {
                        push_line(&mut out, Line3 { a: lift(a), b: lift(b), color, edge: false });
                    }
                }
            }
            Kind::Tris => {
                for [a, b, c] in pts.as_chunks::<3>().0 {
                    push_tri(&mut out, Tri { p: [lift(a), lift(b), lift(c)], color: p.color });
                }
            }
            _ => {}
        }
    }
    out
}

fn push_tri(s: &mut Scene, t: Tri) {
    if s.tris.len() < MAX_SCENE && t.p.iter().all(|p| p.is_finite()) {
        s.tris.push(t);
    }
}

fn push_line(s: &mut Scene, l: Line3) {
    if s.lines.len() < MAX_SCENE && l.a.is_finite() && l.b.is_finite() {
        s.lines.push(l);
    }
}

/// How a shaded view looks at the drawing: the view plane's axes (as [`Projection`]: `dir`
/// towards the viewer), the point of the plane at the image's centre and the plane's height the
/// image shows.
#[derive(Clone, Copy, Debug)]
pub struct ShadeView {
    pub dir: Vec3,
    pub right: Vec3,
    pub up: Vec3,
    pub center: Vec2,
    pub height: f64,
    pub width_px: u32,
    pub height_px: u32,
}

/// Largest raster side.
pub const MAX_SIDE: u32 = 16_384;

impl ShadeView {
    /// A view through `p` (`None`: the plan view, looking down Z).
    pub fn new(p: Option<Projection>, center: Vec2, height: f64, width_px: u32, height_px: u32) -> ShadeView {
        let (dir, right, up) = match p {
            Some(p) => (p.dir, p.right, p.up),
            None => (Vec3::Z, Vec3::new(1.0, 0.0, 0.0), Vec3::new(0.0, 1.0, 0.0)),
        };
        ShadeView { dir, right, up, center, height, width_px: width_px.clamp(1, MAX_SIDE), height_px: height_px.clamp(1, MAX_SIDE) }
    }

    /// A world point on the image: pixel x, y (down) and depth (larger is nearer the viewer).
    fn screen(&self, p: Vec3) -> (f64, f64, f64) {
        let px = self.height / f64::from(self.height_px);
        let x = (p.dot(self.right) - self.center.x) / px + f64::from(self.width_px) / 2.0;
        let y = f64::from(self.height_px) / 2.0 - (p.dot(self.up) - self.center.y) / px;
        (x, y, p.dot(self.dir))
    }
}

/// A view through `p` (`None`: the plan) fitting everything `scene` holds into a `w` × `h`
/// image with a little margin; `None` for an empty scene.
pub fn fit(scene: &Scene, p: Option<Projection>, w: u32, h: u32) -> Option<ShadeView> {
    let v = ShadeView::new(p, Vec2::ZERO, 1.0, w, h);
    let on_plane = |q: Vec3| Vec2::new(q.dot(v.right), q.dot(v.up));
    let b = cadaddy_geom::Bounds2::from_points(scene.tris.iter().flat_map(|t| t.p).chain(scene.lines.iter().flat_map(|l| [l.a, l.b])).map(on_plane));
    if b.is_empty() || !(b.width().is_finite() && b.height().is_finite()) {
        return None;
    }
    let height = (b.height().max(b.width() * f64::from(v.height_px) / f64::from(v.width_px)) * 1.04).max(1e-9);
    Some(ShadeView { center: b.center(), height, ..v })
}

/// The drawing's model space drawn in `style` along `p`, fitted into a `w` × `h` image on `bg`
/// (exports and plots of a shaded or 3D view).
pub fn render_fitted(d: &Drawing, p: Option<Projection>, style: VisualStyle, w: u32, h: u32, bg: Rgb) -> Option<Raster> {
    let ext = d.extents(&Space::Model);
    let size = ext.width().max(ext.height());
    let size = if size.is_finite() && size > 0.0 { size } else { 1.0 };
    // Roughly first, for the extents on the view plane; then at the image's detail.
    let view = fit(&scene(d, &Space::Model, size / 500.0, bg), p, w, h)?;
    let s = scene(d, &Space::Model, view.height / f64::from(view.height_px) * 0.5, bg);
    Some(rasterize(&s, &view, style, bg))
}

/// Rows a band of the raster has (bands are drawn side by side).
const BAND: usize = 32;

/// Draw `scene` as `style` shows it: surfaces with a depth buffer (lit for the shaded styles,
/// the background colour for Hidden), then lines and the edges the style shows, those behind a
/// surface left out (Wireframe shows them all).
pub fn rasterize(scene: &Scene, v: &ShadeView, style: VisualStyle, bg: Rgb) -> Raster {
    let (w, h) = (v.width_px as usize, v.height_px as usize);
    let mut color = vec![[bg.0, bg.1, bg.2, 255u8]; w * h];
    let mut depth = vec![f32::NEG_INFINITY; w * h];
    let faces = style != VisualStyle::Wireframe && style != VisualStyle::Wireframe2d;
    // Light from over the viewer's left shoulder; Realistic adds a highlight.
    let light = (v.dir * 0.6 + v.up * 0.6 - v.right * 0.35).normalized();
    let half = (light + v.dir).normalized();
    let specular = style == VisualStyle::Realistic;
    struct ScreenTri {
        s: [(f64, f64, f64); 3],
        rgb: [u8; 3],
        y0: usize,
        y1: usize,
    }
    let mut zmin = f64::INFINITY;
    let mut zmax = f64::NEG_INFINITY;
    let tris: Vec<ScreenTri> = if faces {
        scene
            .tris
            .iter()
            .filter_map(|t| {
                let s = t.p.map(|p| v.screen(p));
                let n = (t.p[1] - t.p[0]).cross(t.p[2] - t.p[0]);
                let len = n.len();
                if !(len.is_finite() && len > 1e-300) {
                    return None;
                }
                let n = n * (1.0 / len);
                let n = if n.dot(v.dir) < 0.0 { -n } else { n };
                let rgb = if style == VisualStyle::Hidden {
                    [bg.0, bg.1, bg.2]
                } else {
                    let diffuse = n.dot(light).max(0.0);
                    let k = 0.3 + 0.7 * diffuse;
                    let spec = if specular { n.dot(half).max(0.0).powi(24) * 0.35 } else { 0.0 };
                    let ch = |c: u8| ((f64::from(c) * k + 255.0 * spec).round().clamp(0.0, 255.0)) as u8;
                    [ch(t.color.0), ch(t.color.1), ch(t.color.2)]
                };
                let ys = s.map(|p| p.1);
                let y0 = ys.iter().copied().fold(f64::INFINITY, f64::min).floor().max(0.0);
                let y1 = ys.iter().copied().fold(f64::NEG_INFINITY, f64::max).ceil().min(h as f64);
                if !(y0.is_finite() && y1.is_finite()) || y1 <= y0 {
                    return None;
                }
                Some(ScreenTri { s, rgb, y0: y0 as usize, y1: y1 as usize })
            })
            .collect()
    } else {
        Vec::new()
    };
    for t in &tris {
        for p in &t.s {
            zmin = zmin.min(p.2);
            zmax = zmax.max(p.2);
        }
    }
    // Triangles by the bands they cover.
    let bands = h.div_ceil(BAND).max(1);
    let mut binned: Vec<Vec<usize>> = vec![Vec::new(); bands];
    for (i, t) in tris.iter().enumerate() {
        for b in t.y0 / BAND..=(t.y1.saturating_sub(1) / BAND).min(bands - 1) {
            if let Some(list) = binned.get_mut(b) {
                list.push(i);
            }
        }
    }
    let draw_band = |(b, (cband, zband)): (usize, (&mut [[u8; 4]], &mut [f32]))| {
        let row0 = b * BAND;
        for &i in binned.get(b).map(Vec::as_slice).unwrap_or(&[]) {
            let Some(t) = tris.get(i) else { continue };
            raster_tri(t.s, t.rgb, row0, cband, zband, w);
        }
    };
    #[cfg(not(target_arch = "wasm32"))]
    {
        use rayon::prelude::*;
        color.par_chunks_mut(BAND * w).zip(depth.par_chunks_mut(BAND * w)).enumerate().for_each(draw_band);
    }
    #[cfg(target_arch = "wasm32")]
    color.chunks_mut(BAND * w).zip(depth.chunks_mut(BAND * w)).enumerate().for_each(draw_band);
    // Lines: tested against the surfaces with a little slack, so edges on a face show.
    let px = v.height / f64::from(v.height_px);
    let range = if zmax > zmin { zmax - zmin } else { 1.0 };
    let bias = (range * 2e-3 + px * 1.5) as f32;
    let edges = matches!(style, VisualStyle::Wireframe | VisualStyle::Hidden | VisualStyle::ShadedEdges | VisualStyle::Wireframe2d);
    let dark = if bg.luma() > 0.5 { Rgb(40, 40, 40) } else { Rgb(20, 20, 20) };
    for l in &scene.lines {
        if l.edge && !edges {
            continue;
        }
        let c = if l.edge && style == VisualStyle::ShadedEdges { dark } else { l.color };
        let (a, b) = (v.screen(l.a), v.screen(l.b));
        raster_line(a, b, [c.0, c.1, c.2], &mut color, &depth, w, h, if faces { Some(bias) } else { None });
    }
    let rgba: Vec<u8> = color.into_iter().flatten().collect();
    Raster::new(v.width_px, v.height_px, rgba).unwrap_or(Raster { width: 1, height: 1, rgba: vec![bg.0, bg.1, bg.2, 255] })
}

/// Fill a triangle into one band (rows `row0..`), nearer pixels winning.
fn raster_tri(s: [(f64, f64, f64); 3], rgb: [u8; 3], row0: usize, color: &mut [[u8; 4]], depth: &mut [f32], w: usize) {
    let rows = color.len() / w.max(1);
    let [(x0, y0, z0), (x1, y1, z1), (x2, y2, z2)] = s;
    let area = (x1 - x0) * (y2 - y0) - (x2 - x0) * (y1 - y0);
    if !(area.is_finite() && area.abs() > 1e-12) {
        return;
    }
    let ymin = y0.min(y1).min(y2).floor().max(row0 as f64) as usize;
    let ymax = (y0.max(y1).max(y2).ceil().min((row0 + rows) as f64)).max(0.0) as usize;
    let xmin = x0.min(x1).min(x2).floor().max(0.0) as usize;
    let xmax = x0.max(x1).max(x2).ceil().min(w as f64).max(0.0) as usize;
    for y in ymin..ymax {
        let py = y as f64 + 0.5;
        for x in xmin..xmax {
            let px = x as f64 + 0.5;
            let w0 = ((x1 - px) * (y2 - py) - (x2 - px) * (y1 - py)) / area;
            let w1 = ((x2 - px) * (y0 - py) - (x0 - px) * (y2 - py)) / area;
            let w2 = 1.0 - w0 - w1;
            if w0 < -1e-9 || w1 < -1e-9 || w2 < -1e-9 {
                continue;
            }
            let z = (w0 * z0 + w1 * z1 + w2 * z2) as f32;
            let i = (y - row0) * w + x;
            if let (Some(d), Some(c)) = (depth.get_mut(i), color.get_mut(i))
                && z > *d
            {
                *d = z;
                *c = [rgb[0], rgb[1], rgb[2], 255];
            }
        }
    }
}

/// Draw a line one pixel wide; with `bias`, pixels behind a surface (deeper by more than the
/// bias) are left out.
#[allow(clippy::too_many_arguments)]
fn raster_line(a: (f64, f64, f64), b: (f64, f64, f64), rgb: [u8; 3], color: &mut [[u8; 4]], depth: &[f32], w: usize, h: usize, bias: Option<f32>) {
    let (dx, dy) = (b.0 - a.0, b.1 - a.1);
    let steps = dx.abs().max(dy.abs()).ceil();
    if !steps.is_finite() || steps > 1e6 {
        return;
    }
    let n = (steps as usize).max(1);
    for k in 0..=n {
        let t = k as f64 / n as f64;
        let (x, y, z) = (a.0 + dx * t, a.1 + dy * t, a.2 + (b.2 - a.2) * t);
        if x < 0.0 || y < 0.0 {
            continue;
        }
        let (xi, yi) = (x as usize, y as usize);
        if xi >= w || yi >= h {
            continue;
        }
        let i = yi * w + xi;
        if let Some(bias) = bias
            && depth.get(i).is_some_and(|d| (z as f32) + bias < *d)
        {
            continue;
        }
        if let Some(c) = color.get_mut(i) {
            *c = [rgb[0], rgb[1], rgb[2], 255];
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cadaddy_doc::{Common, Line, Mesh, MeshFace, MeshKind};

    fn cube() -> Mesh {
        let v = |x: f64, y: f64, z: f64| Vec3::new(x, y, z);
        let vertices = vec![
            v(0.0, 0.0, 0.0),
            v(1.0, 0.0, 0.0),
            v(1.0, 1.0, 0.0),
            v(0.0, 1.0, 0.0),
            v(0.0, 0.0, 1.0),
            v(1.0, 0.0, 1.0),
            v(1.0, 1.0, 1.0),
            v(0.0, 1.0, 1.0),
        ];
        let f = |a: [u32; 4]| MeshFace { v: a.to_vec(), hidden: 0 };
        let faces = vec![f([0, 3, 2, 1]), f([4, 5, 6, 7]), f([0, 1, 5, 4]), f([1, 2, 6, 5]), f([2, 3, 7, 6]), f([3, 0, 4, 7])];
        Mesh { vertices, faces, kind: MeshKind::Subd }
    }

    #[test]
    fn styles_parse_by_name_and_keyword() {
        for s in VisualStyle::ALL {
            assert_eq!(VisualStyle::parse(s.name()), Some(s));
        }
        assert_eq!(VisualStyle::parse("e"), Some(VisualStyle::ShadedEdges));
        assert_eq!(VisualStyle::parse("_2dwireframe"), Some(VisualStyle::Wireframe2d));
        assert_eq!(VisualStyle::parse("sketchy"), None);
    }

    /// A unit cube at the origin and a line behind it (at y = 3, across x), seen from the front
    /// (looking along +Y).
    fn front() -> (Drawing, ShadeView) {
        let mut d = Drawing::new_metric();
        d.add(&Space::Model, Common::default(), EntityKind::Mesh(cube())).unwrap();
        d.add(&Space::Model, Common::default(), EntityKind::Line(Line { a: Vec3::new(-1.0, 3.0, 0.5), b: Vec3::new(2.0, 3.0, 0.5) })).unwrap();
        let p = Projection::new(Vec3::new(0.0, -1.0, 0.0));
        let v = ShadeView::new(p, Vec2::new(0.5, 0.5), 4.0, 200, 200);
        (d, v)
    }

    fn pixel(r: &Raster, x: u32, y: u32) -> [u8; 4] {
        r.pixel(x, y).unwrap()
    }

    #[test]
    fn hidden_lines_are_hidden() {
        let (d, v) = front();
        let bg = Rgb(0, 0, 0);
        let s = scene(&d, &Space::Model, 0.01, bg);
        assert_eq!(s.tris.len(), 12);
        // The line crosses the middle row; behind the cube it is hidden, beside it seen.
        let row = 100;
        let hidden = rasterize(&s, &v, VisualStyle::Hidden, bg);
        let wire = rasterize(&s, &v, VisualStyle::Wireframe, bg);
        // x = 0.5 (the cube's middle) is pixel 100; x = -0.75 is pixel 37.5.
        assert_eq!(pixel(&hidden, 100, row), [0, 0, 0, 255], "behind the cube: hidden");
        assert_ne!(pixel(&wire, 100, row), [0, 0, 0, 255], "wireframe sees through");
        assert_ne!(pixel(&hidden, 37, row), [0, 0, 0, 255], "beside the cube: seen");
        // Shaded: the cube's front face is lit, not the background.
        let shaded = rasterize(&s, &v, VisualStyle::Shaded, bg);
        let p = pixel(&shaded, 100, 80);
        assert!(p[0] > 60 && p != [0, 0, 0, 255], "{p:?}");
        // Realistic is brighter where the highlight falls, never darker.
        let real = rasterize(&s, &v, VisualStyle::Realistic, bg);
        assert!(pixel(&real, 100, 80)[0] >= p[0]);
    }

    #[test]
    fn nearer_surfaces_win_whatever_the_order() {
        let mut d = Drawing::new_metric();
        let quad = |y: f64, c: u8| {
            let v = |x: f64, z: f64| Vec3::new(x, y, z);
            let m = Mesh {
                vertices: vec![v(0.0, 0.0), v(1.0, 0.0), v(1.0, 1.0), v(0.0, 1.0)],
                faces: vec![MeshFace { v: vec![0, 1, 2, 3], hidden: 0 }],
                kind: MeshKind::Subd,
            };
            (EntityKind::Mesh(m), Common { color: cadaddy_color::Color::Index(c), ..Common::default() })
        };
        // The far one (y = 2, red) added after the near one (y = 0, blue).
        for (k, c) in [quad(0.0, 5), quad(2.0, 1)] {
            d.add(&Space::Model, c, k).unwrap();
        }
        let v = ShadeView::new(Projection::new(Vec3::new(0.0, -1.0, 0.0)), Vec2::new(0.5, 0.5), 2.0, 64, 64);
        let r = rasterize(&scene(&d, &Space::Model, 0.01, Rgb(0, 0, 0)), &v, VisualStyle::Shaded, Rgb(0, 0, 0));
        let p = pixel(&r, 32, 32);
        assert!(p[2] > p[0], "the near blue face in front: {p:?}");
    }

    #[test]
    fn empty_and_hostile_scenes() {
        let v = ShadeView::new(None, Vec2::ZERO, 10.0, 0, 70_000);
        assert_eq!((v.width_px, v.height_px), (1, MAX_SIDE));
        let s = Scene {
            tris: vec![Tri { p: [Vec3::ZERO; 3], color: Rgb(1, 2, 3) }],
            lines: vec![Line3 { a: Vec3::ZERO, b: Vec3::new(1e300, 0.0, 0.0), color: Rgb(1, 2, 3), edge: false }],
        };
        let r = rasterize(&s, &ShadeView::new(None, Vec2::ZERO, 10.0, 32, 32), VisualStyle::ShadedEdges, Rgb(9, 9, 9));
        assert_eq!(r.pixel(0, 0), Some([9, 9, 9, 255]));
    }
}
