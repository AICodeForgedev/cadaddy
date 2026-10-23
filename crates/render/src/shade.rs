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
    /// Its surface in [`Scene::surfaces`] ([`NO_SURFACE`]: its colour, plain).
    pub mat: u32,
    /// Texture coordinates of its corners (tiles of its material's mapping).
    pub uv: [[f32; 2]; 3],
}

/// [`Tri::mat`] of a triangle with no material: lit in its own colour.
pub const NO_SURFACE: u32 = u32::MAX;

impl Tri {
    /// A triangle in its own colour, no material.
    pub fn plain(p: [Vec3; 3], color: Rgb) -> Tri {
        Tri { p, color, mat: NO_SURFACE, uv: [[0.0; 2]; 3] }
    }
}

/// A material as rendered views draw it: linear base colour (under its texture, if any),
/// roughness, metalness, opacity.
#[derive(Clone, Debug, PartialEq)]
pub struct SurfaceMat {
    pub name: String,
    pub base: [f32; 3],
    pub roughness: f32,
    pub metallic: f32,
    pub opacity: f32,
    /// Its base colour texture in [`Scene::textures`].
    pub texture: Option<u32>,
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
    /// The materials of the triangles that have one.
    pub surfaces: Vec<SurfaceMat>,
    /// Base colour textures of the surfaces.
    pub textures: Vec<std::sync::Arc<Raster>>,
    /// What lights the scene (the Realistic style).
    pub lighting: Option<crate::light::Lighting>,
    /// The object each triangle belongs to (as [`scene`] builds it; parallel to `tris`).
    pub owners: Vec<Handle>,
    /// Baked indirect light ([`crate::lightmap`]) for the triangles that have it.
    pub baked: Option<crate::lightmap::Baked>,
    /// The object whose triangles are being added.
    pub(crate) owner_now: Handle,
}

/// Texture size generated for a library material, pixels.
pub const TEXTURE_PX: u32 = 256;

/// The surfaces of a scene being built, by material name.
#[derive(Default)]
struct Surfaces {
    by_name: std::collections::HashMap<String, u32>,
    by_texture: std::collections::HashMap<String, u32>,
}

impl Surfaces {
    /// The surface index of material `m` (added on first use, with its texture).
    fn index(&mut self, d: &Drawing, out: &mut Scene, m: &cadaddy_doc::material::Material) -> u32 {
        if let Some(i) = self.by_name.get(&m.name) {
            return *i;
        }
        let lin = |v: u8| (f32::from(v) / 255.0).powf(2.2);
        let texture = m.color_map.as_ref().and_then(|t| {
            use cadaddy_doc::material::Texture;
            let key = match t {
                Texture::Library(id) => format!("lib:{id}"),
                Texture::File(p) => format!("file:{p}"),
            };
            if let Some(i) = self.by_texture.get(&key) {
                return Some(*i);
            }
            let raster: Option<std::sync::Arc<Raster>> = match t {
                Texture::Library(id) => cadaddy_doc::material::recipe(id).and_then(|r| {
                    let maps = crate::materials::generate(r, TEXTURE_PX);
                    Raster::new(maps.size, maps.size, maps.color).map(std::sync::Arc::new)
                }),
                // The drawing's own copy first (it travels with a .cdy file), else the file.
                Texture::File(p) => {
                    d.embedded_texture(p).and_then(|b| crate::images::embedded(b)).or_else(|| crate::images::get(p)).map(|r| r.fit(1024))
                }
            };
            let r = raster?;
            let i = u32::try_from(out.textures.len()).ok()?;
            out.textures.push(r);
            self.by_texture.insert(key, i);
            Some(i)
        });
        let i = u32::try_from(out.surfaces.len()).unwrap_or(NO_SURFACE);
        out.surfaces.push(SurfaceMat {
            name: m.name.clone(),
            // A texture carries the colour; the base colour is then its tint (white).
            base: if texture.is_some() { [1.0; 3] } else { [lin(m.color.0), lin(m.color.1), lin(m.color.2)] },
            roughness: m.roughness as f32,
            metallic: m.metallic as f32,
            opacity: m.opacity as f32,
            texture,
        });
        self.by_name.insert(m.name.clone(), i);
        i
    }
}

/// A triangle with object `e`'s material (if it has one) and texture coordinates from its mapping.
fn material_tri(d: &Drawing, e: &cadaddy_doc::Entity, p: [Vec3; 3], color: Rgb, out: &mut Scene, surfaces: &mut Surfaces) -> Tri {
    let Some(m) = d.material_of(e, None) else { return Tri::plain(p, color) };
    let mat = surfaces.index(d, out, m);
    let mapping = cadaddy_doc::material::object_mapping(&e.common.xdata).unwrap_or(m.mapping);
    let n = (p[1] - p[0]).cross(p[2] - p[0]).normalized();
    let n = if n.is_finite() { n } else { Vec3::Z };
    let uv = p.map(|q| {
        let (u, v) = mapping.uv(q, n);
        [u as f32, v as f32]
    });
    Tri { p, color, mat, uv }
}

/// Most triangles and lines a scene holds (beyond them the rest is left out).
pub const MAX_SCENE: usize = 20_000_000;

/// The colour of faces a live section cuts.
pub const CUT_COLOR: Rgb = Rgb(214, 96, 77);

/// The scene of `space`: surfaces and lines in world coordinates, curves tessellated to `tol`
/// world units, colour 7 as it shows on `bg`. Extra `clip_planes` (e.g. from a section box) are
/// appended to the live section planes for clipping.
pub fn scene(d: &Drawing, space: &Space, tol: f64, bg: Rgb) -> Scene {
    scene_clipped(d, space, tol, bg, &[])
}

/// Like [`scene`] but with additional clipping planes (section box faces).
pub fn scene_clipped(d: &Drawing, space: &Space, tol: f64, bg: Rgb, extra_planes: &[cadaddy_doc::SectionPlane]) -> Scene {
    let Some(store) = d.space(space) else { return Scene::default() };
    let list = build(d, space, &Options { tolerance: tol.max(1e-9), ..Options::default() });
    // Each object's colours (its first primitive's): its lines as drawn on `bg` (colour 7 black on
    // white), its surfaces in their own colour (colour 7 is white there).
    let mut colors: std::collections::HashMap<Handle, (Rgb, Rgb)> = std::collections::HashMap::new();
    for p in &list.prims {
        colors.entry(p.handle).or_insert_with(|| (p.display_rgb(bg), p.color));
    }
    let ink = if bg.luma() > 0.5 { Rgb(0, 0, 0) } else { Rgb(255, 255, 255) };
    let mut out = Scene { lighting: Some(crate::light::Lighting::of(d, space)), ..Scene::default() };
    let mut surfaces = Surfaces::default();
    let mut native: std::collections::HashSet<Handle> = std::collections::HashSet::new();
    let mut elevation: std::collections::HashMap<Handle, f64> = std::collections::HashMap::new();
    let mut planes = crate::live::live_sections(d, space);
    planes.extend_from_slice(extra_planes);
    // Clipped by the live sections, if any.
    // A lit triangle (its material and texture coordinates, `material_tri`) clipped by them.
    let tri = |out: &mut Scene, t: Tri| {
        if planes.is_empty() {
            push_tri(out, t);
            return;
        }
        let pts = crate::live::clip_polygon(t.p.to_vec(), &planes);
        for k in 1..pts.len().saturating_sub(1) {
            if let (Some(a), Some(b), Some(c)) = (pts.first(), pts.get(k), pts.get(k + 1)) {
                push_tri(out, Tri { p: [*a, *b, *c], ..t });
            }
        }
    };
    let line = |out: &mut Scene, a: Vec3, b: Vec3, color: Rgb, edge: bool| {
        if let Some((a, b)) = crate::live::clip_segment(a, b, &planes) {
            push_line(out, Line3 { a, b, color, edge });
        }
    };
    for e in store.iter() {
        if !d.is_visible(e) {
            continue;
        }
        let (color, surface) = colors.get(&e.handle).copied().unwrap_or((ink, Rgb(255, 255, 255)));
        out.owner_now = e.handle;
        match &e.kind {
            EntityKind::Mesh(m) => {
                native.insert(e.handle);
                for t in m.triangles() {
                    let tri_ = material_tri(d, e, t, surface, &mut out, &mut surfaces);
                    tri(&mut out, tri_);
                }
                for (a, b) in m.edges() {
                    if let (Some(a), Some(b)) = (m.vertices.get(a as usize), m.vertices.get(b as usize)) {
                        line(&mut out, *a, *b, color, true);
                    }
                }
            }
            EntityKind::Solid3d(so) if let Some((cut, first)) = crate::live::cut_solid(so, &planes) => {
                native.insert(e.handle);
                for p in cut.polys(tol).iter() {
                    let c = if cadaddy_doc::tag_leaf(p.tag) >= first { CUT_COLOR } else { surface };
                    for k in 1..p.pts.len().saturating_sub(1) {
                        push_tri(&mut out, Tri::plain([p.pts[0], p.pts[k], p.pts[k + 1]], c));
                    }
                }
                for w in cut.wires(tol) {
                    for seg in w.windows(2) {
                        if let [a, b] = seg {
                            push_line(&mut out, Line3 { a: *a, b: *b, color, edge: true });
                        }
                    }
                }
            }
            EntityKind::Solid3d(so) => {
                native.insert(e.handle);
                // Lit with its material like a mesh.
                for t in crate::solid_mesh(so, tol).triangles() {
                    let tri = material_tri(d, e, t, surface, &mut out, &mut surfaces);
                    push_tri(&mut out, tri);
                }
                for w in so.wires(tol) {
                    for seg in w.windows(2) {
                        if let [a, b] = seg {
                            push_line(&mut out, Line3 { a: *a, b: *b, color, edge: true });
                        }
                    }
                }
            }
            EntityKind::Surface(sf) => {
                native.insert(e.handle);
                for t in sf.mesh(tol).triangles() {
                    let tri = material_tri(d, e, t, surface, &mut out, &mut surfaces);
                    push_tri(&mut out, tri);
                }
                for w in sf.wires(tol) {
                    for seg in w.windows(2) {
                        if let [a, b] = seg {
                            push_line(&mut out, Line3 { a: *a, b: *b, color, edge: true });
                        }
                    }
                }
            }
            EntityKind::Face3d(f) => {
                native.insert(e.handle);
                let [a, b, c, dd] = f.corners;
                let tri_ = material_tri(d, e, [a, b, c], surface, &mut out, &mut surfaces);
                tri(&mut out, tri_);
                if c != dd {
                    let tri_ = material_tri(d, e, [a, c, dd], surface, &mut out, &mut surfaces);
                    tri(&mut out, tri_);
                }
                let corners = if c == dd { vec![a, b, c] } else { vec![a, b, c, dd] };
                let n = corners.len();
                for i in 0..n {
                    if f.hidden_edges & (1 << i) == 0
                        && let (Some(p), Some(q)) = (corners.get(i), corners.get((i + 1) % n))
                    {
                        line(&mut out, *p, *q, color, true);
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
                            // A section plane's own outline isn't cut.
                            if matches!(k, EntityKind::SectionPlane(_)) {
                                push_line(&mut out, Line3 { a: *p, b: *q, color, edge: false });
                            } else {
                                line(&mut out, *p, *q, color, false);
                            }
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
        out.owner_now = p.handle;
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
                    push_tri(&mut out, Tri::plain([lift(a), lift(b), lift(c)], p.color));
                }
            }
            _ => {}
        }
    }
    let bounds = scene_box(&out);
    if let Some(lit) = out.lighting.as_mut() {
        lit.bounds = bounds;
    }
    out
}

/// The box round everything `s` holds (fog distances start at its front); `None` when empty.
pub fn scene_box(s: &Scene) -> Option<(Vec3, Vec3)> {
    let (mut lo, mut hi) =
        (Vec3::new(f64::INFINITY, f64::INFINITY, f64::INFINITY), Vec3::new(f64::NEG_INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY));
    for p in s.tris.iter().flat_map(|t| t.p).chain(s.lines.iter().flat_map(|l| [l.a, l.b])) {
        lo = Vec3::new(lo.x.min(p.x), lo.y.min(p.y), lo.z.min(p.z));
        hi = Vec3::new(hi.x.max(p.x), hi.y.max(p.y), hi.z.max(p.z));
    }
    (lo.x <= hi.x).then_some((lo, hi))
}

pub(crate) fn push_tri(s: &mut Scene, t: Tri) {
    if s.tris.len() < MAX_SCENE && t.p.iter().all(|p| p.is_finite()) {
        s.tris.push(t);
        s.owners.push(s.owner_now);
    }
}

pub(crate) fn push_line(s: &mut Scene, l: Line3) {
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
    rasterize_lit(scene, v, style, bg, scene.lighting.as_ref())
}

/// [`rasterize`] lit by `lit` in place of the scene's own lighting (the live view's, with the
/// render environment applied).
pub fn rasterize_lit(scene: &Scene, v: &ShadeView, style: VisualStyle, bg: Rgb, lit: Option<&crate::light::Lighting>) -> Raster {
    if style == VisualStyle::Realistic
        && let Some(lit) = lit
    {
        return realistic(scene, v, lit);
    }
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

/// Shadow map size (texels on a side).
const SHADOW_PX: usize = 2048;

/// Depth towards the sun over the whole scene, for the sun's shadows.
struct ShadowMap {
    u: Vec3,
    v: Vec3,
    d: Vec3,
    lo: (f64, f64),
    /// Texels per world unit.
    k: f64,
    n: usize,
    depth: Vec<f32>,
    /// Depth slack: a fraction of the scene's depth plus a texel.
    bias: f32,
    /// The scene's depth along the sun (world units).
    range: f64,
}

impl ShadowMap {
    fn new(scene: &Scene, sun: Vec3) -> Option<ShadowMap> {
        let d = sun.normalized();
        let helper = if d.z.abs() < 0.9 { Vec3::Z } else { Vec3::new(1.0, 0.0, 0.0) };
        let u = helper.cross(d).normalized();
        let v = d.cross(u).normalized();
        let pts = scene.tris.iter().filter(|t| scene.surfaces.get(t.mat as usize).is_none_or(|s| s.opacity > 0.5)).flat_map(|t| t.p);
        let (mut lo, mut hi, mut zlo, mut zhi) = ((f64::MAX, f64::MAX), (f64::MIN, f64::MIN), f64::MAX, f64::MIN);
        for p in pts {
            let (a, b, c) = (p.dot(u), p.dot(v), p.dot(d));
            lo = (lo.0.min(a), lo.1.min(b));
            hi = (hi.0.max(a), hi.1.max(b));
            zlo = zlo.min(c);
            zhi = zhi.max(c);
        }
        let span = (hi.0 - lo.0).max(hi.1 - lo.1);
        if !(span.is_finite() && span > 0.0) {
            return None;
        }
        let n = SHADOW_PX;
        let k = (n as f64 - 2.0) / span;
        let mut depth = vec![f32::NEG_INFINITY; n * n];
        for t in &scene.tris {
            if scene.surfaces.get(t.mat as usize).is_some_and(|s| s.opacity <= 0.5) {
                continue;
            }
            let s = t.p.map(|p| ((p.dot(u) - lo.0) * k + 1.0, (p.dot(v) - lo.1) * k + 1.0, p.dot(d)));
            raster_depth(s, &mut depth, n);
        }
        let range = (zhi - zlo).max(1e-9);
        let bias = (range * 2e-3 + 2.0 / k) as f32;
        Some(ShadowMap { u, v, d, lo, k, n, depth, bias, range })
    }

    /// Point `p` in the map: texel coordinates and depth towards the sun.
    fn coords(&self, p: Vec3) -> (f64, f64, f32) {
        ((p.dot(self.u) - self.lo.0) * self.k + 1.0, (p.dot(self.v) - self.lo.1) * self.k + 1.0, p.dot(self.d) as f32)
    }

    /// The nearest-to-the-sun depth in texel `(i, j)` (−∞ outside the map).
    fn at(&self, i: i64, j: i64) -> f32 {
        if i < 0 || j < 0 || i >= self.n as i64 || j >= self.n as i64 {
            return f32::NEG_INFINITY;
        }
        self.depth.get(j as usize * self.n + i as usize).copied().unwrap_or(f32::NEG_INFINITY)
    }

    /// How lit by the sun point `p` is (0 shadowed … 1 lit), averaged over 3×3 texels.
    fn lit(&self, p: Vec3) -> f32 {
        let (x, y, z) = self.coords(p);
        if !(x.is_finite() && y.is_finite()) {
            return 1.0;
        }
        let (cx, cy) = (x.floor() as i64, y.floor() as i64);
        let mut sum = 0.0;
        for dy in -1..=1 {
            for dx in -1..=1 {
                sum += if self.at(cx + dx, cy + dy) > z + self.bias { 0.0 } else { 1.0 };
            }
        }
        sum / 9.0
    }

    /// Lit (1) or not (0) at texel coordinates `(x, y)`, filtered between the four nearest
    /// texels.
    fn compare(&self, x: f64, y: f64, z: f32) -> f32 {
        let (fx, fy) = (x - 0.5, y - 0.5);
        let (i, j) = (fx.floor(), fy.floor());
        let (tx, ty) = ((fx - i) as f32, (fy - j) as f32);
        let (i, j) = (i as i64, j as i64);
        let lit = |a: i64, b: i64| if self.at(a, b) > z + self.bias { 0.0 } else { 1.0 };
        let top = lit(i, j) * (1.0 - tx) + lit(i + 1, j) * tx;
        let bottom = lit(i, j + 1) * (1.0 - tx) + lit(i + 1, j + 1) * tx;
        top * (1.0 - ty) + bottom * ty
    }

    /// How lit point `p` is by a sun whose apparent radius has tangent `tan_half`:
    /// percentage-closer soft shadows. The casters near the point set the penumbra's width
    /// (sharp where an object meets the ground, wider farther from it), then the map is filtered
    /// over that width. `tan_half` 0: [`ShadowMap::lit`].
    fn lit_soft(&self, p: Vec3, tan_half: f64) -> f32 {
        if !(tan_half.is_finite() && tan_half > 0.0) {
            return self.lit(p);
        }
        let (x, y, z) = self.coords(p);
        if !(x.is_finite() && y.is_finite()) {
            return 1.0;
        }
        use crate::ao::{SOFT_MAX_TEXELS, SOFT_TAPS, disc};
        // Casters: within the widest penumbra the scene allows.
        let search = (tan_half * self.range * self.k).clamp(1.0, SOFT_MAX_TEXELS);
        let (mut sum, mut count) = (0.0f64, 0u32);
        for k in 0..SOFT_TAPS {
            let (dx, dy) = disc(k, SOFT_TAPS);
            let occ = self.at((x + dx * search).floor() as i64, (y + dy * search).floor() as i64);
            if occ > z + self.bias {
                sum += f64::from(occ);
                count += 1;
            }
        }
        if count == 0 {
            return 1.0;
        }
        let blocker = sum / f64::from(count);
        let radius = ((blocker - f64::from(z)) * tan_half * self.k).clamp(1.0, SOFT_MAX_TEXELS);
        let mut lit = 0.0;
        for k in 0..SOFT_TAPS {
            let (dx, dy) = disc(k, SOFT_TAPS);
            lit += self.compare(x + dx * radius, y + dy * radius, z);
        }
        lit / SOFT_TAPS as f32
    }
}

/// Fill a triangle's depth (larger nearer the light) into a square map.
fn raster_depth(s: [(f64, f64, f64); 3], depth: &mut [f32], n: usize) {
    let [(x0, y0, z0), (x1, y1, z1), (x2, y2, z2)] = s;
    let area = (x1 - x0) * (y2 - y0) - (x2 - x0) * (y1 - y0);
    if !(area.is_finite() && area.abs() > 1e-12) {
        return;
    }
    let ymin = y0.min(y1).min(y2).floor().max(0.0) as usize;
    let ymax = y0.max(y1).max(y2).ceil().min(n as f64).max(0.0) as usize;
    let xmin = x0.min(x1).min(x2).floor().max(0.0) as usize;
    let xmax = x0.max(x1).max(x2).ceil().min(n as f64).max(0.0) as usize;
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
            if let Some(d) = depth.get_mut(y * n + x)
                && z > *d
            {
                *d = z;
            }
        }
    }
}

/// A texture's colour at `uv` (tiles; wrapping, bilinear), linear.
pub(crate) fn sample(r: &Raster, uv: [f32; 2]) -> [f32; 3] {
    let (w, h) = (r.width.max(1) as usize, r.height.max(1) as usize);
    let x = uv[0].rem_euclid(1.0) * w as f32 - 0.5;
    let y = (1.0 - uv[1].rem_euclid(1.0)) * h as f32 - 0.5;
    let (x0, y0) = (x.floor(), y.floor());
    let (fx, fy) = (x - x0, y - y0);
    let at = |i: f32, j: f32| -> [f32; 3] {
        let (i, j) = ((i as i64).rem_euclid(w as i64) as usize, (j as i64).rem_euclid(h as i64) as usize);
        let k = (j * w + i) * 4;
        let ch = |o: usize| (f32::from(r.rgba.get(k + o).copied().unwrap_or(128)) / 255.0).powf(2.2);
        [ch(0), ch(1), ch(2)]
    };
    let (a, b, c, d) = (at(x0, y0), at(x0 + 1.0, y0), at(x0, y0 + 1.0), at(x0 + 1.0, y0 + 1.0));
    [0, 1, 2].map(|i| (a[i] * (1.0 - fx) + b[i] * fx) * (1.0 - fy) + (c[i] * (1.0 - fx) + d[i] * fx) * fy)
}

/// Barycentric weights of pixel `(x, y)`'s centre in screen triangle `s`; `None` outside it.
fn weights(s: &[(f64, f64, f64); 3], x: usize, y: usize) -> Option<[f64; 3]> {
    let [(x0, y0, _), (x1, y1, _), (x2, y2, _)] = *s;
    let area = (x1 - x0) * (y2 - y0) - (x2 - x0) * (y1 - y0);
    if !(area.is_finite() && area.abs() > 1e-12) {
        return None;
    }
    let (px, py) = (x as f64 + 0.5, y as f64 + 0.5);
    let w0 = ((x1 - px) * (y2 - py) - (x2 - px) * (y1 - py)) / area;
    let w1 = ((x2 - px) * (y0 - py) - (x0 - px) * (y2 - py)) / area;
    let w2 = 1.0 - w0 - w1;
    (w0 >= -1e-9 && w1 >= -1e-9 && w2 >= -1e-9).then_some([w0, w1, w2])
}

/// A triangle of the Realistic style on the screen.
struct ScreenTri {
    s: [(f64, f64, f64); 3],
    /// Its index in the scene.
    tri: usize,
    /// Its normal, turned towards the viewer.
    n: Vec3,
    y0: usize,
    y1: usize,
    opaque: bool,
}

impl ScreenTri {
    /// Call `f(x, y, depth)` for each pixel of rows `row0 .. row0 + rows` the triangle covers.
    fn cover(&self, row0: usize, rows: usize, w: usize, mut f: impl FnMut(usize, usize, f32)) {
        let [(x0, y0, z0), (x1, y1, z1), (x2, y2, z2)] = self.s;
        let ymin = y0.min(y1).min(y2).floor().max(row0 as f64) as usize;
        let ymax = (y0.max(y1).max(y2).ceil().min((row0 + rows) as f64)).max(0.0) as usize;
        let xmin = x0.min(x1).min(x2).floor().max(0.0) as usize;
        let xmax = x0.max(x1).max(x2).ceil().min(w as f64).max(0.0) as usize;
        for y in ymin..ymax {
            for x in xmin..xmax {
                if let Some([w0, w1, w2]) = weights(&self.s, x, y) {
                    f(x, y, (w0 * z0 + w1 * z1 + w2 * z2) as f32);
                }
            }
        }
    }
}

/// The Realistic style: every pixel of every surface lit physically ([`crate::light`]) by the
/// sky (dimmed in corners and creases by screen-space ambient occlusion, [`crate::ao`]), the sun
/// (with soft shadows) and the lights, through its material and texture; glass and other
/// see-through surfaces over what lies behind; line objects on top; the sky behind; all through
/// the tone curve.
///
/// Opaque surfaces go first into a visibility buffer (the nearest triangle at each pixel); the
/// occlusion is worked out from its depths; then each pixel is lit once.
fn realistic(scene: &Scene, v: &ShadeView, lit: &crate::light::Lighting) -> Raster {
    use crate::light::{Background, Surface, apply_fog, background_image_pixel, fog_front, shade_occluded, view_background};
    // Which triangle a pixel shows: none (the background).
    const NONE: u32 = u32::MAX;
    let (w, h) = (v.width_px as usize, v.height_px as usize);
    // The background: sky gradient, solid, gradient or image (it varies across only as an image).
    let mut color: Vec<[u8; 4]> = Vec::with_capacity(w * h);
    let across = matches!(lit.background, Background::Image(_));
    for y in 0..h {
        let t = 1.0 - (y as f32 + 0.5) / h as f32;
        if across {
            color.extend((0..w).map(|x| {
                let u = (x as f32 + 0.5) / w as f32;
                let c = background_image_pixel(lit, u, t).unwrap_or_else(|| lit.display(view_background(lit, u, t)));
                [c[0], c[1], c[2], 255]
            }));
        } else {
            let c = lit.display(view_background(lit, 0.5, t));
            color.extend(std::iter::repeat_n([c[0], c[1], c[2], 255], w));
        }
    }
    // Fog: by each surface point's distance from the front of the scene, in linear light.
    let front = fog_front(lit, v.dir.normalized());
    let mut depth = vec![f32::NEG_INFINITY; w * h];
    let mut owner = vec![NONE; w * h];
    let shadows = lit.sun.and_then(|(d, _)| ShadowMap::new(scene, d));
    let tan_half = sun_tan_half(lit.sun_size);
    let view_dir = v.dir.normalized();
    let tris: Vec<ScreenTri> = scene
        .tris
        .iter()
        .enumerate()
        .filter_map(|(i, t)| {
            let s = t.p.map(|p| v.screen(p));
            let n = (t.p[1] - t.p[0]).cross(t.p[2] - t.p[0]);
            let len = n.len();
            if !(len.is_finite() && len > 1e-300) {
                return None;
            }
            let n = n * (1.0 / len);
            let n = if n.dot(view_dir) < 0.0 { -n } else { n };
            let ys = s.map(|p| p.1);
            let y0 = ys.iter().copied().fold(f64::INFINITY, f64::min).floor().max(0.0);
            let y1 = ys.iter().copied().fold(f64::NEG_INFINITY, f64::max).ceil().min(h as f64);
            if !(y0.is_finite() && y1.is_finite()) || y1 <= y0 {
                return None;
            }
            let opaque = scene.surfaces.get(t.mat as usize).is_none_or(|m| m.opacity > 0.99);
            Some(ScreenTri { s, tri: i, n, y0: y0 as usize, y1: y1 as usize, opaque })
        })
        .collect();
    let lin = |c: u8| (f32::from(c) / 255.0).powf(2.2);
    // Colour of triangle `tri` at barycentric weights `b`, `open` to the sky (ambient
    // occlusion): material, texture, light.
    let pixel = |ti: usize, tri: &Tri, n: Vec3, b: [f64; 3], open: f32| -> ([u8; 3], f32) {
        let p = tri.p[0] * b[0] + tri.p[1] * b[1] + tri.p[2] * b[2];
        let (surface, opacity) = match scene.surfaces.get(tri.mat as usize) {
            Some(m) => {
                let mut base = m.base;
                if let Some(r) = m.texture.and_then(|i| scene.textures.get(i as usize)) {
                    let uv =
                        [0, 1].map(|k| (f64::from(tri.uv[0][k]) * b[0] + f64::from(tri.uv[1][k]) * b[1] + f64::from(tri.uv[2][k]) * b[2]) as f32);
                    let tex = sample(r, uv);
                    base = [base[0] * tex[0], base[1] * tex[1], base[2] * tex[2]];
                }
                (Surface { base, roughness: m.roughness, metallic: m.metallic }, m.opacity)
            }
            None => (Surface { base: [lin(tri.color.0), lin(tri.color.1), lin(tri.color.2)], roughness: 0.6, metallic: 0.0 }, 1.0),
        };
        let sun = shadows.as_ref().map_or(1.0, |m| m.lit_soft(p + n * f64::from(m.bias) * 0.5, tan_half));
        // Its back is the side seen when the corners turn clockwise from here.
        let back = (tri.p[1] - tri.p[0]).cross(tri.p[2] - tri.p[0]).dot(n) < 0.0;
        let baked = scene.baked.as_ref().and_then(|bk| bk.at(ti, b[1], b[2], back));
        let mut c = shade_occluded(lit, p, n, view_dir, &surface, sun, baked, open);
        if let Some(fog) = &lit.fog {
            c = apply_fog(c, (front - p.dot(view_dir)) as f32, fog);
        }
        (lit.display(c), opacity)
    };
    let bands = h.div_ceil(BAND).max(1);
    let mut binned: Vec<Vec<usize>> = vec![Vec::new(); bands];
    for (i, t) in tris.iter().enumerate() {
        for b in t.y0 / BAND..=(t.y1.saturating_sub(1) / BAND).min(bands - 1) {
            if let Some(list) = binned.get_mut(b) {
                list.push(i);
            }
        }
    }
    // 1. The nearest opaque surface at each pixel.
    let visible = |(b, (zband, oband)): (usize, (&mut [f32], &mut [u32]))| {
        let row0 = b * BAND;
        let rows = zband.len() / w.max(1);
        for &i in binned.get(b).map(Vec::as_slice).unwrap_or(&[]) {
            let Some(t) = tris.get(i).filter(|t| t.opaque) else { continue };
            t.cover(row0, rows, w, |x, y, z| {
                let k = (y - row0) * w + x;
                if let (Some(d), Some(o)) = (zband.get_mut(k), oband.get_mut(k))
                    && z > *d
                {
                    *d = z;
                    *o = i as u32;
                }
            });
        }
    };
    #[cfg(not(target_arch = "wasm32"))]
    {
        use rayon::prelude::*;
        depth.par_chunks_mut(BAND * w).zip(owner.par_chunks_mut(BAND * w)).enumerate().for_each(visible);
    }
    #[cfg(target_arch = "wasm32")]
    depth.chunks_mut(BAND * w).zip(owner.chunks_mut(BAND * w)).enumerate().for_each(visible);
    // 2. How open each pixel is to the sky.
    let px = v.height / f64::from(v.height_px);
    let open = if lit.ao > 0.0 {
        let normal = |i: usize| {
            let t = tris.get(*owner.get(i)? as usize)?;
            Some([t.n.dot(v.right), t.n.dot(v.up), t.n.dot(view_dir)])
        };
        let reach = crate::light::ao_reach(lit.ao_radius, scene_diag(scene));
        crate::ao::occlusion(&depth, normal, w, h, px, reach, lit.ao)
    } else {
        Vec::new()
    };
    // 3. Each pixel lit once.
    let shade_row = |(y, row): (usize, &mut [[u8; 4]])| {
        for (x, c) in row.iter_mut().enumerate() {
            let i = y * w + x;
            let Some(t) = owner.get(i).filter(|o| **o != NONE).and_then(|o| tris.get(*o as usize)) else { continue };
            let Some(tri) = scene.tris.get(t.tri) else { continue };
            let Some(b) = weights(&t.s, x, y) else { continue };
            let (rgb, _) = pixel(t.tri, tri, t.n, b, open.get(i).copied().unwrap_or(1.0));
            *c = [rgb[0], rgb[1], rgb[2], 255];
        }
    };
    #[cfg(not(target_arch = "wasm32"))]
    {
        use rayon::prelude::*;
        color.par_chunks_mut(w.max(1)).enumerate().for_each(shade_row);
    }
    #[cfg(target_arch = "wasm32")]
    color.chunks_mut(w.max(1)).enumerate().for_each(shade_row);
    // 4. See-through surfaces over them, farthest first.
    let glass = |(b, (cband, zband)): (usize, (&mut [[u8; 4]], &[f32]))| {
        let row0 = b * BAND;
        let rows = cband.len() / w.max(1);
        let mut clear: Vec<&ScreenTri> =
            binned.get(b).map(Vec::as_slice).unwrap_or(&[]).iter().filter_map(|&i| tris.get(i)).filter(|t| !t.opaque).collect();
        clear.sort_by(|a, b| {
            let z = |t: &ScreenTri| t.s.iter().map(|p| p.2).sum::<f64>();
            z(a).total_cmp(&z(b))
        });
        for t in clear {
            let Some(tri) = scene.tris.get(t.tri) else { continue };
            t.cover(row0, rows, w, |x, y, z| {
                let k = (y - row0) * w + x;
                if zband.get(k).is_none_or(|d| z <= *d) {
                    return;
                }
                let Some(b) = weights(&t.s, x, y) else { return };
                let (rgb, opacity) = pixel(t.tri, tri, t.n, b, 1.0);
                if let Some(c) = cband.get_mut(k) {
                    let a = opacity.clamp(0.0, 1.0);
                    let mix = |o: u8, n: u8| (f32::from(o) * (1.0 - a) + f32::from(n) * a).round() as u8;
                    *c = [mix(c[0], rgb[0]), mix(c[1], rgb[1]), mix(c[2], rgb[2]), 255];
                }
            });
        }
    };
    if tris.iter().any(|t| !t.opaque) {
        #[cfg(not(target_arch = "wasm32"))]
        {
            use rayon::prelude::*;
            color.par_chunks_mut(BAND * w).zip(depth.par_chunks(BAND * w)).enumerate().for_each(glass);
        }
        #[cfg(target_arch = "wasm32")]
        color.chunks_mut(BAND * w).zip(depth.chunks(BAND * w)).enumerate().for_each(glass);
    }
    // Line objects on top (surface edges are not drawn in this style).
    let bias = (px * 1.5) as f32;
    for l in scene.lines.iter().filter(|l| !l.edge) {
        let (a, b) = (v.screen(l.a), v.screen(l.b));
        raster_line(a, b, [l.color.0, l.color.1, l.color.2], &mut color, &depth, w, h, Some(bias));
    }
    let rgba: Vec<u8> = color.into_iter().flatten().collect();
    Raster::new(v.width_px, v.height_px, rgba).unwrap_or(Raster { width: 1, height: 1, rgba: vec![0, 0, 0, 255] })
}

/// The tangent of the sun's apparent radius for an apparent size of `degrees` (0: a point).
pub fn sun_tan_half(degrees: f32) -> f64 {
    let d = f64::from(degrees);
    if d.is_finite() && d > 0.0 { (d.min(cadaddy_doc::lighting::Look::MAX_SOFT) * 0.5).to_radians().tan() } else { 0.0 }
}

/// The diagonal of everything `scene` holds (the ambient occlusion's reach by default).
pub fn scene_diag(scene: &Scene) -> f64 {
    let (mut lo, mut hi) = (Vec3::new(f64::MAX, f64::MAX, f64::MAX), Vec3::new(f64::MIN, f64::MIN, f64::MIN));
    for p in scene.tris.iter().flat_map(|t| t.p).chain(scene.lines.iter().flat_map(|l| [l.a, l.b])).filter(|p| p.is_finite()) {
        lo = Vec3::new(lo.x.min(p.x), lo.y.min(p.y), lo.z.min(p.z));
        hi = Vec3::new(hi.x.max(p.x), hi.y.max(p.y), hi.z.max(p.z));
    }
    let d = (hi - lo).len();
    if d.is_finite() { d } else { 0.0 }
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

    /// A 10 × 10 floor at z 0 and a 2 × 2 × 2 box on it, in a metric drawing; the box made of
    /// `box_material` when given.
    fn courtyard(box_material: Option<&str>) -> Drawing {
        let mut d = Drawing::new_metric();
        let v = |x: f64, y: f64, z: f64| Vec3::new(x, y, z);
        let floor = Mesh {
            vertices: vec![v(-5.0, -5.0, 0.0), v(5.0, -5.0, 0.0), v(5.0, 5.0, 0.0), v(-5.0, 5.0, 0.0)],
            faces: vec![MeshFace { v: vec![0, 1, 2, 3], hidden: 0 }],
            kind: MeshKind::Subd,
        };
        d.add(&Space::Model, Common::default(), EntityKind::Mesh(floor)).unwrap();
        let mut b = cube();
        for p in &mut b.vertices {
            *p = Vec3::new(p.x * 2.0 - 1.0, p.y * 2.0 - 1.0, p.z * 2.0);
        }
        let mut c = Common::default();
        if let Some(id) = box_material {
            let m = cadaddy_doc::material::recipe(id).unwrap().material(1.0);
            c.material = Some(m.name.clone()).into();
            d.materials.push(m);
        }
        d.add(&Space::Model, c, EntityKind::Mesh(b)).unwrap();
        d
    }

    #[test]
    fn the_sun_casts_shadows() {
        let d = courtyard(None);
        let s = scene(&d, &Space::Model, 0.01, Rgb(0, 0, 0));
        let lit = s.lighting.clone().unwrap();
        let (sun, _) = lit.sun.unwrap();
        let map = ShadowMap::new(&s, sun).unwrap();
        // Behind the box from the sun (along the sun's direction on the ground): shadowed; on
        // the sunny side and far to one side: lit.
        let away = Vec3::new(-sun.x, -sun.y, 0.0).normalized();
        let behind = away * 1.6 + Vec3::new(0.0, 0.0, 0.001);
        let sunny = -away * 2.5 + Vec3::new(0.0, 0.0, 0.001);
        assert!(map.lit(behind) < 0.2, "{}", map.lit(behind));
        assert!(map.lit(sunny) > 0.9, "{}", map.lit(sunny));
    }

    #[test]
    fn realistic_renders_materials_light_and_exposure() {
        let p = Projection::new(Vec3::new(-1.0, -1.0, 1.0));
        let view = |s: &Scene| fit(s, p, 160, 120).unwrap();
        let plain = scene(&courtyard(None), &Space::Model, 0.01, Rgb(0, 0, 0));
        let brick = scene(&courtyard(Some("brick-red")), &Space::Model, 0.01, Rgb(0, 0, 0));
        assert_eq!((brick.surfaces.len(), brick.textures.len()), (1, 1));
        let a = rasterize(&plain, &view(&plain), VisualStyle::Realistic, Rgb(0, 0, 0));
        let b = rasterize(&brick, &view(&brick), VisualStyle::Realistic, Rgb(0, 0, 0));
        // Deterministic.
        assert_eq!(a.rgba, rasterize(&plain, &view(&plain), VisualStyle::Realistic, Rgb(0, 0, 0)).rgba);
        // The brick box is red and textured: its pixels vary; the plain box's face is even.
        let centre = |r: &Raster| (r.width / 2, r.height / 2);
        let (cx, cy) = centre(&b);
        let patch = |r: &Raster| -> Vec<[u8; 4]> { (0..6).flat_map(|dy| (0..6).filter_map(move |dx| r.pixel(cx - 3 + dx, cy - 3 + dy))).collect() };
        let distinct = |v: Vec<[u8; 4]>| v.into_iter().collect::<std::collections::HashSet<_>>().len();
        assert!(distinct(patch(&b)) > distinct(patch(&a)), "texture shows");
        let px = b.pixel(cx, cy).unwrap();
        assert!(px[0] > px[2], "brick is red: {px:?}");
        // Exposure: two stops brighter.
        let mut bright = courtyard(None);
        bright.sun.exposure = 2.0;
        let bs = scene(&bright, &Space::Model, 0.01, Rgb(0, 0, 0));
        let c = rasterize(&bs, &view(&bs), VisualStyle::Realistic, Rgb(0, 0, 0));
        let lum = |r: &Raster| r.rgba.chunks(4).map(|p| u64::from(p[0]) + u64::from(p[1]) + u64::from(p[2])).sum::<u64>();
        assert!(lum(&c) > lum(&a));
        // Night with only a lamp is darker than day.
        let mut night = courtyard(None);
        night.sun.minutes = 0;
        night
            .add(
                &Space::Model,
                Common::default(),
                EntityKind::Light(cadaddy_doc::lighting::Light { position: Vec3::new(0.0, -3.0, 2.0), intensity: 40.0, ..Default::default() }),
            )
            .unwrap();
        let ns = scene(&night, &Space::Model, 0.01, Rgb(0, 0, 0));
        assert_eq!(ns.lighting.as_ref().map(|l| l.lights.len()), Some(1));
        let n = rasterize(&ns, &view(&ns), VisualStyle::Realistic, Rgb(0, 0, 0));
        assert!(lum(&n) < lum(&a));
        if let Ok(out) = std::env::var("CDY_RENDER_DUMP") {
            let big = |s: &Scene| rasterize(s, &fit(s, p, 800, 600).unwrap(), VisualStyle::Realistic, Rgb(0, 0, 0));
            for (name, s) in [("plain", &plain), ("brick", &brick), ("night", &ns)] {
                std::fs::write(format!("{out}/{name}.png"), big(s).encode_png().unwrap()).unwrap();
            }
        }
    }

    #[test]
    fn fog_fades_distant_geometry_in_realistic_view() {
        use crate::light::FogParams;
        let p = Projection::new(Vec3::new(-1.0, -1.0, 1.0));
        let d = courtyard(None);
        let s = scene(&d, &Space::Model, 0.01, Rgb(0, 0, 0));
        let v = fit(&s, p, 80, 60).unwrap();
        let lum = |r: &Raster| r.rgba.chunks(4).map(|p| u64::from(p[0]) + u64::from(p[1]) + u64::from(p[2])).sum::<u64>();
        // Without fog.
        let no_fog = rasterize(&s, &v, VisualStyle::Realistic, Rgb(0, 0, 0));
        // With white fog.
        let mut sf = scene(&d, &Space::Model, 0.01, Rgb(0, 0, 0));
        if let Some(lit) = sf.lighting.as_mut() {
            lit.fog = Some(FogParams { color: [1.0, 1.0, 1.0], near: 0.0, far: 0.5 });
        }
        let with_fog = rasterize(&sf, &v, VisualStyle::Realistic, Rgb(0, 0, 0));
        // Fog pushes surface pixels towards white, so the overall luminance increases.
        assert!(lum(&with_fog) > lum(&no_fog), "fog brightens (towards white): {} vs {}", lum(&with_fog), lum(&no_fog));
    }

    #[test]
    fn solid_background_replaces_sky_in_realistic_view() {
        use crate::light::Background;
        let p = Projection::new(Vec3::new(-1.0, -1.0, 1.0));
        let d = courtyard(None);
        let s = scene(&d, &Space::Model, 0.01, Rgb(0, 0, 0));
        let v = fit(&s, p, 80, 60).unwrap();
        // With a red solid background.
        let mut sr = scene(&d, &Space::Model, 0.01, Rgb(0, 0, 0));
        if let Some(lit) = sr.lighting.as_mut() {
            lit.background = Background::Solid([1.0, 0.0, 0.0]);
        }
        let r = rasterize(&sr, &v, VisualStyle::Realistic, Rgb(0, 0, 0));
        // The top-left corner (sky area) should be red.
        let px = r.pixel(0, 0).unwrap();
        assert!(px[0] > 100 && px[1] < 50 && px[2] < 50, "sky area is red background: {px:?}");
    }

    #[test]
    fn gradient_background_varies_top_to_bottom() {
        use crate::light::Background;
        let p = Projection::new(Vec3::new(-1.0, -1.0, 1.0));
        let d = courtyard(None);
        let s = scene(&d, &Space::Model, 0.01, Rgb(0, 0, 0));
        let v = fit(&s, p, 80, 60).unwrap();
        let mut sg = scene(&d, &Space::Model, 0.01, Rgb(0, 0, 0));
        if let Some(lit) = sg.lighting.as_mut() {
            lit.background = Background::Gradient { bottom: [0.0, 0.0, 0.0], top: [1.0, 1.0, 1.0] };
        }
        let r = rasterize(&sg, &v, VisualStyle::Realistic, Rgb(0, 0, 0));
        let top = r.pixel(0, 0).unwrap();
        let bot = r.pixel(0, r.height - 1).unwrap();
        // Top should be brighter than bottom in the sky region.
        assert!(top[0] > bot[0] + 20, "gradient top brighter: {top:?} vs {bot:?}");
    }

    #[test]
    fn empty_and_hostile_scenes() {
        let v = ShadeView::new(None, Vec2::ZERO, 10.0, 0, 70_000);
        assert_eq!((v.width_px, v.height_px), (1, MAX_SIDE));
        let s = Scene {
            tris: vec![Tri::plain([Vec3::ZERO; 3], Rgb(1, 2, 3))],
            lines: vec![Line3 { a: Vec3::ZERO, b: Vec3::new(1e300, 0.0, 0.0), color: Rgb(1, 2, 3), edge: false }],
            ..Scene::default()
        };
        let r = rasterize(&s, &ShadeView::new(None, Vec2::ZERO, 10.0, 32, 32), VisualStyle::ShadedEdges, Rgb(9, 9, 9));
        assert_eq!(r.pixel(0, 0), Some([9, 9, 9, 255]));
    }

    fn luma(p: [u8; 4]) -> f64 {
        0.2126 * f64::from(p[0]) + 0.7152 * f64::from(p[1]) + 0.0722 * f64::from(p[2])
    }

    /// The courtyard seen from the south-west, Realistic, with its lighting changed by `f`; the
    /// image and its view.
    fn courtyard_look(f: impl Fn(&mut crate::light::Lighting)) -> (Raster, ShadeView) {
        let mut s = scene(&courtyard(None), &Space::Model, 0.01, Rgb(0, 0, 0));
        if let Some(l) = s.lighting.as_mut() {
            f(l);
        }
        let v = fit(&s, Projection::new(Vec3::new(-1.0, -1.0, 1.0)), 320, 240).unwrap();
        (rasterize(&s, &v, VisualStyle::Realistic, Rgb(0, 0, 0)), v)
    }

    #[test]
    fn ambient_occlusion_darkens_where_the_box_meets_the_floor() {
        let (open, v) = courtyard_look(|l| l.ao = 0.0);
        let (shut, _) = courtyard_look(|l| l.ao = 1.0);
        let (strong, _) = courtyard_look(|l| l.ao = 2.0);
        if let Ok(out) = std::env::var("CDY_RENDER_DUMP") {
            for (name, r) in [("ao-off", &open), ("ao-on", &shut), ("ao-strong", &strong)] {
                std::fs::write(format!("{out}/{name}.png"), r.encode_png().unwrap()).unwrap();
            }
        }
        let at = |r: &Raster, p: Vec3| {
            let (x, y, _) = v.screen(p);
            luma(r.pixel(x as u32, y as u32).unwrap())
        };
        // On the floor beside the box's west face, in the corner: darker with occlusion, and
        // darker still when stronger.
        let corner = Vec3::new(-1.05, 0.0, 0.0);
        assert!(at(&shut, corner) < at(&open, corner) - 2.0, "{} vs {}", at(&shut, corner), at(&open, corner));
        assert!(at(&strong, corner) < at(&shut, corner), "{} vs {}", at(&strong, corner), at(&shut, corner));
        // Up the box's face, near the floor: darker too.
        let face = Vec3::new(-1.0, 0.0, 0.05);
        assert!(at(&shut, face) < at(&open, face), "{} vs {}", at(&shut, face), at(&open, face));
        // Open ground far from the box, and the box's top: unchanged.
        for p in [Vec3::new(-4.0, 3.5, 0.0), Vec3::new(0.0, 0.0, 2.0)] {
            assert!((at(&shut, p) - at(&open, p)).abs() <= 1.0, "{p:?}: {} vs {}", at(&shut, p), at(&open, p));
        }
        // On the whole: a little darker, never brighter anywhere.
        let mean = |r: &Raster| r.rgba.chunks(4).map(|p| luma([p[0], p[1], p[2], 255])).sum::<f64>() / (r.rgba.len() / 4) as f64;
        assert!(mean(&shut) < mean(&open));
        assert!(shut.rgba.chunks(4).zip(open.rgba.chunks(4)).all(|(a, b)| a[0] <= b[0].saturating_add(1)));
    }

    #[test]
    fn a_flat_floor_has_no_occlusion() {
        let mut d = Drawing::new_metric();
        let v = |x: f64, y: f64| Vec3::new(x, y, 0.0);
        let floor = Mesh {
            vertices: vec![v(-5.0, -5.0), v(5.0, -5.0), v(5.0, 5.0), v(-5.0, 5.0)],
            faces: vec![MeshFace { v: vec![0, 1, 2, 3], hidden: 0 }],
            kind: MeshKind::Polyface,
        };
        d.add(&Space::Model, Common::default(), EntityKind::Mesh(floor)).unwrap();
        let s = scene(&d, &Space::Model, 0.01, Rgb(0, 0, 0));
        let view = fit(&s, Projection::new(Vec3::new(-1.0, -0.5, 0.7)), 200, 150).unwrap();
        let mut off = s.clone();
        if let Some(l) = off.lighting.as_mut() {
            l.ao = 0.0;
        }
        let a = rasterize(&s, &view, VisualStyle::Realistic, Rgb(0, 0, 0));
        let b = rasterize(&off, &view, VisualStyle::Realistic, Rgb(0, 0, 0));
        assert_eq!(a.rgba, b.rgba);
    }

    /// A 10 × 10 floor and two 3 × 2 plates over it: the west one 0.5 above, the east one 3.
    fn plates() -> Scene {
        let mut d = Drawing::new_metric();
        let quad = |x0: f64, x1: f64, y0: f64, y1: f64, z: f64| Mesh {
            vertices: vec![Vec3::new(x0, y0, z), Vec3::new(x1, y0, z), Vec3::new(x1, y1, z), Vec3::new(x0, y1, z)],
            faces: vec![MeshFace { v: vec![0, 1, 2, 3], hidden: 0 }],
            kind: MeshKind::Polyface,
        };
        for m in [quad(-5.0, 5.0, -5.0, 5.0, 0.0), quad(-4.0, -1.0, -1.0, 1.0, 0.5), quad(1.0, 4.0, -1.0, 1.0, 3.0)] {
            d.add(&Space::Model, Common::default(), EntityKind::Mesh(m)).unwrap();
        }
        scene(&d, &Space::Model, 0.01, Rgb(0, 0, 0))
    }

    /// How wide (drawing units) the penumbra across the shadow edge at y = 1 below x is, the sun
    /// overhead and `degrees` across.
    fn penumbra(map: &ShadowMap, x: f64, degrees: f32) -> f64 {
        let t = sun_tan_half(degrees);
        let step = 0.002;
        let soft = (0..1000).map(|i| Vec3::new(x, i as f64 * step, 0.001)).filter(|p| (0.05..0.95).contains(&map.lit_soft(*p, t))).count();
        soft as f64 * step
    }

    #[test]
    fn soft_shadows_widen_with_the_distance_from_what_casts_them() {
        let s = plates();
        let map = ShadowMap::new(&s, Vec3::Z).unwrap();
        // Under the plates: in shadow; beside them: lit (sharp or soft).
        for t in [0.0, sun_tan_half(3.0)] {
            assert!(map.lit_soft(Vec3::new(-2.5, 0.0, 0.001), t) < 0.05);
            assert!(map.lit_soft(Vec3::new(2.5, 0.0, 0.001), t) < 0.05);
            assert!(map.lit_soft(Vec3::new(0.0, 3.0, 0.001), t) > 0.95);
        }
        let (low_sharp, high_sharp) = (penumbra(&map, -2.5, 0.0), penumbra(&map, 2.5, 0.0));
        let (low, high) = (penumbra(&map, -2.5, 3.0), penumbra(&map, 2.5, 3.0));
        // Sharp shadows: about a texel either way, whatever the height.
        assert!(low_sharp < 0.02 && high_sharp < 0.02, "{low_sharp} {high_sharp}");
        // Soft: wider than sharp, and the plate 3 up casts a much wider penumbra than the one
        // 0.5 up (about 2 × height × tan(1.5°): 0.16 against 0.03).
        assert!(low > low_sharp && high > low * 2.5, "{low} {high}");
        assert!(high > 0.08 && high < 0.4, "{high}");
        // A bigger sun, a wider penumbra.
        assert!(penumbra(&map, 2.5, 6.0) > high);
    }

    #[test]
    fn soft_shadows_show_in_the_realistic_image() {
        // Seen from above, the sun a little off overhead: floor pixels between full shadow and
        // full sun, against both.
        let s = plates();
        let view = fit(&s, None, 300, 300).unwrap();
        let partial = |deg: f32| {
            let mut s = s.clone();
            if let Some(l) = s.lighting.as_mut() {
                l.sun = Some((Vec3::new(0.2, 0.1, 1.0).normalized(), [3.0; 3]));
                l.sun_size = deg;
                l.ao = 0.0;
            }
            let r = rasterize(&s, &view, VisualStyle::Realistic, Rgb(0, 0, 0));
            if let Ok(out) = std::env::var("CDY_RENDER_DUMP") {
                std::fs::write(format!("{out}/soft-{deg}.png"), r.encode_png().unwrap()).unwrap();
            }
            let at = |p: Vec3| {
                let (x, y, _) = view.screen(p);
                luma(r.pixel(x as u32, y as u32).unwrap())
            };
            // In the high plate's shadow where the plate doesn't hide it, and in the open.
            let (shadow, sun) = (at(Vec3::new(0.7, -0.3, 0.0)), at(Vec3::new(0.0, 3.5, 0.0)));
            assert!(sun > shadow + 40.0, "{sun} {shadow}");
            let (lo, hi) = (view.screen(Vec3::new(-4.8, 4.8, 0.0)), view.screen(Vec3::new(4.8, -4.8, 0.0)));
            let mut n = 0;
            for y in lo.1 as u32..hi.1 as u32 {
                for x in lo.0 as u32..hi.0 as u32 {
                    let l = luma(r.pixel(x, y).unwrap());
                    if l > shadow + (sun - shadow) * 0.1 && l < sun - (sun - shadow) * 0.1 {
                        n += 1;
                    }
                }
            }
            n
        };
        let (sharp, soft) = (partial(0.0), partial(4.0));
        assert!(soft > sharp * 3 && soft > 200, "{sharp} {soft}");
    }

    #[test]
    fn tone_curves_change_the_realistic_image() {
        use cadaddy_doc::lighting::ToneMap;
        let imgs: Vec<Raster> = ToneMap::ALL.iter().map(|t| courtyard_look(|l| l.tone = *t).0).collect();
        for (i, a) in imgs.iter().enumerate() {
            for b in imgs.iter().skip(i + 1) {
                assert_ne!(a.rgba, b.rgba);
            }
        }
        // The drawing's own setting reaches the lighting.
        let mut d = courtyard(None);
        d.sun.look.tone_map = ToneMap::Neutral;
        d.sun.look.ambient_occlusion = 0.5;
        d.sun.look.soft_shadows = 2.0;
        let l = crate::light::Lighting::of(&d, &Space::Model);
        assert_eq!((l.tone, l.ao, l.sun_size), (ToneMap::Neutral, 0.5, 2.0));
    }

    #[test]
    fn hostile_look_values_do_not_panic() {
        for (ao, radius, size) in [
            (f32::NAN, f64::NAN, f32::NAN),
            (f32::INFINITY, f64::INFINITY, f32::INFINITY),
            (-1.0, -1.0, -1.0),
            (1e30, 1e-300, 1e30),
            (2.0, 1e300, 90.0),
        ] {
            let (r, _) = courtyard_look(|l| {
                l.ao = ao;
                l.ao_radius = radius;
                l.sun_size = size;
            });
            assert_eq!(r.rgba.len(), 320 * 240 * 4);
        }
        assert_eq!(sun_tan_half(f32::NAN), 0.0);
        assert!(sun_tan_half(1e30).is_finite());
    }

    /// Markers standing on a ground plane at known distances behind the front one (0, 10, 40,
    /// 80 drawing units), seen square on; fog from 20 to 60.
    fn fog_bench() -> (Scene, ShadeView) {
        let grey = Rgb(120, 120, 120);
        let mut s = Scene::default();
        let quad = |s: &mut Scene, a: Vec3, b: Vec3, c: Vec3, d: Vec3| {
            s.tris.push(Tri::plain([a, b, c], grey));
            s.tris.push(Tri::plain([a, c, d], grey));
        };
        // The ground, under them all.
        quad(&mut s, Vec3::new(-10.0, 0.0, 0.0), Vec3::new(130.0, 0.0, 0.0), Vec3::new(130.0, 100.0, 0.0), Vec3::new(-10.0, 100.0, 0.0));
        for (k, y) in [0.0, 10.0, 40.0, 80.0].into_iter().enumerate() {
            let x = 30.0 * k as f64;
            quad(&mut s, Vec3::new(x, y, 1.0), Vec3::new(x + 20.0, y, 1.0), Vec3::new(x + 20.0, y, 21.0), Vec3::new(x, y, 21.0));
        }
        let mut d = Drawing::new_metric();
        d.sun.on = false;
        let mut lit = crate::light::Lighting::of(&d, &Space::Model);
        lit.bounds = scene_box(&s);
        s.lighting = Some(lit);
        // Looking along +Y (towards the viewer is −Y): distance = y.
        let v = ShadeView {
            dir: Vec3::new(0.0, -1.0, 0.0),
            right: Vec3::new(1.0, 0.0, 0.0),
            up: Vec3::new(0.0, 0.0, 1.0),
            center: Vec2::new(55.0, 11.0),
            height: 30.0,
            width_px: 240,
            height_px: 60,
        };
        (s, v)
    }

    /// The linear value a tone-mapped byte came from (exposure 1, neutral balance).
    fn untone(b: u8) -> f32 {
        let (mut lo, mut hi) = (0.0f32, 64.0f32);
        for _ in 0..60 {
            let mid = (lo + hi) / 2.0;
            if crate::light::tonemap([mid; 3], 1.0)[0] < b { lo = mid } else { hi = mid }
        }
        (lo + hi) / 2.0
    }

    /// One fog, measured from the front of the scene in drawing units, the same on the CPU
    /// rasterizer and the path tracer: the marker before `near` unchanged (±2/255), the one past
    /// `far` the fog colour (±2/255), the one at the middle half fogged in linear light (±5%).
    #[test]
    fn fog_is_by_distance_the_same_in_the_rasterizer_and_the_tracer() {
        use crate::light::{FogParams, tonemap};
        let fog = FogParams { color: [0.5, 0.45, 0.4], near: 20.0, far: 60.0 };
        let fog_byte = tonemap(fog.color, 1.0);
        let (s, v) = fog_bench();
        let images = |s: &Scene| -> [Raster; 2] {
            let raster = rasterize(s, &v, VisualStyle::Realistic, Rgb(0, 0, 0));
            let lit = s.lighting.clone().unwrap();
            let opts = crate::trace::TraceOptions { bounces: 1, denoise: false, crop: None, seed: 7 };
            let mut r = crate::trace::Render::new(s.clone(), v, lit, opts);
            for _ in 0..4 {
                r.pass();
            }
            [raster, r.image()]
        };
        let plain = images(&s);
        let mut sf = s.clone();
        sf.lighting.as_mut().unwrap().fog = Some(fog);
        let fogged = images(&sf);
        // Each marker's middle (x = 30k + 10, z = 11): the view is 30 units high in 60 px.
        let at = |k: usize| ((30.0 * k as f64 + 10.0 - 55.0) * 2.0 + 120.0, 30.0);
        for (path, (a, b)) in ["raster", "tracer"].iter().zip(plain.iter().zip(&fogged)) {
            let px = |r: &Raster, k: usize| {
                let (x, y) = at(k);
                r.pixel(x as u32, y as u32).unwrap()
            };
            // 10 from the front: before the fog.
            for c in 0..3 {
                assert!((i32::from(px(a, 1)[c]) - i32::from(px(b, 1)[c])).abs() <= 2, "{path}: before near {:?} {:?}", px(a, 1), px(b, 1));
                assert!((i32::from(px(b, 3)[c]) - i32::from(fog_byte[c])).abs() <= 2, "{path}: past far {:?} vs {fog_byte:?}", px(b, 3));
            }
            // 40 from the front: half way between 20 and 60, half fogged in linear light.
            let t = (untone(px(b, 2)[0]) - untone(px(a, 2)[0])) / (fog.color[0] - untone(px(a, 2)[0]));
            assert!((t - 0.5).abs() <= 0.05, "{path}: middle {t} ({:?} → {:?})", px(a, 2), px(b, 2));
            // The front marker is at distance 0: no fog.
            assert_eq!(px(a, 0), px(b, 0), "{path}: the front");
        }
    }

    /// An image background covers the whole view: a 2 × 2 picture's four colours fill the
    /// view's four quarters (±3/255), on the rasterizer and the path tracer.
    #[test]
    fn an_image_background_fills_the_view() {
        let quads = [[200u8, 30, 30], [30, 200, 30], [30, 30, 200], [220, 220, 40]];
        let rgba: Vec<u8> = quads.iter().flat_map(|c| [c[0], c[1], c[2], 255]).collect();
        let picture = std::sync::Arc::new(Raster::new(2, 2, rgba).unwrap());
        let (mut s, _) = fog_bench();
        let lit = s.lighting.as_mut().unwrap();
        lit.background = crate::light::Background::Image(picture);
        // Looking at empty space beside the markers.
        let v = ShadeView { center: Vec2::new(-500.0, 500.0), ..fog_bench().1 };
        let raster = rasterize(&s, &v, VisualStyle::Realistic, Rgb(0, 0, 0));
        let opts = crate::trace::TraceOptions { bounces: 1, denoise: false, crop: None, seed: 3 };
        let mut r = crate::trace::Render::new(s.clone(), v, s.lighting.clone().unwrap(), opts);
        r.pass();
        let traced = r.image();
        let (w, h) = (v.width_px, v.height_px);
        for (path, img) in [("raster", &raster), ("tracer", &traced)] {
            // Top left, top right, bottom left, bottom right: the picture's rows from the top.
            for (k, (x, y)) in [(w / 4, h / 4), (3 * w / 4, h / 4), (w / 4, 3 * h / 4), (3 * w / 4, 3 * h / 4)].into_iter().enumerate() {
                let p = img.pixel(x, y).unwrap();
                for c in 0..3 {
                    assert!((i32::from(p[c]) - i32::from(quads[k][c])).abs() <= 3, "{path}: quarter {k} at ({x}, {y}): {p:?} vs {:?}", quads[k]);
                }
            }
        }
    }
}
