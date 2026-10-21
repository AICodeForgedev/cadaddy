//! CADaddy rendering.
//!
//! [`build`] turns a drawing space into a [`DisplayList`]: world-space polylines, filled
//! triangles and points, each tagged with its top-level entity handle and resolved colour.
//! Linetypes, hatches, text, block references and dimensions are expanded here. The UI uploads
//! the list to the GPU; [`raster`] draws it on the CPU (PNG export, plot preview, tests).
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]
#![forbid(unsafe_code)]

mod cache;
pub use cache::{ChunkCache, build_cached, build_cached_into};
pub mod clip;
pub mod clouds;
mod dim;
pub mod encode;
mod fill;
mod hatch;
pub mod hlr;
pub mod images;
pub mod light;
pub mod lightmap;
mod linetype;
mod linetype_text;
pub mod live;
pub mod materials;
mod mline;
pub mod model3;
mod outline;
pub mod paper;
pub mod pen;
pub mod pia;
mod plotstyle;
mod point;
pub mod raster;
pub mod shade;
mod solid;
mod text;
mod tolerance;
pub mod trace;
pub mod units;
mod wide;

use cadaddy_color::{Color, Rgb};
use cadaddy_doc::{Drawing, Entity, EntityKind, Handle, Lineweight, Prim, Space};
use cadaddy_geom::{Bounds2, Mat3, Polyline, Vec2};
use pen::Pen;

pub use dim::{
    ArrowGeom, Arrowhead, DimGeometry, DimText, LineRole, arrowhead, dimension_geometry, dimension_geometry_with, format_linear_value, right_to_left,
};
pub use fill::triangulate_evenodd;
pub use hatch::{fill_loops as hatch_fill_loops, pattern_lines as hatch_pattern_lines, pattern_swatch};
pub use paper::{PAPER_SIZES, PaperSize, Sheet, paper_size, sheet};
pub use solid::solid_mesh;
pub use tolerance::{gdt_points, tolerance_in};

/// Handle carried by model-space geometry drawn inside a paper-space viewport (it belongs to no
/// single paper-space entity, so selection highlighting never matches it).
pub const VIEWPORT_CONTENT: Handle = Handle(0);

/// [`DisplayList::prim_layers`] of a primitive whose layer isn't in the layer table.
pub const NO_LAYER: u32 = u32::MAX;

/// What a display-list primitive draws.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Kind {
    /// `verts[start..start+len]` as a connected polyline.
    Polyline,
    /// `tris[start..start+len]`, three vertices per triangle.
    Tris,
    /// A wipeout: triangles laid out like [`Kind::Tris`], filled with the background colour
    /// (paper white when plotting, the canvas colour on screen) so they hide what was drawn
    /// before them.
    Mask,
    /// A single point marker at `verts[start]`.
    Point,
    /// An infinite line or ray: `verts[start]` = base, `verts[start+1]` = unit direction.
    Infinite { ray: bool },
    /// A raster image ([`DisplayList::images`] entry): triangles laid out like [`Kind::Tris`]
    /// covering the visible part of the image (its clip boundary, cut by any viewport), painted
    /// with the image's pixels.
    Image(u32),
}

impl Kind {
    /// The primitive is triangles in [`DisplayList::tris`] (otherwise points in `verts`).
    pub fn is_fill(self) -> bool {
        matches!(self, Kind::Tris | Kind::Mask | Kind::Image(_))
    }
}

/// The pixels a [`Kind::Image`] primitive paints and where they lie: pixel space (0..width
/// across, 0..height down from the top-left corner) maps to world space by `origin + x·ex/width
/// + y·ey/height`.
#[derive(Clone, Debug)]
pub struct ImageRef {
    /// The file the IMAGE names (the cache key).
    pub path: String,
    pub raster: std::sync::Arc<images::Raster>,
    /// World position of the image's top-left corner.
    pub origin: Vec2,
    /// World vectors along the image's full width (left to right) and full height (top down).
    pub ex: Vec2,
    pub ey: Vec2,
    /// Opacity of the image, 0..=255 (FADE and transparency).
    pub alpha: u8,
    /// A dark one-colour image on a clear background ([`images::is_dark_stencil`]): drawn white
    /// on a dark background, like colour 7.
    pub dark_stencil: bool,
}

impl ImageRef {
    /// The pixels to show on `background`: a dark stencil turns white on a dark background.
    pub fn raster_on(&self, background: Rgb) -> std::sync::Arc<images::Raster> {
        let dark_bg = u32::from(background.0) + u32::from(background.1) + u32::from(background.2) < 3 * 128;
        if self.dark_stencil && dark_bg { images::white_version(&self.raster) } else { std::sync::Arc::clone(&self.raster) }
    }
    /// Texture coordinates (0..1 across, 0..1 down) of world point `p`.
    pub fn uv(&self, p: Vec2) -> (f64, f64) {
        let det = self.ex.x * self.ey.y - self.ex.y * self.ey.x;
        if det.abs() < 1e-300 {
            return (0.0, 0.0);
        }
        let d = p - self.origin;
        ((d.x * self.ey.y - d.y * self.ey.x) / det, (self.ex.x * d.y - self.ex.y * d.x) / det)
    }
    /// The affine map from pixel space to world space: `[a, b, c, d, e, f]` with
    /// `x' = a·x + c·y + e`, `y' = b·x + d·y + f`.
    pub fn pixel_to_world(&self) -> [f64; 6] {
        let (w, h) = (f64::from(self.raster.width.max(1)), f64::from(self.raster.height.max(1)));
        [self.ex.x / w, self.ex.y / w, self.ey.x / h, self.ey.y / h, self.origin.x, self.origin.y]
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DPrim {
    pub handle: Handle,
    pub color: Rgb,
    /// Colour 7 (directly, by layer or by block): drawn white on a dark background and black on
    /// a light one, see [`DPrim::display_rgb`]. Other colours keep their RGB.
    pub aci7: bool,
    /// Lineweight in mm (0 = thinnest).
    pub lw: f32,
    /// Transparency in percent (0 = opaque, at most 90): the object's own, its layer's when
    /// ByLayer, the block reference's when ByBlock. Always 0 unless [`Options::transparency`].
    pub transparency: u8,
    pub kind: Kind,
    pub start: u32,
    pub len: u32,
}

#[derive(Clone, Debug, Default)]
pub struct DisplayList {
    pub prims: Vec<DPrim>,
    pub verts: Vec<Vec2>,
    pub tris: Vec<Vec2>,
    /// Plots ([`build_plot`]): for each primitive, the index in [`Drawing::layers`] of the layer
    /// it is drawn on (block contents on layer 0: the block reference's; [`NO_LAYER`] when the
    /// layer isn't in the table), for PDF layers. Empty for other lists.
    pub prim_layers: Vec<u32>,
    /// Plots: outline-font glyphs that a plotter can write as text instead of drawing the
    /// filled outline primitive they stand for ([`PlotText::prim`]), in drawing order.
    pub texts: Vec<PlotText>,
    /// Plots: texts drawn in a stroke font (as lines), for a hidden searchable copy (as a PDF
    /// plotter writes for SHX lettering), in drawing order.
    pub words: Vec<PlotWords>,
    pub bounds: Bounds2,
    /// The paper sheet when the list was built for a layout (paper space); the UI draws the
    /// white sheet, its shadow and the printable-area outline from it.
    pub sheet: Option<Sheet>,
    /// Block contents expanded while building (see [`MAX_BLOCK_EXPANSION`]).
    pub expanded: usize,
    /// Plot style end, join and fill style per primitive (parallel to `prims`; missing entries
    /// are "use object"), turned into geometry by [`pen::apply`].
    pub pens: Vec<Pen>,
    /// The images [`Kind::Image`] primitives paint.
    pub images: Vec<ImageRef>,
}

/// A glyph of plotted text: the font glyph and, in world coordinates, where it sits. It is
/// drawn by primitive [`PlotText::prim`] (a filled outline in the primitive's colour); output
/// with real text writes the glyph instead of that primitive.
#[derive(Clone, Debug, PartialEq)]
pub struct PlotText {
    /// The filled outline primitive (index in [`DisplayList::prims`]); [`PlotText::NONE`] when
    /// the glyph has none in the list (clipped by a viewport).
    pub prim: u32,
    /// Triangle vertices of the outline when it was drawn, and their bounding box (a clipped
    /// outline has others).
    pub verts: u32,
    pub bounds: Bounds2,
    /// The glyph; its origin and em axes are in world coordinates.
    pub glyph: cadaddy_fonts::TextGlyph,
}

/// A text drawn as strokes, for plots that add an invisible searchable copy of it: the words,
/// where they start (the lower left of what was drawn), their direction and height, in world
/// coordinates, and the first primitive drawn for them (whose layer they are on).
#[derive(Clone, Debug, PartialEq)]
pub struct PlotWords {
    pub text: String,
    pub origin: Vec2,
    /// Unit vector along the baseline.
    pub dir: Vec2,
    pub height: f64,
    pub prim: u32,
}

impl PlotText {
    pub const NONE: u32 = u32::MAX;
    /// [`PlotText::prim`] of a glyph cut by a viewport edge.
    pub const CLIPPED: u32 = u32::MAX - 1;
}

impl DPrim {
    /// The colour to draw with on `background`.
    pub fn display_rgb(&self, background: Rgb) -> Rgb {
        cadaddy_color::display_rgb(self.color, self.aci7, background)
    }
    /// The opacity to draw with: 255 when opaque, down to 26 at 90 % transparency.
    pub fn alpha(&self) -> u8 {
        alpha(self.transparency)
    }
}

/// The opacity (0..=255) of a transparency percentage, which is clamped to 0..=90 first.
pub fn alpha(transparency: u8) -> u8 {
    let t = u32::from(transparency.min(MAX_TRANSPARENCY));
    u8::try_from((255 * (100 - t) + 50) / 100).unwrap_or(u8::MAX)
}

/// The most transparent an object can be, in percent.
pub const MAX_TRANSPARENCY: u8 = 90;

/// A resolved colour: its RGB, whether it is colour 7, and its index colour (0 = a true
/// colour), which picks the style of a colour-dependent plot style table.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Ink {
    rgb: Rgb,
    aci7: bool,
    aci: u8,
}

/// Resolve `c` like [`Color::resolve`], noting whether it comes out as colour 7.
fn ink(c: Color, layer: Color, block: Color) -> Ink {
    // ByLayer / ByBlock left unresolved (layer or block colour itself logical) mean colour 7.
    let is7 = |c: Color| matches!(c, Color::ByLayer | Color::ByBlock | Color::Index(7));
    let aci7 = match c {
        Color::ByLayer => is7(layer),
        Color::ByBlock => is7(block),
        c => c == Color::Index(7),
    };
    let aci = match c {
        Color::ByLayer => layer,
        Color::ByBlock => block,
        c => c,
    };
    let aci = match aci {
        Color::Index(i) => i,
        Color::True(_) => 0,
        Color::ByLayer | Color::ByBlock => 7,
    };
    Ink { rgb: c.resolve(layer, block), aci7, aci }
}

impl DisplayList {
    pub fn segment_count(&self) -> usize {
        self.prims.iter().filter(|p| p.kind == Kind::Polyline).map(|p| p.len.saturating_sub(1) as usize).sum()
    }
    /// The polyline points of a primitive.
    pub fn points(&self, p: &DPrim) -> &[Vec2] {
        match p.kind {
            k if k.is_fill() => self.tris.get(p.start as usize..(p.start + p.len) as usize).unwrap_or(&[]),
            _ => self.verts.get(p.start as usize..(p.start + p.len) as usize).unwrap_or(&[]),
        }
    }
    /// The boundary of a filled primitive ([`Kind::Tris`], [`Kind::Mask`], [`Kind::Image`]) as
    /// closed polylines, for highlighting it; empty for other kinds.
    pub fn fill_outlines(&self, p: &DPrim) -> Vec<Vec<Vec2>> {
        if p.kind.is_fill() { outline::fill_outlines(self.points(p)) } else { Vec::new() }
    }
    /// The image a [`Kind::Image`] primitive paints.
    pub fn image(&self, p: &DPrim) -> Option<&ImageRef> {
        match p.kind {
            Kind::Image(i) => self.images.get(i as usize),
            _ => None,
        }
    }
}

/// Build options.
#[derive(Clone, Debug)]
pub struct Options {
    /// Chord tolerance in world units (about half a pixel at the current zoom).
    pub tolerance: f64,
    /// Two pixels in world units: shorter dashes are continuous and filled font text uses
    /// screen-size detail. Zero keeps full detail (plots and image/vector exports).
    pub min_dash: f64,
    /// Draw text (off for very coarse previews).
    pub text: bool,
    /// Highlighted (selected) handles are not special here; the canvas overlays them.
    pub fill: bool,
    pub lineweights: bool,
    /// Height of the visible area in world units of the space being built. Relative point
    /// sizes (`PDSIZE` <= 0) are a percentage of it; 0 = the height of the space's extents.
    pub view_height: f64,
    /// Plot style table to draw with. When `None`, [`build`] of a layout whose page setup has
    /// "Display plot styles" on uses the layout's table; [`build_plot`] uses none.
    pub plot_style_table: Option<std::sync::Arc<cadaddy_doc::PlotStyleTable>>,
    /// The area being looked at (world units of the space being built), for on-screen builds:
    /// hatch patterns are only generated inside it. `None` = everywhere (plots, exports, picks).
    pub view: Option<Bounds2>,
    /// Draw objects with their transparency ([`DPrim::transparency`]). Off by default: the screen
    /// follows TRANSPARENCYDISPLAY, plots the page setup's "Plot transparency".
    pub transparency: bool,
    /// A layout viewport whose model-space contents are left out (its border is still drawn):
    /// the canvas draws the active viewport's contents itself, see [`viewport_content`].
    pub skip_viewport: Option<Handle>,
    /// Set from another thread to stop the build early (the canvas cancels a background
    /// rebuild the view no longer needs). A cancelled build returns a partial list, which the
    /// caller must discard; see [`Options::cancelled`].
    pub cancel: Option<std::sync::Arc<std::sync::atomic::AtomicBool>>,
    /// A 3D view of model space (`None`: the plan view). The list is then built on the view
    /// plane: planar objects through the map of their plane, 3D objects point by point.
    pub projection: Option<cadaddy_geom::view3::Projection>,
    /// Shade field values with a grey box (FIELDDISPLAY, on screen only).
    pub field_shading: bool,
}

impl Options {
    /// Whether [`Options::cancel`] has been set.
    pub fn cancelled(&self) -> bool {
        self.cancel.as_ref().is_some_and(|c| c.load(std::sync::atomic::Ordering::Relaxed))
    }
}

impl Default for Options {
    fn default() -> Self {
        Options {
            tolerance: 0.001,
            min_dash: 0.0,
            text: true,
            fill: true,
            lineweights: false,
            view_height: 0.0,
            plot_style_table: None,
            view: None,
            skip_viewport: None,
            transparency: false,
            cancel: None,
            projection: None,
            field_shading: false,
        }
    }
}

/// The largest axis scale of a transform's linear part (its largest singular value): a length
/// `l` in the transform's own units measures at most `l * max_scale` after it, in any direction.
/// A uniform scale gives its factor, where [`Mat3::scale_factor`] gives the mean of the axes.
fn max_scale(m: &Mat3) -> f64 {
    let (a, b, c, d) = (m.a, m.b, m.c, m.d);
    let e = (a * a + b * b + c * c + d * d) / 2.0;
    let f = (((a * a + b * b - c * c - d * d) / 2.0).powi(2) + (a * c + b * d).powi(2)).sqrt();
    let s = (e + f).max(0.0).sqrt();
    if s.is_finite() { s } else { 0.0 }
}

/// Graphics state inherited through block references.
#[derive(Clone)]
struct Ctx<'a> {
    d: &'a Drawing,
    xf: Mat3,
    block_color: Color,
    block_layer: Option<String>,
    block_lw: Lineweight,
    block_ltype: String,
    depth: usize,
    top: Handle,
    /// Layers frozen in the viewport being drawn.
    frozen: &'a [String],
    /// Per-viewport layer colour overrides of the viewport being drawn.
    vp_colors: &'a [(String, Color)],
    /// Extra linetype scale for model space seen through a viewport: 1 / viewport scale when
    /// `PSLTSCALE` is on (dashes keep their paper-space length at any viewport scale), else 1.
    lt_factor: f64,
    /// Named plot style (table index) of the block reference being expanded, for `ByBlock`.
    block_pstyle: Option<usize>,
    /// Transparency (percent) of the block reference being expanded, for `ByBlock`.
    block_transparency: u8,
    /// The annotation scale (drawing units per paper unit) of the viewport being drawn when it
    /// has its own, apart from its zoom scale.
    view_anno: Option<f64>,
}

impl Ctx<'_> {
    /// A layer's colour, honouring the viewport's override.
    fn layer_color(&self, name: &str) -> Option<Color> {
        if let Some((_, c)) = self.vp_colors.iter().find(|(n, _)| n.eq_ignore_ascii_case(name)) {
            return Some(*c);
        }
        self.d.layer(name).map(|l| l.color)
    }
}

struct Builder<'a> {
    list: DisplayList,
    opts: &'a Options,
    /// Plotting: skip layers marked "do not plot".
    plotting: bool,
    /// The plot style table drawn with.
    styles: Option<&'a plotstyle::Styler>,
    /// Named plot style (table index) of the entity being drawn.
    named: Option<usize>,
    /// The block-expansion budget shared by the parts of a list built in parallel.
    shared_budget: Option<&'a std::sync::atomic::AtomicUsize>,
    /// Transparency (percent) of the entity being drawn.
    transparency: u8,
}

/// Upper bound on the block contents drawn per display list (each block reference, MINSERT copy
/// and entity drawn inside a block counts one). Nested, self-referencing or arrayed block
/// references in a hostile file would otherwise expand exponentially (`MAX_BLOCK_DEPTH` only
/// bounds the depth); past the limit the remaining block contents are not drawn.
pub const MAX_BLOCK_EXPANSION: usize = 2_000_000;

impl Builder<'_> {
    /// `color` and `lw` as the plot style table prints them.
    fn styled(&self, color: Ink, lw: f32) -> (Ink, f32) {
        match self.styles {
            Some(s) => s.apply(color, self.named, lw, self.opts.lineweights),
            None => (color, lw),
        }
    }
    /// Record the end, join and fill style `color` prints with for the primitive pushed next.
    fn note_pen(&mut self, color: Ink) {
        let Some(s) = self.styles else { return };
        let pen = s.pen(color, self.named);
        if !pen.is_object() {
            let i = self.list.prims.len();
            self.list.pens.resize(i, Pen::default());
            self.list.pens.push(pen);
        }
    }
    fn polyline(&mut self, ctx: &Ctx, color: Ink, lw: f32, pts: &[Vec2]) {
        if pts.len() < 2 {
            return;
        }
        self.note_pen(color);
        let (color, lw) = self.styled(color, lw);
        let start = self.list.verts.len() as u32;
        for p in pts {
            let q = ctx.xf.apply(*p);
            self.list.bounds.add(q);
            self.list.verts.push(q);
        }
        self.list.prims.push(DPrim {
            handle: ctx.top,
            color: color.rgb,
            aci7: color.aci7,
            lw,
            transparency: self.transparency,
            kind: Kind::Polyline,
            start,
            len: pts.len() as u32,
        });
    }
    fn tris(&mut self, ctx: &Ctx, color: Ink, tris: &[Vec2]) {
        self.tris_at(ctx, color, tris, ctx.xf);
    }
    fn tris_at(&mut self, ctx: &Ctx, color: Ink, tris: &[Vec2], xf: Mat3) {
        if tris.len() < 3 {
            return;
        }
        self.note_pen(color);
        let (color, _) = self.styled(color, 0.0);
        let start = self.list.tris.len() as u32;
        for p in tris {
            let q = xf.apply(*p);
            self.list.bounds.add(q);
            self.list.tris.push(q);
        }
        self.list.prims.push(DPrim {
            handle: ctx.top,
            color: color.rgb,
            aci7: color.aci7,
            lw: 0.0,
            transparency: self.transparency,
            kind: Kind::Tris,
            start,
            len: tris.len() as u32,
        });
    }
    /// Image triangles (in the entity's coordinates) painted with `img` (world coordinates
    /// apart from `ctx.xf`, which this maps as it maps the triangles).
    fn image_tris(&mut self, ctx: &Ctx, color: Ink, tris: &[Vec2], img: ImageRef) {
        if tris.len() < 3 {
            return;
        }
        let xf = ctx.xf;
        let img = ImageRef {
            origin: xf.apply(img.origin),
            ex: xf.apply(img.origin + img.ex) - xf.apply(img.origin),
            ey: xf.apply(img.origin + img.ey) - xf.apply(img.origin),
            ..img
        };
        let (color, _) = self.styled(color, 0.0);
        let start = self.list.tris.len() as u32;
        for p in tris {
            let q = xf.apply(*p);
            self.list.bounds.add(q);
            self.list.tris.push(q);
        }
        let idx = self.list.images.len() as u32;
        self.list.images.push(img);
        self.list.prims.push(DPrim {
            handle: ctx.top,
            color: color.rgb,
            aci7: color.aci7,
            lw: 0.0,
            transparency: self.transparency,
            kind: Kind::Image(idx),
            start,
            len: tris.len() as u32,
        });
    }
    /// Shaped text: strokes as polylines; TrueType glyphs filled (TEXTFILL) or outlined.
    /// Plots: note a text drawn as strokes from primitive `at` on (see [`DisplayList::words`]).
    fn words(&mut self, ctx: &Ctx, at: usize, text: &str, rotation: f64, height: f64) {
        if !self.plotting || text.trim().is_empty() || self.list.prims.len() <= at || self.list.words.len() >= 100_000 {
            return;
        }
        let b =
            self.list.prims.get(at..).map_or(Bounds2::EMPTY, |ps| {
                ps.iter().fold(Bounds2::EMPTY, |b, p| b.union(&Bounds2::from_points(self.list.points(p).iter().copied())))
            });
        let dir = ctx.xf.apply_vec(Vec2::from_angle(rotation));
        let len = dir.len();
        let (Ok(prim), false) = (u32::try_from(at), b.is_empty() || len.is_nan() || len <= 1e-12) else { return };
        let dir = dir * (1.0 / len);
        // Upright text starts at the lower left of what was drawn; turned text at its corner on
        // the baseline side.
        let origin = if dir.y.abs() < 1e-9 && dir.x > 0.0 { b.min } else { b.center() - dir * (b.width().max(b.height()) / 2.0) };
        self.list.words.push(PlotWords { text: text.chars().take(1000).collect(), origin, dir, height: height * len, prim });
    }

    fn shaped(&mut self, ctx: &Ctx, color: Ink, lw: f32, sh: &cadaddy_fonts::Shaped) {
        for s in &sh.strokes {
            self.polyline(ctx, color, lw, s);
        }
        if sh.glyphs.is_empty() {
            return;
        }
        let fill = self.opts.fill && ctx.d.header.i64("TEXTFILL", 1) != 0;
        // Plots note which filled outlines are font glyphs (see `DisplayList::texts`).
        let mut text = sh.text.iter().peekable();
        let pixel = if self.plotting { 0.0 } else { self.opts.min_dash * 0.5 };
        for (i, g) in sh.glyphs.iter().enumerate() {
            if fill {
                while text.next_if(|t| t.outline < i).is_some() {}
                let glyph = text.next_if(|t| t.outline == i);
                let at = self.list.prims.len();
                let len = if let Some(t) = glyph {
                    let xf = crate::text::placement(t, ctx.xf);
                    let mesh = crate::text::mesh(t, xf, pixel);
                    self.tris_at(ctx, color, &mesh.triangles, xf);
                    mesh.triangles.len()
                } else {
                    let tris = fill::triangulate_evenodd(g);
                    self.tris(ctx, color, &tris);
                    tris.len()
                };
                if let Some(t) = glyph.filter(|_| self.plotting)
                    && self.list.prims.len() > at
                    && let (Ok(prim), Ok(verts)) = (u32::try_from(at), u32::try_from(len))
                {
                    let xf = &ctx.xf;
                    let glyph = cadaddy_fonts::TextGlyph { origin: xf.apply(t.origin), x: xf.apply_vec(t.x), y: xf.apply_vec(t.y), ..t.clone() };
                    let bounds = self.list.prims.get(at).map(|p| Bounds2::from_points(self.list.points(p).iter().copied())).unwrap_or(Bounds2::EMPTY);
                    self.list.texts.push(PlotText { prim, verts, bounds, glyph });
                }
            } else {
                for c in g {
                    self.polyline(ctx, color, lw, c);
                }
            }
        }
    }
    fn point(&mut self, ctx: &Ctx, color: Ink, p: Vec2) {
        let (color, _) = self.styled(color, 0.0);
        let q = ctx.xf.apply(p);
        self.list.bounds.add(q);
        let start = self.list.verts.len() as u32;
        self.list.verts.push(q);
        self.list.prims.push(DPrim {
            handle: ctx.top,
            color: color.rgb,
            aci7: color.aci7,
            lw: 0.0,
            transparency: self.transparency,
            kind: Kind::Point,
            start,
            len: 1,
        });
    }
    fn infinite(&mut self, ctx: &Ctx, color: Ink, base: Vec2, dir: Vec2, ray: bool) {
        let (color, _) = self.styled(color, 0.0);
        let start = self.list.verts.len() as u32;
        self.list.verts.push(ctx.xf.apply(base));
        self.list.verts.push(ctx.xf.apply_vec(dir).normalized());
        self.list.prims.push(DPrim {
            handle: ctx.top,
            color: color.rgb,
            aci7: color.aci7,
            lw: 0.0,
            transparency: self.transparency,
            kind: Kind::Infinite { ray },
            start,
            len: 2,
        });
    }
    /// Take one unit of the block-expansion budget; false once it is spent.
    fn expand(&mut self) -> bool {
        self.list.expanded = self.list.expanded.saturating_add(1);
        match self.shared_budget {
            Some(spent) => spent.fetch_add(1, std::sync::atomic::Ordering::Relaxed) < MAX_BLOCK_EXPANSION,
            None => self.list.expanded <= MAX_BLOCK_EXPANSION,
        }
    }
}

/// Build the display list for a space. For a layout (paper space) this includes the model-space
/// geometry seen through each viewport, clipped to the viewport, and [`DisplayList::sheet`].
pub fn build(d: &Drawing, space: &Space, opts: &Options) -> DisplayList {
    build_space(d, space, opts, false)
}

/// Like [`build`] but for plotting: layers marked "do not plot" (such as `Defpoints`) are left out.
pub fn build_plot(d: &Drawing, space: &Space, opts: &Options) -> DisplayList {
    build_space(d, space, opts, true)
}

fn top_ctx<'a>(d: &'a Drawing, xf: Mat3, top: Handle, frozen: &'a [String], vp_colors: &'a [(String, Color)]) -> Ctx<'a> {
    Ctx {
        d,
        xf,
        block_color: Color::Index(7),
        block_layer: None,
        block_lw: Lineweight::Default,
        block_ltype: "Continuous".into(),
        depth: 0,
        top,
        frozen,
        vp_colors,
        lt_factor: 1.0,
        block_pstyle: None,
        block_transparency: 0,
        view_anno: None,
    }
}

fn build_space(d: &Drawing, space: &Space, opts: &Options, plotting: bool) -> DisplayList {
    let opts = &with_point_view(d, space, opts);
    let styler = style_table(d, space, opts, plotting).map(|t| plotstyle::Styler::new(d, t));
    let mut b = Builder { list: DisplayList::default(), opts, plotting, styles: styler.as_ref(), named: None, shared_budget: None, transparency: 0 };
    if let Space::Paper(name) = space {
        b.list.sheet = paper::sheet(d, name);
    }
    if let Some(store) = d.space(space) {
        let paper = matches!(space, Space::Paper(_));
        #[cfg(not(target_arch = "wasm32"))]
        if !paper && !plotting && b.styles.is_none() && store.len() >= 2 * PARALLEL_CHUNK {
            build_parallel(&mut b, d, store);
            return b.list;
        }
        let mut viewports = 0usize;
        for e in store.iter() {
            if paper && let EntityKind::Viewport(vp) = &e.kind {
                if viewports < MAX_VIEWPORTS {
                    viewports += 1;
                    viewport(&mut b, d, e, vp);
                    if plotting {
                        take_viewport_layers(&mut b.list);
                    }
                }
                continue;
            }
            top_entity(&mut b, d, e);
        }
    }
    // A layout is drawn in paper units, so its pens become geometry now.
    if let Some(sheet) = b.list.sheet {
        pen::apply(&mut b.list, sheet.unit_mm);
    }
    b.list
}

/// Top-level entities per task when a display list is built on all cores.
#[cfg(not(target_arch = "wasm32"))]
const PARALLEL_CHUNK: usize = 256;

/// Build model space for the screen on all cores: the entities are split into chunks built
/// side by side, then joined in drawing order, so the list is the same as a one-thread build.
/// The canvas rebuilds the whole list at every 2x zoom step (in the background since zooming
/// stopped blocking the UI) and after every edit; one core made both slow on real drawings. Plots, layouts and plot styles keep the one-thread
/// path (they run once, and their pens and per-primitive layers are built along the way).
#[cfg(not(target_arch = "wasm32"))]
fn build_parallel(b: &mut Builder, d: &Drawing, store: &cadaddy_doc::EntityStore) {
    use rayon::prelude::*;
    let ents: Vec<&std::sync::Arc<Entity>> = store.iter().collect();
    let budget = std::sync::atomic::AtomicUsize::new(0);
    let (opts, plotting) = (b.opts, b.plotting);
    let parts: Vec<DisplayList> = ents
        .par_chunks(PARALLEL_CHUNK)
        .map(|chunk| {
            let mut part =
                Builder { list: DisplayList::default(), opts, plotting, styles: None, named: None, shared_budget: Some(&budget), transparency: 0 };
            for e in chunk {
                top_entity(&mut part, d, e);
            }
            part.list
        })
        .collect();
    // One allocation per array for the joined list.
    b.list.prims.reserve(parts.iter().map(|p| p.prims.len()).sum());
    b.list.verts.reserve(parts.iter().map(|p| p.verts.len()).sum());
    b.list.tris.reserve(parts.iter().map(|p| p.tris.len()).sum());
    for part in parts {
        append(&mut b.list, part);
    }
}

/// Append `src` to `dst`: its primitives index into `dst`'s vertex and triangle arrays.
#[cfg(not(target_arch = "wasm32"))]
fn append(dst: &mut DisplayList, src: DisplayList) {
    let (verts, tris, prims, images) = (dst.verts.len() as u32, dst.tris.len() as u32, dst.prims.len() as u32, dst.images.len() as u32);
    // Pens are parallel to the primitives, with missing entries meaning "use object".
    if !src.pens.is_empty() {
        dst.pens.resize(dst.prims.len(), Pen::default());
    }
    dst.prims.extend(src.prims.into_iter().map(|mut p| {
        p.start += if p.kind.is_fill() { tris } else { verts };
        if let Kind::Image(i) = &mut p.kind {
            *i += images;
        }
        p
    }));
    dst.images.extend(src.images);
    dst.verts.extend(src.verts);
    dst.tris.extend(src.tris);
    dst.prim_layers.extend(src.prim_layers);
    dst.texts.extend(src.texts.into_iter().map(|mut t| {
        if t.prim < PlotText::CLIPPED {
            t.prim += prims;
        }
        t
    }));
    dst.pens.extend(src.pens);
    dst.bounds = dst.bounds.union(&src.bounds);
    dst.expanded = dst.expanded.saturating_add(src.expanded);
}

/// The plot style table a build draws with: the one in `opts`, else (on screen) the layout's
/// when its page setup has "Display plot styles" on.
fn style_table(d: &Drawing, space: &Space, opts: &Options, plotting: bool) -> Option<std::sync::Arc<cadaddy_doc::PlotStyleTable>> {
    opts.plot_style_table.clone().or_else(|| match space {
        Space::Paper(n) if !plotting => d
            .layout(n)
            .filter(|l| l.page.show_plot_styles)
            .and_then(|l| cadaddy_doc::plot_style_table(d, &l.page.plot_style_table))
            .map(std::sync::Arc::new),
        _ => None,
    })
}

/// `opts` with a view height for relative point sizes: the extents height when none is given
/// and points draw as figures sized relative to the view.
fn with_point_view(d: &Drawing, space: &Space, opts: &Options) -> Options {
    let mut o = opts.clone();
    let relative = d.header.f64("PDSIZE", 0.0) <= 0.0 && point::sized(d.header.i64("PDMODE", 0));
    if !(o.view_height.is_finite() && o.view_height > 0.0) && relative {
        let ext = d.extents(space);
        o.view_height = if ext.is_empty() { 0.0 } else { ext.height().max(ext.width() * 1e-3) };
    }
    o
}

/// Upper bound on viewports drawn per layout (hostile files).
const MAX_VIEWPORTS: usize = 256;

/// A paper-space viewport: its border (as an ordinary entity) and model space seen through it.
/// Viewport id 1 is the paper-space view itself and is not drawn. A clipped viewport shows
/// model space through its clip boundary's outline (the boundary entity draws itself, so the
/// rectangle is not drawn); a viewport turned off shows only its border.
fn viewport(b: &mut Builder, d: &Drawing, e: &Entity, vp: &cadaddy_doc::Viewport) {
    if !viewport_shown(d, e, vp) {
        return;
    }
    let poly = viewport_clip(d, e.handle, vp);
    // Border (on the viewport's layer; layer off hides only the border).
    if poly.is_none() {
        entity(b, &top_ctx(d, Mat3::IDENTITY, e.handle, &[], &[]), e);
    }
    if b.opts.skip_viewport == Some(e.handle) {
        return;
    }
    match &poly {
        Some(p) => contents(b, d, vp, p.bounds(), &Clip::Poly(p)),
        None => {
            if let Some(rect) = viewport_rect(vp) {
                contents(b, d, vp, rect, &Clip::Rect(rect));
            }
        }
    }
}

/// Whether a viewport draws anything: id 1 is the paper-space view itself, and a hidden viewport
/// or one on a frozen layer shows neither its border nor its contents.
fn viewport_shown(d: &Drawing, e: &Entity, vp: &cadaddy_doc::Viewport) -> bool {
    vp.id != 1 && e.common.visible && !d.is_hidden_object(e.handle) && !d.layer(&e.common.layer).is_some_and(|l| l.frozen)
}

/// A viewport's rectangle on paper; `None` for an unusable size or centre.
fn viewport_rect(vp: &cadaddy_doc::Viewport) -> Option<Bounds2> {
    let center = vp.center.xy();
    let ok = |v: f64| v.is_finite() && v > 0.0;
    (ok(vp.width) && ok(vp.height) && center.is_finite()).then(|| {
        let half = Vec2::new(vp.width / 2.0, vp.height / 2.0);
        Bounds2::new(center - half, center + half)
    })
}

/// Model space seen through the layout viewport `handle` of `space`, in paper units, clipped to
/// `area` (paper units) instead of to the viewport: what [`build`] draws inside that viewport,
/// over an area the caller chooses. The canvas builds the active viewport this way for an area
/// around it and draws it clipped to the viewport, so panning and zooming inside it moves the
/// geometry instead of rebuilding the layout (see [`Options::skip_viewport`]). Empty when the
/// viewport shows nothing.
pub fn viewport_content(d: &Drawing, space: &Space, handle: Handle, area: Bounds2, opts: &Options) -> DisplayList {
    let Some(e) = d.space(space).and_then(|s| s.get(handle)) else { return DisplayList::default() };
    let EntityKind::Viewport(vp) = &e.kind else { return DisplayList::default() };
    if !viewport_shown(d, e, vp) || area.is_empty() || !(area.min.is_finite() && area.max.is_finite()) {
        return DisplayList::default();
    }
    let opts = &with_point_view(d, space, opts);
    let styler = style_table(d, space, opts, false).map(|t| plotstyle::Styler::new(d, t));
    let mut b =
        Builder { list: DisplayList::default(), opts, plotting: false, styles: styler.as_ref(), named: None, shared_budget: None, transparency: 0 };
    contents(&mut b, d, vp, area, &Clip::Rect(area));
    b.list
}

/// Model space seen through `vp`, in paper units, appended to `b`'s list clipped to `clip`;
/// `area` (the clip's bounds on paper) picks the model objects to draw.
fn contents(b: &mut Builder, d: &Drawing, vp: &cadaddy_doc::Viewport, area: Bounds2, clip: &Clip) {
    if b.list.expanded >= MAX_BLOCK_EXPANSION || !vp.on {
        return;
    }
    // VP Linetype and VP Lineweight: the layers as this viewport overrides them (the stores are
    // shared, only the layer table is copied).
    let overridden;
    let d = if vp.layer_linetypes.is_empty() && vp.layer_lineweights.is_empty() {
        d
    } else {
        let mut o = d.clone();
        for l in &mut o.layers {
            if let Some((_, lt)) = vp.layer_linetypes.iter().find(|(n, _)| n.eq_ignore_ascii_case(&l.name)) {
                l.linetype = lt.clone();
            }
            if let Some((_, lw)) = vp.layer_lineweights.iter().find(|(n, _)| n.eq_ignore_ascii_case(&l.name)) {
                l.lineweight = *lw;
            }
        }
        overridden = o;
        &overridden
    };
    let center = vp.center.xy();
    let ok = |v: f64| v.is_finite() && v > 0.0;
    if !(ok(vp.width) && ok(vp.height) && ok(vp.view_height) && center.is_finite() && vp.view_center.is_finite()) {
        return;
    }
    let s = vp.height / vp.view_height;
    if !ok(s) {
        return;
    }
    let twist = if vp.twist.is_finite() { vp.twist } else { 0.0 };
    let xf = Mat3::translate(center).then_before(Mat3::rotate(twist)).then_before(Mat3::scale(s, s)).then_before(Mat3::translate(-vp.view_center));
    // The model window: `area` seen through the (possibly twisted) view.
    let Some(corners) = area.corners().into_iter().map(|c| vp.to_model(c)).collect::<Option<Vec<_>>>() else { return };
    let mwin = Bounds2::from_points(corners);
    // With slack: entity bounds are approximate (text, dimensions).
    let win = mwin.expand(mwin.width().max(mwin.height()) * 0.05);
    // PSLTSCALE on: model-space dashes are scaled so they measure the same on paper as in
    // paper space, whatever the viewport scale.
    let lt_factor = if d.header.i64("PSLTSCALE", 1) != 0 { 1.0 / s } else { 1.0 };
    // A viewport's own annotation scale, when it names one of the drawing's scales.
    let view_anno = vp.anno_scale.as_deref().and_then(|n| d.anno_scale(n)).filter(|sc| sc.is_valid()).map(|sc| sc.factor());
    let ctx = || Ctx { lt_factor, view_anno, ..top_ctx(d, xf, VIEWPORT_CONTENT, &vp.frozen_layers, &vp.layer_colors) };
    let seen = |me: &Entity| {
        if matches!(me.kind, EntityKind::Viewport(_)) {
            return false;
        }
        if matches!(me.kind, EntityKind::Ray(_) | EntityKind::XLine(_)) {
            return true;
        }
        let eb = cadaddy_doc::entity_bounds(d, me, 0);
        eb.is_empty() || eb.intersects(&win)
    };
    let mut sub = Builder {
        list: DisplayList::default(),
        opts: b.opts,
        plotting: b.plotting,
        styles: b.styles,
        named: None,
        shared_budget: None,
        transparency: 0,
    };
    #[cfg(not(target_arch = "wasm32"))]
    let parallel = !b.plotting && b.styles.is_none() && d.model.len() >= 2 * PARALLEL_CHUNK;
    #[cfg(target_arch = "wasm32")]
    let parallel = false;
    if parallel {
        #[cfg(not(target_arch = "wasm32"))]
        {
            // As `build_parallel`: chunks built side by side, joined in drawing order.
            use rayon::prelude::*;
            let ents: Vec<&std::sync::Arc<Entity>> = d.model.iter().collect();
            let budget = std::sync::atomic::AtomicUsize::new(0);
            let opts = b.opts;
            let parts: Vec<DisplayList> = ents
                .par_chunks(PARALLEL_CHUNK)
                .map(|chunk| {
                    let mut part = Builder {
                        list: DisplayList::default(),
                        opts,
                        plotting: false,
                        styles: None,
                        named: None,
                        shared_budget: Some(&budget),
                        transparency: 0,
                    };
                    let c = ctx();
                    for me in chunk.iter().filter(|me| seen(me)) {
                        entity(&mut part, &c, me);
                    }
                    part.list
                })
                .collect();
            for part in parts {
                append(&mut sub.list, part);
            }
        }
    } else {
        let c = ctx();
        for me in d.model.iter().filter(|me| seen(me)) {
            entity(&mut sub, &c, me);
        }
    }
    // Plotted text glyphs move to the layout's list; their outlines carry the glyph's index there
    // in their handle's high half until clipped (see `take_viewport_layers`).
    for t in std::mem::take(&mut sub.list.texts) {
        let k = b.list.texts.len() as u64 + 1;
        if let Some(p) = sub.list.prims.get_mut(t.prim as usize)
            && k < u64::from(u32::MAX)
        {
            p.handle = Handle(p.handle.0 | (k << 32));
            b.list.texts.push(PlotText { prim: PlotText::NONE, ..t });
        }
    }
    // The contents are in paper units now: draw their pens before clipping.
    pen::apply(&mut sub.list, b.list.sheet.map_or_else(|| paper::paper_unit_mm(d), |s| s.unit_mm));
    b.list.expanded = b.list.expanded.saturating_add(sub.list.expanded);
    append_clipped(&mut b.list, &sub.list, clip);
}

/// The clip region of a clipped viewport: the outline of its clip boundary entity, when that
/// is a closed shape in the drawing. `None` for a rectangular viewport (or a dangling,
/// unusable or self-referencing boundary, which falls back to the rectangle).
pub fn viewport_clip(d: &Drawing, handle: Handle, vp: &cadaddy_doc::Viewport) -> Option<clip::PolyClip> {
    let h = vp.clip.filter(|h| *h != handle)?;
    let be = d.entity(h)?;
    clip::PolyClip::new(&clip::boundary_polygon(&be.kind)?)
}

/// What viewport contents are clipped to.
enum Clip<'a> {
    /// The viewport rectangle (the fast path).
    Rect(Bounds2),
    /// A clip boundary's outline.
    Poly(&'a clip::PolyClip),
}

/// Append `src` to `dst`, clipped to `clip`.
fn append_clipped(dst: &mut DisplayList, src: &DisplayList, clip: &Clip) {
    for p in &src.prims {
        let pts = src.points(p);
        match (p.kind, clip) {
            (Kind::Polyline, Clip::Rect(rect)) => {
                for piece in clip::clip_polyline(pts, rect) {
                    push_raw(dst, p, Kind::Polyline, &piece);
                }
            }
            (Kind::Polyline, Clip::Poly(c)) => {
                for piece in c.clip_polyline(pts) {
                    push_raw(dst, p, Kind::Polyline, &piece);
                }
            }
            (Kind::Tris | Kind::Mask, Clip::Rect(rect)) => {
                let t = clip::clip_triangles(pts, rect);
                push_raw(dst, p, p.kind, &t);
            }
            (Kind::Tris | Kind::Mask, Clip::Poly(c)) => {
                let t = c.clip_triangles(pts);
                push_raw(dst, p, p.kind, &t);
            }
            (Kind::Image(i), _) => {
                let t = match clip {
                    Clip::Rect(rect) => clip::clip_triangles(pts, rect),
                    Clip::Poly(c) => c.clip_triangles(pts),
                };
                if let (false, Some(img)) = (t.is_empty(), src.images.get(i as usize)) {
                    let idx = dst.images.len() as u32;
                    dst.images.push(img.clone());
                    push_raw(dst, p, Kind::Image(idx), &t);
                }
            }
            (Kind::Point, _) => {
                let inside = |q: Vec2| match clip {
                    Clip::Rect(rect) => rect.contains(q),
                    Clip::Poly(c) => c.contains(q),
                };
                if let Some(q) = pts.first()
                    && inside(*q)
                {
                    push_raw(dst, p, Kind::Point, &[*q]);
                }
            }
            (Kind::Infinite { ray }, Clip::Rect(rect)) => {
                if let (Some(base), Some(dir)) = (pts.first(), pts.get(1))
                    && let Some((a, c)) = clip::clip_infinite(*base, *dir, ray, rect)
                {
                    push_raw(dst, p, Kind::Polyline, &[a, c]);
                }
            }
            (Kind::Infinite { ray }, Clip::Poly(c)) => {
                if let (Some(base), Some(dir)) = (pts.first(), pts.get(1)) {
                    for piece in c.clip_infinite(*base, *dir, ray) {
                        push_raw(dst, p, Kind::Polyline, &piece);
                    }
                }
            }
        }
    }
}

fn push_raw(dst: &mut DisplayList, p: &DPrim, kind: Kind, pts: &[Vec2]) {
    let min = match kind {
        Kind::Polyline => 2,
        k if k.is_fill() => 3,
        _ => 1,
    };
    if pts.len() < min {
        return;
    }
    let tris = kind.is_fill();
    let start = if tris { dst.tris.len() } else { dst.verts.len() } as u32;
    for q in pts {
        dst.bounds.add(*q);
    }
    if tris {
        dst.tris.extend_from_slice(pts);
    } else {
        dst.verts.extend_from_slice(pts);
    }
    dst.prims.push(DPrim {
        handle: p.handle,
        color: p.color,
        aci7: p.aci7,
        lw: p.lw,
        transparency: p.transparency,
        kind,
        start,
        len: pts.len() as u32,
    });
}

/// Build the display list for a set of loose entities (previews, rubber bands).
pub fn build_entities<'a, I: IntoIterator<Item = &'a Entity>>(d: &Drawing, ents: I, opts: &Options) -> DisplayList {
    let ents: Vec<&Entity> = ents.into_iter().collect();
    // Only previews holding points need the model extents for relative point sizes.
    let with_view;
    let opts = if ents.iter().any(|e| matches!(e.kind, EntityKind::Point(_))) {
        with_view = with_point_view(d, &Space::Model, opts);
        &with_view
    } else {
        opts
    };
    let mut b = Builder { list: DisplayList::default(), opts, plotting: false, styles: None, named: None, shared_budget: None, transparency: 0 };
    for e in ents {
        top_entity(&mut b, d, e);
    }
    b.list
}

/// Draw a top-level object: as it is in the plan view; in a 3D view ([`Options::projection`])
/// on the view plane. A planar object goes through the affine map of its plane (circles become
/// ellipses, text slants), so everything 2D drawing does still applies; a 3D object (a sloped
/// line, 3D polyline or 3D face) becomes the polyline of its projected points.
fn top_entity(b: &mut Builder, d: &Drawing, e: &Entity) {
    let Some(p) = b.opts.projection else {
        entity(b, &top_ctx(d, Mat3::IDENTITY, e.handle, &[], &[]), e);
        return;
    };
    // A mesh: its vertices onto the view plane; its edges are drawn from them.
    if let EntityKind::Mesh(m) = &e.kind {
        let vertices = m.vertices.iter().map(|q| p.project(*q).to3(0.0)).collect();
        let flat = Entity { kind: EntityKind::Mesh(cadaddy_doc::Mesh { vertices, ..m.clone() }), ..e.clone() };
        entity(b, &top_ctx(d, Mat3::IDENTITY, e.handle, &[], &[]), &flat);
        return;
    }
    // A solid: its display curves onto the view plane.
    if let EntityKind::Solid3d(so) = &e.kind {
        let ctx = top_ctx(d, Mat3::IDENTITY, e.handle, &[], &[]);
        // A live section shows what it leaves.
        let cut = live::cut_solid(so, &live::live_sections(d, &Space::Model)).map(|(c, _)| c);
        let shown = cut.as_ref().unwrap_or(so);
        for w in shown.wires(b.opts.tolerance) {
            let vertices = w.iter().map(|q| cadaddy_geom::PolyVertex { p: p.project(*q), bulge: 0.0, start_width: 0.0, end_width: 0.0 }).collect();
            let flat = Entity {
                kind: EntityKind::LwPolyline(cadaddy_doc::LwPolyline { vertices, closed: false, const_width: 0.0, elevation: 0.0, plinegen: true }),
                ..e.clone()
            };
            entity(b, &ctx, &flat);
        }
        return;
    }
    if let Some((pts, closed)) = e.kind.points3d() {
        let vertices = pts.iter().map(|q| cadaddy_geom::PolyVertex { p: p.project(*q), bulge: 0.0, start_width: 0.0, end_width: 0.0 }).collect();
        let flat = Entity {
            kind: EntityKind::LwPolyline(cadaddy_doc::LwPolyline { vertices, closed, const_width: 0.0, elevation: 0.0, plinegen: true }),
            ..e.clone()
        };
        entity(b, &top_ctx(d, Mat3::IDENTITY, e.handle, &[], &[]), &flat);
        return;
    }
    entity(b, &top_ctx(d, plane_xf(&p, e), e.handle, &[], &[]), e);
}

/// The map from `e`'s plan geometry to the view plane. Circles and arcs drawn in a tilted plane
/// (their extrusion not along Z) keep their WCS centre and turn into that plane; everything else
/// lies parallel to XY at its elevation.
fn plane_xf(p: &cadaddy_geom::view3::Projection, e: &Entity) -> Mat3 {
    let n = e.common.extrusion;
    let tilted = n.is_finite() && n.len() > 1e-12 && (n.x.abs() > 1e-9 * n.len() || n.y.abs() > 1e-9 * n.len());
    let center = match &e.kind {
        EntityKind::Circle(c) if tilted => Some(c.center),
        EntityKind::Arc(a) if tilted => Some(a.center),
        _ => None,
    };
    let Some(c) = center else { return p.plane_xf(e.kind.elevation()) };
    let ocs = cadaddy_geom::Mat4::ocs(n);
    let o = ocs.apply(cadaddy_geom::Vec3::ZERO);
    let (ax, ay) =
        (p.project(ocs.apply(cadaddy_geom::Vec3::new(1.0, 0.0, 0.0)) - o), p.project(ocs.apply(cadaddy_geom::Vec3::new(0.0, 1.0, 0.0)) - o));
    let pc = p.project(c);
    // (x, y) ↦ project(c + (x − cx)·ax + (y − cy)·ay)
    Mat3 { a: ax.x, b: ax.y, c: ay.x, d: ay.y, e: pc.x - c.x * ax.x - c.y * ay.x, f: pc.y - c.x * ax.y - c.y * ay.y }
}

/// The scale `xf` draws at, for chord tolerances and dash lengths. In a 3D view a plane can be
/// seen nearly edge-on, where the mean scale goes to zero but curves still span their full
/// width; the largest axis scale keeps their detail. 2D drawing keeps the mean scale.
fn detail_scale(b: &Builder, xf: &Mat3) -> f64 {
    let mean = xf.scale_factor();
    if b.opts.projection.is_none() {
        return mean;
    }
    let (a, bb, c, d) = (xf.a, xf.b, xf.c, xf.d);
    let s = a * a + bb * bb + c * c + d * d;
    let det = xf.det();
    let largest = ((s + (s * s - 4.0 * det * det).max(0.0).sqrt()) / 2.0).sqrt();
    mean.max(largest)
}

/// The layer `e` is drawn on, as an index in the layer table: layer 0 inside a block is the
/// block reference's layer.
fn entity_layer(ctx: &Ctx, e: &Entity) -> u32 {
    let name = if e.common.layer == "0" { ctx.block_layer.as_deref().unwrap_or("0") } else { e.common.layer.as_str() };
    ctx.d.layers.iter().position(|l| l.name.eq_ignore_ascii_case(name)).and_then(|i| u32::try_from(i).ok()).unwrap_or(NO_LAYER)
}

/// After a viewport is appended to a plot: the layers its contents carry in their handles
/// (see [`entity`]) go to [`DisplayList::prim_layers`], and the handles back to
/// [`VIEWPORT_CONTENT`]. The viewport's border was tagged when it was drawn.
///
/// Text glyphs seen through the viewport (marked in the handles' high half) keep their outline
/// primitive when it came through the clip whole; a glyph cut by the viewport edge stays
/// geometry.
fn take_viewport_layers(list: &mut DisplayList) {
    let start = list.prim_layers.len();
    for i in start..list.prims.len() {
        let Some(p) = list.prims.get(i).copied() else { break };
        list.prim_layers.push(u32::try_from(p.handle.0 & 0xFFFF_FFFF).unwrap_or(NO_LAYER));
        let bounds = Bounds2::from_points(list.points(&p).iter().copied());
        if let Some(t) = (p.handle.0 >> 32).checked_sub(1).and_then(|k| list.texts.get_mut(k as usize)) {
            let same = |a: Vec2, b: Vec2| a.dist(b) <= 1e-9 * (1.0 + a.len());
            let whole = t.prim == PlotText::NONE
                && p.kind == Kind::Tris
                && p.len == t.verts
                && same(bounds.min, t.bounds.min)
                && same(bounds.max, t.bounds.max);
            // A second piece (or a cut one) means the clip changed the glyph.
            t.prim = if whole { u32::try_from(i).unwrap_or(PlotText::NONE) } else { PlotText::CLIPPED };
        }
        if let Some(p) = list.prims.get_mut(i) {
            p.handle = VIEWPORT_CONTENT;
        }
    }
}

/// An object's colour, lineweight (mm), linetype, linetype scale, visibility and the transparency
/// of the layer it is drawn on (for ByLayer, see [`transparency`]).
type Resolved = (Ink, f32, Option<cadaddy_doc::Linetype>, f64, bool, u8);

fn resolve(ctx: &Ctx, e: &Entity, plotting: bool) -> Resolved {
    resolve_common(ctx, &e.common, plotting)
}

/// [`resolve`] for an object's properties `c`.
fn resolve_common(ctx: &Ctx, c: &cadaddy_doc::Common, plotting: bool) -> Resolved {
    let d = ctx.d;
    // Layer "0" inside a block takes the insert's layer.
    let layer_name = if c.layer == "0" { ctx.block_layer.as_deref().unwrap_or("0") } else { c.layer.as_str() };
    let layer = d.layer(layer_name);
    let visible =
        c.visible && layer.is_none_or(|l| l.visible() && (!plotting || l.plot)) && !ctx.frozen.iter().any(|f| f.eq_ignore_ascii_case(layer_name));
    let layer_color = ctx.layer_color(layer_name).unwrap_or(Color::Index(7));
    let rgb = ink(c.color, layer_color, ctx.block_color);
    let lw = match c.lineweight {
        Lineweight::ByLayer => layer.map(|l| l.lineweight).unwrap_or(Lineweight::Default),
        Lineweight::ByBlock => ctx.block_lw,
        x => x,
    };
    let lw_mm = match lw {
        Lineweight::Mm100(v) => f32::from(v) / 100.0,
        _ => default_lineweight(d),
    };
    let lt_name = match c.linetype.to_ascii_lowercase().as_str() {
        "bylayer" => layer.map(|l| l.linetype.clone()).unwrap_or_else(|| "Continuous".into()),
        "byblock" => ctx.block_ltype.clone(),
        _ => c.linetype.clone(),
    };
    let lt = d.linetype(&lt_name).filter(|l| !l.pattern.is_empty()).cloned();
    let scale = d.header.f64("LTSCALE", 1.0) * c.ltscale * ctx.lt_factor;
    (rgb, lw_mm, lt, scale, visible, layer.map_or(0, |l| l.transparency))
}

/// The transparency (percent, 0..=90) of an object whose own setting is `t`: the percentage, its
/// layer's `layer` when ByLayer (layer 0 inside a block: the block reference's layer, as
/// [`resolve_common`] finds it), the block reference's `block` when ByBlock (opaque outside
/// blocks).
fn transparency(t: cadaddy_doc::Transparency, layer: u8, block: u8) -> u8 {
    let t = match t {
        cadaddy_doc::Transparency::Percent(p) => p,
        cadaddy_doc::Transparency::ByBlock => block,
        cadaddy_doc::Transparency::ByLayer => layer,
    };
    t.min(MAX_TRANSPARENCY)
}

/// The width in mm that the "Default" lineweight stands for: `LWDEFAULT` (hundredths of a mm,
/// 0 to 211), 0.25 mm when it is unset or out of range.
fn default_lineweight(d: &Drawing) -> f32 {
    match d.header.i64("LWDEFAULT", 25) {
        v @ 0..=211 => v as f32 / 100.0,
        _ => 0.25,
    }
}

fn entity(b: &mut Builder, ctx: &Ctx, e: &Entity) {
    // Hidden by ISOLATEOBJECTS / HIDEOBJECTS (and so in viewports and plots too). A cancelled
    // build stops here (block contents come through here too).
    if ctx.d.is_hidden_object(e.handle) || b.opts.cancelled() {
        return;
    }
    // The entity's named plot style and transparency (set by `entity_parts`), for everything it
    // draws (block contents set their own).
    let (saved, saved_transparency) = (b.named, b.transparency);
    b.named = b.styles.and_then(|s| s.named_index(ctx, &e.common));
    let first = b.list.prims.len();
    entity_parts(b, ctx, e);
    // While a reference is edited in place, the rest of the drawing is faded (XFADECTL).
    if ctx.depth == 0
        && !b.plotting
        && let Some(r) = &ctx.d.refedit
        && !r.owns(e.handle)
    {
        let fade = u8::try_from(ctx.d.header.i64("XFADECTL", 50).clamp(0, i64::from(MAX_TRANSPARENCY))).unwrap_or(50);
        for p in b.list.prims.iter_mut().skip(first) {
            p.transparency = p.transparency.max(fade);
        }
    }
    b.named = saved;
    b.transparency = saved_transparency;
    if b.plotting && b.list.prim_layers.len() < b.list.prims.len() {
        // What it drew that its block contents didn't claim is on its own layer.
        let layer = entity_layer(ctx, e);
        let from = b.list.prim_layers.len();
        b.list.prim_layers.resize(b.list.prims.len(), layer);
        if ctx.top == VIEWPORT_CONTENT {
            // Model space seen through a viewport is clipped into the layout's list; the layer
            // travels in the handle until then (see [`take_viewport_layers`]).
            for p in b.list.prims.iter_mut().skip(from) {
                p.handle = Handle(u64::from(layer));
            }
        }
    }
}

fn entity_parts(b: &mut Builder, ctx: &Ctx, e: &Entity) {
    // A top-level annotative object draws at the view's annotation scale: a viewport's scale for
    // model space seen through it, else the drawing's (CANNOSCALE in model space, 1:1 on paper).
    if ctx.depth == 0 && cadaddy_doc::is_annotative(e) {
        let mode = if ctx.top == VIEWPORT_CONTENT {
            cadaddy_doc::anno_draw(e, ctx.view_anno.unwrap_or_else(|| 1.0 / ctx.xf.scale_factor().max(1e-12)), ctx.d.anno_all_visible())
        } else {
            ctx.d.anno_draw(e)
        };
        match mode {
            cadaddy_doc::AnnoDraw::Hidden => return,
            cadaddy_doc::AnnoDraw::Scaled(f) => {
                let mut scaled = cadaddy_doc::anno_scaled(e, f);
                scaled.common.anno_scales.clear();
                return entity_parts(b, ctx, &scaled);
            }
            cadaddy_doc::AnnoDraw::Plain => {}
        }
    }
    let (rgb, lw, lt, ltscale, visible, layer_transparency) = resolve(ctx, e, b.plotting);
    if !visible {
        return;
    }
    if b.opts.transparency {
        b.transparency = transparency(e.common.transparency, layer_transparency, ctx.block_transparency);
    }
    // A plot style's linetype replaces the object's.
    let lt = match b.styles.and_then(|s| s.linetype(rgb, b.named)) {
        Some(over) => over,
        None => lt,
    };
    let tol = b.opts.tolerance / detail_scale(b, &ctx.xf).max(1e-12);
    let lw = if b.opts.lineweights { lw } else { 0.0 };
    // Stroke a polyline with the entity's linetype.
    let stroke = |b: &mut Builder, pts: &[Vec2]| match &lt {
        Some(lt) => {
            let min = b.opts.min_dash / detail_scale(b, &ctx.xf).max(1e-12);
            for dash in linetype::apply(pts, lt, ltscale, min) {
                if dash.len() == 1 {
                    if let Some(p) = dash.first() {
                        b.point(ctx, rgb, *p);
                    }
                } else {
                    b.polyline(ctx, rgb, lw, &dash);
                }
            }
            // Text embedded in the linetype (shapes need SHX files and are not drawn).
            if b.opts.text {
                for sh in linetype_text::texts(ctx.d, pts, lt, ltscale, min) {
                    b.shaped(ctx, rgb, lw, &sh);
                }
            }
        }
        None => b.polyline(ctx, rgb, lw, pts),
    };
    match &e.kind {
        EntityKind::Text(t) => {
            if !b.opts.text {
                return;
            }
            let mut vals = field_values(ctx.d, e.handle);
            let text: String = match with_fields(&t.value, &mut vals, b.opts.field_shading) {
                Some(marked) => {
                    text_field_boxes(b, ctx, t, &marked);
                    marked.chars().filter(|c| *c != cadaddy_fonts::FIELD_ON && *c != cadaddy_fonts::FIELD_OFF).collect()
                }
                None => t.value.clone(),
            };
            let (sh, _) = place_text_entity(ctx.d, t, &text);
            if qtext(ctx.d) {
                qtext_box(b, ctx, rgb, lw, &[&sh], t.rotation);
                return;
            }
            let at = b.list.prims.len();
            b.shaped(ctx, rgb, lw, &sh);
            if !sh.strokes.is_empty() {
                b.words(ctx, at, &text, t.rotation, t.height);
            }
        }
        EntityKind::AttDef(a) => {
            let (sh, _) = place_text_entity(ctx.d, &a.text, &a.tag);
            if qtext(ctx.d) {
                qtext_box(b, ctx, rgb, lw, &[&sh], a.text.rotation);
            } else {
                b.shaped(ctx, rgb, lw, &sh);
            }
        }
        EntityKind::MText(t) => {
            if !b.opts.text {
                return;
            }
            let mut vals = field_values(ctx.d, e.handle);
            let substituted = with_fields(&t.contents, &mut vals, b.opts.field_shading).map(|contents| cadaddy_doc::MText { contents, ..t.clone() });
            let t = substituted.as_ref().unwrap_or(t);
            if let Some(bg) = &t.background {
                let layer = if e.common.layer == "0" { ctx.block_layer.as_deref().unwrap_or("0") } else { e.common.layer.as_str() };
                let layer_color = ctx.layer_color(layer).unwrap_or(Color::Index(7));
                mtext_mask(b, ctx, t, bg, rgb, layer_color);
            }
            let at = b.list.prims.len();
            let glyphs = b.list.texts.len();
            mtext(b, ctx, t, rgb, lw);
            // Drawn in a stroke font: no glyph of it became text.
            if b.list.texts.len() == glyphs {
                // As it reads: formatting codes (\P, {\f…;}, stacking) gone, then %% codes.
                let plain = cadaddy_fonts::plain_text(&cadaddy_fonts::plain_mtext(&t.contents)).split_whitespace().collect::<Vec<_>>().join(" ");
                b.words(ctx, at, &plain, t.rotation, t.height);
            }
        }
        EntityKind::Insert(ins) => insert(b, ctx, e, ins, rgb, lw),
        EntityKind::Dimension(dm) => {
            if let Some(blk) = dm.block.as_ref().and_then(|n| ctx.d.block(n))
                && ctx.depth < cadaddy_doc::MAX_BLOCK_DEPTH
            {
                let sub = Ctx { block_pstyle: b.named, block_transparency: b.transparency, ..sub_ctx(ctx, e, Mat3::IDENTITY) };
                for be in blk.entities.iter() {
                    if !b.expand() {
                        break;
                    }
                    entity(b, &sub, be);
                }
            } else {
                dimension(b, ctx, e, dm, lw, lt.as_ref(), ltscale);
            }
        }
        EntityKind::Hatch(h) => hatch::draw(b, ctx, e, h, rgb, lw, tol),
        EntityKind::MLine(m) => mline::draw(b, ctx, e, m, rgb, lw, tol),
        EntityKind::LwPolyline(p) if p.const_width > 0.0 || p.vertices.iter().any(|v| v.start_width > 0.0 || v.end_width > 0.0) => {
            if b.opts.fill && ctx.d.header.i64("FILLMODE", 1) != 0 {
                match &lt {
                    // Dashed: each dash a filled piece of the band; dots a line across it.
                    Some(lt) => {
                        let min = b.opts.min_dash / detail_scale(b, &ctx.xf).max(1e-12);
                        let (tris, ticks) = wide::dashed(p, tol, lt, ltscale, min);
                        b.tris(ctx, rgb, &tris);
                        for t in &ticks {
                            match t.as_slice() {
                                [q] => b.point(ctx, rgb, *q),
                                _ => b.polyline(ctx, rgb, lw, t),
                            }
                        }
                    }
                    None => b.tris(ctx, rgb, &fill::wide_polyline(p, tol)),
                }
            } else {
                let pl = Polyline { vertices: p.vertices.clone(), closed: p.closed };
                stroke(b, &pl.tessellate(tol));
            }
        }
        EntityKind::LwPolyline(p) => {
            let pl = Polyline { vertices: p.vertices.clone(), closed: p.closed };
            if p.plinegen || lt.is_none() {
                stroke(b, &pl.tessellate(tol));
            } else {
                for s in pl.segments() {
                    let mut pts = Vec::new();
                    s.tessellate(tol, &mut pts);
                    stroke(b, &pts);
                }
            }
        }
        EntityKind::Solid(s) | EntityKind::Trace(s) => {
            let c = &s.corners;
            let q = [c[0].xy(), c[1].xy(), c[3].xy(), c[2].xy()];
            if b.opts.fill && ctx.d.header.i64("FILLMODE", 1) != 0 {
                b.tris(ctx, rgb, &[q[0], q[1], q[2], q[0], q[2], q[3]]);
            } else {
                b.polyline(ctx, rgb, lw, &[q[0], q[1], q[2], q[3], q[0]]);
            }
        }
        EntityKind::Wipeout(w) => {
            // Hide what was drawn before it (draw order), then the frame per WIPEOUTFRAME:
            // 0 = none, 1 = shown and plotted, 2 = shown but not plotted.
            if b.opts.fill {
                let n = b.list.prims.len();
                b.tris(ctx, rgb, &fill::triangulate_evenodd(std::slice::from_ref(&w.boundary)));
                if let Some(p) = b.list.prims.get_mut(n) {
                    p.kind = Kind::Mask;
                }
            }
            let frame = match ctx.d.header.i64("WIPEOUTFRAME", 1) {
                0 => false,
                2 => !b.plotting,
                _ => true,
            };
            if frame {
                let mut pts = w.boundary.clone();
                if let Some(f) = pts.first().copied() {
                    pts.push(f);
                }
                b.polyline(ctx, rgb, lw, &pts);
            }
        }
        EntityKind::Image(i) => image(b, ctx, i, rgb, lw),
        EntityKind::Underlay(u) => underlay(b, ctx, u, rgb, lw),
        EntityKind::PointCloud(pc) => point_cloud(b, ctx, pc, rgb, lw),
        EntityKind::Table(t) => table(b, ctx, t, rgb, lw),
        EntityKind::Tolerance(t) => tolerance::draw(b, ctx, e, t, lw, tol),
        EntityKind::Leader(l) => {
            let pts: Vec<Vec2> = l.vertices.iter().map(|v| v.xy()).collect();
            stroke(b, &pts);
            if l.arrow
                && let (Some(a), Some(n)) = (pts.first(), pts.get(1))
            {
                let st = ctx.d.dim_style(&l.style);
                // DIMASZ × DIMSCALE of the leader's style (the drawing's DIMSCALE when it is 0).
                let size = st.map(|s| s.arrow_size).unwrap_or(0.18) * leader_scale(ctx.d, st);
                // DIMLDRBLK ("" = closed filled).
                let kind = dim::Arrowhead::parse(st.map(|s| s.leader_arrow_block.as_str()).unwrap_or(""));
                let g = dim::arrowhead(kind, *a, (*a - *n).normalized(), size);
                for l in &g.lines {
                    b.polyline(ctx, rgb, lw, l);
                }
                b.tris(ctx, rgb, &g.tris);
            }
        }
        EntityKind::MLeader(m) => {
            // Leader lines (straight or spline) in the multileader's own leader colour and
            // linetype when it overrides them; block content draws like a block reference.
            let (lrgb, llt) = if m.leader_color.is_some() || m.leader_linetype.is_some() {
                let mut c = e.common.clone();
                if let Some(col) = m.leader_color {
                    c.color = col;
                }
                if let Some(l) = &m.leader_linetype {
                    c.linetype.clone_from(l);
                }
                let (r, _, l, _, _, _) = resolve_common(ctx, &c, b.plotting);
                (r, l)
            } else {
                (rgb, lt.clone())
            };
            let kind = dim::Arrowhead::parse(m.arrow.as_deref().unwrap_or(""));
            for pts in m.leader_paths() {
                stroke_with(b, ctx, lrgb, lw, llt.as_ref(), ltscale, &pts);
                if let (Some(a), Some(n)) = (pts.first(), pts.get(1)) {
                    let g = dim::arrowhead(kind, *a, (*a - *n).normalized(), m.arrow_size);
                    for l in &g.lines {
                        b.polyline(ctx, lrgb, lw, l);
                    }
                    b.tris(ctx, lrgb, &g.tris);
                }
            }
            // The further landings' doglegs.
            for dg in m.root_doglegs() {
                stroke_with(b, ctx, lrgb, lw, llt.as_ref(), ltscale, &dg);
            }
            if let Some(t) = &m.text {
                let land = m.landing.xy();
                let dir = if t.insert.x >= land.x { 1.0 } else { -1.0 };
                stroke_with(b, ctx, lrgb, lw, llt.as_ref(), ltscale, &[land, land + Vec2::new(m.dogleg * dir, 0.0)]);
                mtext(b, ctx, t, rgb, lw);
                if m.text_frame {
                    // A frame around the text, as far out as the landing gap.
                    let st = ctx.d.mleader_styles.iter().find(|s| s.name.eq_ignore_ascii_case(&m.style)).cloned().unwrap_or_default();
                    let k = if st.annotative { 1.0 } else { ctx.d.header.f64("DIMSCALE", 1.0) };
                    let gap = (st.landing_gap * k).max(0.0);
                    let bx = layout_mtext_entity(ctx.d, t).bounds;
                    if !bx.is_empty() && bx.min.is_finite() && bx.max.is_finite() {
                        let bx = bx.expand(if gap.is_finite() { gap } else { 0.0 });
                        let [c0, c1, c2, c3] = bx.corners();
                        b.polyline(ctx, rgb, lw, &[c0, c1, c2, c3, c0]);
                    }
                }
            }
            if let Some(ins) = &m.block {
                if m.dogleg > 0.0 && !m.leaders.is_empty() {
                    let land = m.landing.xy();
                    let dir = if ins.insert.x >= land.x { 1.0 } else { -1.0 };
                    b.polyline(ctx, rgb, lw, &[land, land + Vec2::new(m.dogleg * dir, 0.0)]);
                }
                insert(b, ctx, e, ins, rgb, lw);
            }
        }
        EntityKind::Point(p) => {
            // Relative sizes follow the view: undo the block / viewport scale.
            let view = b.opts.view_height / ctx.xf.scale_factor().max(1e-12);
            let size = point::size(ctx.d.header.f64("PDSIZE", 0.0), view);
            let fig = point::figure(ctx.d.header.i64("PDMODE", 0), size, p.p.xy(), p.angle, tol);
            if fig.dot {
                b.point(ctx, rgb, p.p.xy());
            }
            for l in &fig.lines {
                b.polyline(ctx, rgb, lw, l);
            }
        }
        // Its display curves, seen from above, to the view's tolerance.
        EntityKind::Solid3d(so) => {
            for w in so.wires(tol) {
                let pts: Vec<Vec2> = w.iter().map(|p| p.xy()).collect();
                b.polyline(ctx, rgb, lw, &pts);
            }
        }
        kind => {
            for prim in kind.prims() {
                match prim {
                    Prim::Seg(s) => {
                        let mut pts = Vec::new();
                        s.tessellate(tol, &mut pts);
                        stroke(b, &pts);
                    }
                    Prim::Circle(c) => {
                        let mut pts = Vec::new();
                        c.tessellate(tol, &mut pts);
                        stroke(b, &pts);
                    }
                    Prim::Ellipse(el) => {
                        let mut pts = Vec::new();
                        el.tessellate(tol, &mut pts);
                        stroke(b, &pts);
                    }
                    Prim::Spline(s) => stroke(b, &s.tessellate(tol)),
                    Prim::Point(p) => b.point(ctx, rgb, p),
                    Prim::Infinite { base, dir, ray } => b.infinite(ctx, rgb, base, dir, ray),
                    Prim::Fill(pts) => {
                        let tris = fill::triangulate_evenodd(&[pts]);
                        b.tris(ctx, rgb, &tris);
                    }
                }
            }
        }
    }
}

/// The text style of an entity (Standard defaults when missing).
fn text_style(d: &Drawing, name: &str) -> cadaddy_doc::TextStyle {
    d.text_style(name).cloned().unwrap_or_default()
}

/// Shape a TEXT / ATTRIB / ATTDEF with its style's font and generation flags.
pub fn place_text_entity(d: &Drawing, t: &cadaddy_doc::Text, value: &str) -> (cadaddy_fonts::Shaped, Bounds2) {
    let ts = text_style(d, &t.style);
    let font = cadaddy_fonts::TextFont::resolve_style(&ts.font, &ts.big_font);
    let (h, v) = align(t.halign, t.valign);
    let p = cadaddy_fonts::TextParams {
        insert: t.insert.xy(),
        align_pt: t.align_pt.map(|p| p.xy()),
        height: t.height,
        rotation: t.rotation,
        width_factor: t.width_factor,
        oblique: t.oblique,
        h,
        v,
        backwards: ts.backwards,
        upside_down: ts.upside_down,
        vertical: ts.vertical,
    };
    cadaddy_fonts::place(&font, value, &p)
}

/// The grey behind field values on screen (FIELDDISPLAY).
const FIELD_INK: Ink = Ink { rgb: Rgb(128, 128, 128), aci7: false, aci: 8 };

/// The values fields of object `h` last evaluated to.
fn field_values(d: &Drawing, h: Handle) -> std::slice::Iter<'_, String> {
    d.field_values.get(&h).map_or([].iter(), |v| v.iter())
}

/// `s` with its fields' values (taken from `vals`) in place of their codes, each value between
/// [`cadaddy_fonts::FIELD_ON`] and [`cadaddy_fonts::FIELD_OFF`] when `marked`; `None` when `s`
/// holds no field.
fn with_fields(s: &str, vals: &mut std::slice::Iter<'_, String>, marked: bool) -> Option<String> {
    if !cadaddy_doc::field::has_fields(s) {
        return None;
    }
    let marks = marked.then_some((cadaddy_fonts::FIELD_ON, cadaddy_fonts::FIELD_OFF));
    Some(cadaddy_doc::field::substitute(s, &mut vals.by_ref().map(String::as_str), marks))
}

/// Grey boxes behind the field values of a single-line text (`marked` holds the values between
/// field marks), from the widths of the text before and in each value.
fn text_field_boxes(b: &mut Builder, ctx: &Ctx, t: &cadaddy_doc::Text, marked: &str) {
    if !marked.contains(cadaddy_fonts::FIELD_ON) {
        return;
    }
    let ts = text_style(ctx.d, &t.style);
    if ts.backwards || ts.upside_down || ts.vertical {
        return;
    }
    let font = cadaddy_fonts::TextFont::resolve(&ts.font);
    let h = t.height;
    let width = |s: &str| cadaddy_fonts::text_width(&font, s, h, t.width_factor);
    let plain: String = marked.chars().filter(|c| *c != cadaddy_fonts::FIELD_ON && *c != cadaddy_fonts::FIELD_OFF).collect();
    let total = width(&plain);
    let align_pt = t.align_pt.unwrap_or(t.insert).xy();
    use cadaddy_doc::{HAlign, VAlign};
    let (anchor, x0) = match t.halign {
        HAlign::Left => (t.insert.xy(), 0.0),
        HAlign::Center | HAlign::Middle => (align_pt, -total / 2.0),
        HAlign::Right => (align_pt, -total),
        HAlign::Aligned | HAlign::Fit => return,
    };
    let y0 = match (t.halign, t.valign) {
        (HAlign::Middle, _) | (_, VAlign::Middle) => -h / 2.0,
        (_, VAlign::Top) => -h,
        (_, VAlign::Bottom) => h * 0.3,
        _ => 0.0,
    };
    let (sin, cos) = t.rotation.sin_cos();
    let world = |q: Vec2| anchor + Vec2::new(q.x * cos - q.y * sin, q.x * sin + q.y * cos);
    let mut before = String::new();
    let mut start: Option<f64> = None;
    for c in marked.chars() {
        match c {
            cadaddy_fonts::FIELD_ON => start = Some(width(&before)),
            cadaddy_fonts::FIELD_OFF => {
                if let Some(a) = start.take() {
                    let w = width(&before) - a;
                    let q = [
                        Vec2::new(x0 + a, y0 - h * 0.3),
                        Vec2::new(x0 + a + w, y0 - h * 0.3),
                        Vec2::new(x0 + a + w, y0 + h * 1.2),
                        Vec2::new(x0 + a, y0 + h * 1.2),
                    ]
                    .map(world);
                    b.tris(ctx, FIELD_INK, &[q[0], q[1], q[2], q[0], q[2], q[3]]);
                }
            }
            c => before.push(c),
        }
    }
}

/// Lay out an MTEXT with its style's font.
pub fn layout_mtext_entity(d: &Drawing, t: &cadaddy_doc::MText) -> cadaddy_fonts::MTextLayout {
    let ts = text_style(d, &t.style);
    let params = cadaddy_fonts::MTextParams {
        insert: t.insert.xy(),
        height: t.height,
        width: t.width,
        attach: t.attach,
        rotation: t.rotation,
        line_spacing: t.line_spacing,
        line_spacing_exact: t.line_spacing_exact,
        font: cadaddy_fonts::TextFont::resolve_style(&ts.font, &ts.big_font),
        width_factor: ts.width_factor,
        oblique: ts.oblique,
        columns: t.columns.as_ref().map(|c| cadaddy_fonts::MTextColumnSpec {
            dynamic: c.kind == cadaddy_doc::MTextColumnType::Dynamic,
            count: usize::from(c.count.max(1)),
            gutter: c.gutter,
            height: c.height,
            heights: if c.kind == cadaddy_doc::MTextColumnType::Dynamic && !c.auto_height { c.heights.clone() } else { Vec::new() },
            reversed: c.flow_reversed,
        }),
    };
    cadaddy_fonts::layout_mtext_with(&t.contents, &params)
}

fn mtext_color(c: Option<cadaddy_fonts::MTextColor>, rgb: Ink) -> Ink {
    match c {
        Some(cadaddy_fonts::MTextColor::Aci(i)) => {
            u8::try_from(i).ok().filter(|i| *i > 0).map(|i| ink(Color::Index(i), Color::Index(7), Color::Index(7))).unwrap_or(rgb)
        }
        Some(cadaddy_fonts::MTextColor::Rgb(v)) => {
            Ink { rgb: Rgb(((v >> 16) & 0xFF) as u8, ((v >> 8) & 0xFF) as u8, (v & 0xFF) as u8), aci7: false, aci: 0 }
        }
        None => rgb,
    }
}

/// An MTEXT's background mask: a box round the laid-out text (in the text's own direction),
/// reaching `scale - 1` text heights past it, filled with the mask colour or, like a wipeout,
/// with the drawing's background. Drawn before the text, so it hides what lies beneath.
fn mtext_mask(b: &mut Builder, ctx: &Ctx, t: &cadaddy_doc::MText, bg: &cadaddy_doc::MTextBackground, rgb: Ink, layer_color: Color) {
    if !b.opts.fill {
        return;
    }
    let l = layout_mtext_entity(ctx.d, t);
    let o = t.insert.xy();
    let (sin, cos) = t.rotation.sin_cos();
    // Into the text's frame (rotation undone about the insertion point) and back.
    let local = |p: Vec2| {
        let d = p - o;
        Vec2::new(d.x * cos + d.y * sin, -d.x * sin + d.y * cos)
    };
    let world = |q: Vec2| o + Vec2::new(q.x * cos - q.y * sin, q.x * sin + q.y * cos);
    let mut bx = Bounds2::EMPTY;
    for p in l.strokes.iter().flatten() {
        bx.add(local(*p));
    }
    if bx.is_empty() || !bx.min.is_finite() || !bx.max.is_finite() {
        return;
    }
    let margin = (bg.scale - 1.0).clamp(0.0, 10.0) * t.height;
    let bx = bx.expand(margin);
    let q = [world(bx.min), world(Vec2::new(bx.max.x, bx.min.y)), world(bx.max), world(Vec2::new(bx.min.x, bx.max.y))];
    let tris = [q[0], q[1], q[2], q[0], q[2], q[3]];
    match bg.color {
        Some(c) => b.tris(ctx, ink(c, layer_color, ctx.block_color), &tris),
        None => {
            let n = b.list.prims.len();
            b.tris(ctx, rgb, &tris);
            if let Some(p) = b.list.prims.get_mut(n) {
                p.kind = Kind::Mask;
            }
        }
    }
}

/// QTEXT (QTEXTMODE 1): text is shown and plotted as the box round it.
fn qtext(d: &Drawing) -> bool {
    d.header.i64("QTEXTMODE", 0) != 0
}

/// The box round placed text, along its rotation: the outline QTEXT draws instead of the text.
pub fn qtext_outline(shapes: &[&cadaddy_fonts::Shaped], rotation: f64) -> Option<[Vec2; 4]> {
    let (c, s) = if rotation.is_finite() { (rotation.cos(), rotation.sin()) } else { (1.0, 0.0) };
    let local = |p: &Vec2| Vec2::new(p.x * c + p.y * s, -p.x * s + p.y * c);
    let mut bx = Bounds2::EMPTY;
    for sh in shapes {
        for p in sh.strokes.iter().flatten().chain(sh.glyphs.iter().flatten().flatten()) {
            if p.is_finite() {
                bx.add(local(p));
            }
        }
    }
    if bx.is_empty() {
        return None;
    }
    let world = |p: Vec2| Vec2::new(p.x * c - p.y * s, p.x * s + p.y * c);
    Some([world(bx.min), world(Vec2::new(bx.max.x, bx.min.y)), world(bx.max), world(Vec2::new(bx.min.x, bx.max.y))])
}

fn qtext_box(b: &mut Builder, ctx: &Ctx, rgb: Ink, lw: f32, shapes: &[&cadaddy_fonts::Shaped], rotation: f64) {
    if let Some([p0, p1, p2, p3]) = qtext_outline(shapes, rotation) {
        b.polyline(ctx, rgb, lw, &[p0, p1, p2, p3, p0]);
    }
}

fn mtext(b: &mut Builder, ctx: &Ctx, t: &cadaddy_doc::MText, rgb: Ink, lw: f32) {
    let l = layout_mtext_entity(ctx.d, t);
    if qtext(ctx.d) {
        let shapes: Vec<&cadaddy_fonts::Shaped> = l.pieces.iter().map(|p| &p.shaped).collect();
        qtext_box(b, ctx, rgb, lw, &shapes, t.rotation);
        return;
    }
    for q in &l.fields {
        b.tris(ctx, FIELD_INK, &[q[0], q[1], q[2], q[0], q[2], q[3]]);
    }
    for p in &l.pieces {
        b.shaped(ctx, mtext_color(p.color, rgb), lw, &p.shaped);
    }
}

/// The font settings of a dimension style's text style.
pub fn dim_text(d: &Drawing, st: &cadaddy_doc::DimStyle) -> DimText {
    let ts = text_style(d, &st.text_style);
    DimText {
        font: cadaddy_fonts::TextFont::resolve_style(&ts.font, &ts.big_font),
        fixed_height: ts.height,
        width_factor: ts.width_factor,
        oblique: ts.oblique,
    }
}

/// Geometry of a dimension in a drawing: its style, overrides and text font.
pub fn dimension_in(d: &Drawing, dm: &cadaddy_doc::Dimension) -> DimGeometry {
    let style = d.dim_style(&dm.style).cloned().unwrap_or_default();
    let st = style.with_overrides(&dm.overrides);
    dim::dimension_geometry_with(dm, &style, d.header.f64("DIMSCALE", 1.0), &dim_text(d, &st))
}

/// Draw a polyline with a linetype (None: continuous).
fn stroke_with(b: &mut Builder, ctx: &Ctx, rgb: Ink, lw: f32, lt: Option<&cadaddy_doc::Linetype>, ltscale: f64, pts: &[Vec2]) {
    match lt {
        Some(lt) => {
            let min = b.opts.min_dash / detail_scale(b, &ctx.xf).max(1e-12);
            for dash in linetype::apply(pts, lt, ltscale, min) {
                match dash.as_slice() {
                    [p] => b.point(ctx, rgb, *p),
                    _ => b.polyline(ctx, rgb, lw, &dash),
                }
            }
        }
        None => b.polyline(ctx, rgb, lw, pts),
    }
}

/// `lw` / `lt` are the dimension's own lineweight and linetype (ByBlock in DIMLWD, DIMLWE,
/// DIMLTYPE, DIMLTEX1 and DIMLTEX2).
fn dimension(b: &mut Builder, ctx: &Ctx, e: &Entity, dm: &cadaddy_doc::Dimension, lw: f32, lt: Option<&cadaddy_doc::Linetype>, ltscale: f64) {
    let d = ctx.d;
    let style = d.dim_style(&dm.style).cloned().unwrap_or_default().with_overrides(&dm.overrides);
    let g = dimension_in(d, dm);
    // DIMCLRD / DIMCLRE / DIMCLRT: ByBlock = the dimension's own colour.
    let layer_name = if e.common.layer == "0" { ctx.block_layer.as_deref().unwrap_or("0") } else { e.common.layer.as_str() };
    let layer_color = ctx.layer_color(layer_name).unwrap_or(Color::Index(7));
    let own = match e.common.color {
        Color::ByLayer => layer_color,
        Color::ByBlock => ctx.block_color,
        c => c,
    };
    let dim_rgb = ink(style.dim_line_color, layer_color, own);
    let ext_rgb = ink(style.ext_line_color, layer_color, own);
    let txt_rgb = ink(style.text_color, layer_color, own);
    let layer = d.layer(layer_name);
    let weight = |v: i16| -> f32 {
        if !b.opts.lineweights {
            return 0.0;
        }
        match Lineweight::from_dxf(v) {
            Lineweight::ByBlock => lw,
            Lineweight::Mm100(x) => f32::from(x) / 100.0,
            Lineweight::ByLayer => match layer.map(|l| l.lineweight) {
                Some(Lineweight::Mm100(x)) => f32::from(x) / 100.0,
                _ => default_lineweight(d),
            },
            Lineweight::Default => default_lineweight(d),
        }
    };
    let ltype = |name: &str| -> Option<cadaddy_doc::Linetype> {
        match name.trim().to_ascii_lowercase().as_str() {
            "" | "byblock" => lt.cloned(),
            "bylayer" => layer.and_then(|l| d.linetype(&l.linetype)).filter(|l| !l.pattern.is_empty()).cloned(),
            _ => d.linetype(name.trim()).filter(|l| !l.pattern.is_empty()).cloned(),
        }
    };
    let (dim_lw, ext_lw) = (weight(style.dim_lineweight), weight(style.ext_lineweight));
    let (dim_lt, ext1_lt, ext2_lt) = (ltype(&style.dim_linetype), ltype(&style.ext_linetype1), ltype(&style.ext_linetype2));
    for (i, (l, role)) in g.lines.iter().zip(g.line_roles.iter().chain(std::iter::repeat(&LineRole::Dim))).enumerate() {
        let (rgb, w, t) = match role {
            LineRole::Ext if g.ext2_lines.contains(&i) => (ext_rgb, ext_lw, ext2_lt.as_ref()),
            LineRole::Ext => (ext_rgb, ext_lw, ext1_lt.as_ref()),
            LineRole::Dim => (dim_rgb, dim_lw, dim_lt.as_ref()),
        };
        stroke_with(b, ctx, rgb, w, t, ltscale, l);
    }
    for t in &g.fills {
        b.tris(ctx, dim_rgb, t);
    }
    // DIMTFILL: a box behind the text (DIMGAP round it), in the drawing's background colour (1)
    // or DIMTFILLCLR (2). Drawn after the lines, so it hides what runs under the text.
    if b.opts.text && b.opts.fill && style.text_fill != 0 {
        let (sin, cos) = g.text_angle.sin_cos();
        let o = g.text_pos;
        let local = |p: Vec2| {
            let v = p - o;
            Vec2::new(v.x * cos + v.y * sin, -v.x * sin + v.y * cos)
        };
        let mut bx = Bounds2::EMPTY;
        for p in g.text.iter().flatten().chain(g.text_glyphs.iter().flatten().flatten()) {
            bx.add(local(*p));
        }
        if !bx.is_empty() && bx.min.is_finite() && bx.max.is_finite() {
            let gap = if style.text_height > 0.0 { style.text_gap.abs() / style.text_height * g.text_height } else { 0.0 };
            let bx = bx.expand(if gap.is_finite() { gap.min(1e9) } else { 0.0 });
            let world = |q: Vec2| o + Vec2::new(q.x * cos - q.y * sin, q.x * sin + q.y * cos);
            let q = [world(bx.min), world(Vec2::new(bx.max.x, bx.min.y)), world(bx.max), world(Vec2::new(bx.min.x, bx.max.y))];
            let tris = [q[0], q[1], q[2], q[0], q[2], q[3]];
            if style.text_fill == 2 {
                b.tris(ctx, ink(style.text_fill_color, layer_color, own), &tris);
            } else {
                let n = b.list.prims.len();
                b.tris(ctx, txt_rgb, &tris);
                if let Some(p) = b.list.prims.get_mut(n) {
                    p.kind = Kind::Mask;
                }
            }
        }
    }
    if b.opts.text {
        let sh = cadaddy_fonts::Shaped { strokes: g.text.clone(), glyphs: g.text_glyphs.clone(), width: 0.0, text: g.text_runs.clone() };
        b.shaped(ctx, txt_rgb, lw, &sh);
    }
}

mod table;
pub use table::{MAX_LAYOUT_CELLS, TableFill, TableGeom, TableLine, TableText, fit_rows, table_covered, table_geometry, table_style_of};

fn table(b: &mut Builder, ctx: &Ctx, t: &cadaddy_doc::Table, rgb: Ink, lw: f32) {
    table::draw(b, ctx, t, rgb, lw);
}

fn align(h: cadaddy_doc::HAlign, v: cadaddy_doc::VAlign) -> (cadaddy_fonts::Align, cadaddy_fonts::VAlign) {
    use cadaddy_fonts::{Align as A, VAlign as V};
    let ha = match h {
        cadaddy_doc::HAlign::Left => A::Left,
        cadaddy_doc::HAlign::Center => A::Center,
        cadaddy_doc::HAlign::Right => A::Right,
        cadaddy_doc::HAlign::Aligned => A::Aligned,
        cadaddy_doc::HAlign::Middle => A::Middle,
        cadaddy_doc::HAlign::Fit => A::Fit,
    };
    let va = match v {
        cadaddy_doc::VAlign::Baseline => V::Baseline,
        cadaddy_doc::VAlign::Bottom => V::Bottom,
        cadaddy_doc::VAlign::Middle => V::Middle,
        cadaddy_doc::VAlign::Top => V::Top,
    };
    (ha, va)
}

fn sub_ctx<'a>(ctx: &Ctx<'a>, e: &Entity, m: Mat3) -> Ctx<'a> {
    let layer = if e.common.layer == "0" { ctx.block_layer.clone() } else { Some(e.common.layer.clone()) };
    let color = match e.common.color {
        Color::ByBlock => ctx.block_color,
        Color::ByLayer => ctx.layer_color(layer.as_deref().unwrap_or("0")).unwrap_or(Color::Index(7)),
        c => c,
    };
    let lw = match e.common.lineweight {
        Lineweight::ByBlock => ctx.block_lw,
        Lineweight::ByLayer => ctx.d.layer(layer.as_deref().unwrap_or("0")).map(|l| l.lineweight).unwrap_or(Lineweight::Default),
        x => x,
    };
    let lt = match e.common.linetype.to_ascii_lowercase().as_str() {
        "byblock" => ctx.block_ltype.clone(),
        "bylayer" => ctx.d.layer(layer.as_deref().unwrap_or("0")).map(|l| l.linetype.clone()).unwrap_or_else(|| "Continuous".into()),
        _ => e.common.linetype.clone(),
    };
    Ctx {
        d: ctx.d,
        xf: ctx.xf.then_before(m),
        block_color: color,
        block_layer: layer,
        block_lw: lw,
        block_ltype: lt,
        depth: ctx.depth + 1,
        top: ctx.top,
        frozen: ctx.frozen,
        vp_colors: ctx.vp_colors,
        lt_factor: ctx.lt_factor,
        block_pstyle: ctx.block_pstyle,
        // The reference's own, set by the caller (`Builder::transparency` while it draws).
        block_transparency: ctx.block_transparency,
        view_anno: ctx.view_anno,
    }
}

/// Loads a PDF underlay's page: its file and 1-based page → the page as a drawing in sheet inches
/// (origin at the page's lower-left corner). The engine installs one that reads through the io
/// hooks and caches pages; without one an underlay shows as its frame.
pub type UnderlayLoader = fn(&str, u32) -> Option<std::sync::Arc<Drawing>>;

static UNDERLAY_LOADER: std::sync::OnceLock<UnderlayLoader> = std::sync::OnceLock::new();

pub fn set_underlay_loader(f: UnderlayLoader) {
    let _ = UNDERLAY_LOADER.set(f);
}

/// A PDF underlay: the page's objects (its layers turned off left out) placed by the underlay,
/// faded (as transparency) and in colour 7 when monochrome, inside its clip boundary or, without
/// one, its page frame. A page that can't be loaded shows as its frame.
/// A point cloud: about [`clouds::DRAW_BUDGET`] of its points (the octree's level of detail for
/// what the view shows), in the scan's colours, its intensity, by elevation or the object's
/// colour; its box when the file can't be read.
fn point_cloud(b: &mut Builder, ctx: &Ctx, pc: &cadaddy_doc::pointcloud::PointCloud, rgb: Ink, lw: f32) {
    let cloud = if ctx.depth < cadaddy_doc::MAX_BLOCK_DEPTH { clouds::cloud(&pc.path) } else { None };
    let Some(cloud) = cloud else {
        let c = pc.extents().corners();
        let mut frame: Vec<Vec2> = c.iter().map(|p| ctx.xf.apply(*p)).collect();
        frame.extend(frame.first().copied());
        b.polyline(ctx, rgb, lw, &frame);
        return;
    };
    // The view in the cloud's own coordinates (drawn at top level only; in a block, all of it).
    let view = b.opts.view.filter(|_| ctx.depth == 0).map(|v| {
        let own = cadaddy_geom::Bounds2::from_points(v.corners().iter().map(|p| pc.unplace(p.to3(0.0)).xy()));
        (own.min, own.max)
    });
    let single = [rgb.rgb.0, rgb.rgb.1, rgb.rgb.2];
    for i in cloud.lod(clouds::DRAW_BUDGET, view) {
        let Some(p) = cloud.points.get(i) else { continue };
        let own = cadaddy_geom::Vec3::new(f64::from(p[0]), f64::from(p[1]), f64::from(p[2]));
        if !pc.shows(own) {
            continue;
        }
        let [r, g, bl] = cloud.color(i, pc.color_mode, single);
        b.point(ctx, Ink { rgb: Rgb(r, g, bl), aci7: false, aci: 0 }, pc.place(own).xy());
    }
}

fn underlay(b: &mut Builder, ctx: &Ctx, u: &cadaddy_doc::Underlay, rgb: Ink, lw: f32) {
    let frame: Vec<Vec2> = u.corners().iter().map(|p| ctx.xf.apply(*p)).collect();
    let content = if ctx.depth < cadaddy_doc::MAX_BLOCK_DEPTH { UNDERLAY_LOADER.get().and_then(|f| f(&u.path, u.page.max(1))) } else { None };
    let Some(page) = content else {
        let mut closed = frame.clone();
        closed.extend(frame.first().copied());
        b.polyline(ctx, rgb, lw, &closed);
        return;
    };
    let m = ctx.xf.then_before(u.transform());
    let sub = Ctx { d: &page, xf: m, depth: ctx.depth + 1, frozen: &[], vp_colors: &[], ..top_ctx(&page, m, ctx.top, &[], &[]) };
    let mut part = Builder {
        list: DisplayList::default(),
        opts: b.opts,
        plotting: b.plotting,
        styles: None,
        named: None,
        shared_budget: b.shared_budget,
        transparency: 0,
    };
    part.list.expanded = b.list.expanded;
    for pe in page.model.iter() {
        if u.layers_off.iter().any(|l| l.eq_ignore_ascii_case(&pe.common.layer)) {
            continue;
        }
        if !part.expand() {
            break;
        }
        entity(&mut part, &sub, pe);
    }
    b.list.expanded = part.list.expanded;
    part.list.texts.clear();
    let fade = u.fade.min(80);
    for p in &mut part.list.prims {
        if u.monochrome {
            p.color = Rgb(255, 255, 255);
            p.aci7 = true;
        }
        p.transparency = p.transparency.max(fade);
    }
    let boundary: Vec<Vec2> = if u.clipping && u.clip.len() >= 2 {
        let t = u.transform();
        let pts: Vec<Vec2> = match u.clip.as_slice() {
            [a, c] => Bounds2::new(*a, *c).corners().to_vec(),
            pts => pts.to_vec(),
        };
        pts.into_iter().map(|p| ctx.xf.apply(t.apply(p))).collect()
    } else {
        frame
    };
    match clip::PolyClip::new(&boundary) {
        Some(poly) => append_clipped(&mut b.list, &part.list, &Clip::Poly(&poly)),
        None => append_clipped(&mut b.list, &part.list, &Clip::Rect(Bounds2::from_points(boundary))),
    }
}

/// A raster image: its pixels inside its clip boundary, and its frame per IMAGEFRAME (0 none,
/// 1 shown and plotted, 2 shown only). An image whose file can't be read is drawn as its frame
/// with the diagonals, as AutoCAD marks an unloaded image.
fn image(b: &mut Builder, ctx: &Ctx, i: &cadaddy_doc::Image, rgb: Ink, lw: f32) {
    let o = i.insert.xy();
    let (u, v) = (i.u.xy(), i.v.xy());
    let (w, h) = (i.size.x, i.size.y);
    if !(w.is_finite() && h.is_finite() && w > 0.0 && h > 0.0) {
        return;
    }
    // pixel coordinates (image corner at -0.5, -0.5, y down) to the entity's coordinates
    let at = |px: f64, py: f64| o + u * (px + 0.5) + v * (h - (py + 0.5));
    let boundary: Vec<Vec2> = match i.clip.as_slice() {
        [a, c] if i.clipping => {
            let (x0, x1, y0, y1) = (a.x.min(c.x), a.x.max(c.x), a.y.min(c.y), a.y.max(c.y));
            vec![at(x0, y1), at(x1, y1), at(x1, y0), at(x0, y0)]
        }
        pts if i.clipping && pts.len() >= 3 => pts.iter().map(|q| at(q.x, q.y)).collect(),
        _ => vec![o, o + u * w, o + u * w + v * h, o + v * h],
    };
    let raster = if i.display & 1 != 0 { images::get(&i.path) } else { None };
    let missing = i.display & 1 != 0 && raster.is_none();
    match raster {
        Some(raster) => {
            let raster = images::adjusted(&raster, i.brightness, i.contrast);
            let tris = triangulate_evenodd(std::slice::from_ref(&boundary));
            // TRANSPARENCY off (display bit 8 clear, AutoCAD's default): the image is opaque, its
            // transparent pixels showing the background colour, not what lies under it; on, they
            // show what is behind (UI review 0.7.7c: the flag changed nothing).
            if i.display & 8 == 0 && b.opts.fill {
                let n = b.list.prims.len();
                b.tris(ctx, rgb, &tris);
                if let Some(p) = b.list.prims.get_mut(n) {
                    p.kind = Kind::Mask;
                }
            }
            let alpha = (255.0 * (1.0 - f64::from(i.fade.min(100)) / 100.0)).round() as u8;
            let dark_stencil = images::is_dark_stencil(&raster);
            let img = ImageRef { path: i.path.clone(), raster, origin: o + v * h, ex: u * w, ey: v * -h, alpha, dark_stencil };
            b.image_tris(ctx, rgb, &tris, img);
        }
        None if i.display & 1 != 0 => {
            // not loaded: the frame and its diagonals, always (nothing else shows it is there)
            let c = [o, o + u * w, o + u * w + v * h, o + v * h];
            b.polyline(ctx, rgb, lw, &[c[0], c[2]]);
            b.polyline(ctx, rgb, lw, &[c[1], c[3]]);
        }
        None => {}
    }
    let frame = match ctx.d.header.i64("IMAGEFRAME", 1) {
        0 => false,
        2 => !b.plotting,
        _ => true,
    };
    if frame || missing {
        let mut closed = boundary;
        if let Some(f) = closed.first().copied() {
            closed.push(f);
        }
        b.polyline(ctx, rgb, lw, &closed);
    }
}

/// A block reference's contents: every copy (MINSERT rows × columns) of its block's objects.
fn insert_contents(b: &mut Builder, ctx: &Ctx, e: &Entity, ins: &cadaddy_doc::Insert, blk: &cadaddy_doc::Block) {
    // XCLIP Clipdepth: objects beyond the clipping planes are left out.
    let depth = ins.clip.as_ref().filter(|c| c.active() && (c.front.is_some() || c.back.is_some()));
    let cols = ins.cols.clamp(1, 10_000);
    let rows = ins.rows.clamp(1, 10_000);
    'copies: for r in 0..rows {
        for c in 0..cols {
            if !b.expand() {
                break 'copies;
            }
            let off = Vec2::new(ins.col_spacing * f64::from(c), ins.row_spacing * f64::from(r)).rotate(ins.rotation);
            let m = Mat3::translate(off).then_before(ins.transform(blk.base.xy()));
            let mut sub = Ctx { block_pstyle: b.named, block_transparency: b.transparency, ..sub_ctx(ctx, e, m) };
            // In a 3D view a block reference inside a block (an array's items on their levels)
            // stands at its own elevation; a top-level one is already placed at its elevation.
            if ctx.depth > 0
                && let Some(p) = b.opts.projection
                && ins.insert.z.is_finite()
                && ins.insert.z != 0.0
            {
                sub.xf = Mat3::translate(p.project(cadaddy_geom::Vec3::new(0.0, 0.0, ins.insert.z))).then_before(sub.xf);
            }
            for be in blk.entities.iter() {
                // Constant/visible attribute definitions inside blocks are not drawn; attribs are.
                if matches!(be.kind, EntityKind::AttDef(_)) || depth.is_some_and(|c| c.beyond_depth(&be.kind)) {
                    continue;
                }
                if !b.expand() {
                    break 'copies;
                }
                entity(b, &sub, be);
            }
        }
    }
}

fn insert(b: &mut Builder, ctx: &Ctx, e: &Entity, ins: &cadaddy_doc::Insert, rgb: Ink, lw: f32) {
    if ctx.depth >= cadaddy_doc::MAX_BLOCK_DEPTH {
        return;
    }
    if let Some(blk) = ctx.d.block(&ins.block) {
        // XCLIP: the contents are drawn on their own, then only what is inside the boundary
        // (in block coordinates, placed like the block) is kept.
        let clip = ins.clip.as_ref().filter(|c| c.active()).and_then(|c| {
            let m = ctx.xf.then_before(ins.transform(blk.base.xy()));
            let poly = clip::PolyClip::new(&c.boundary.iter().map(|p| m.apply(*p)).collect::<Vec<_>>())?;
            Some(if c.inverted { poly.inverted() } else { poly })
        });
        match clip {
            Some(poly) => {
                let mut part = Builder {
                    list: DisplayList::default(),
                    opts: b.opts,
                    plotting: b.plotting,
                    styles: b.styles,
                    named: b.named,
                    shared_budget: b.shared_budget,
                    transparency: b.transparency,
                };
                // The same expansion budget as the rest of the list.
                part.list.expanded = b.list.expanded;
                insert_contents(&mut part, ctx, e, ins, blk);
                b.list.expanded = part.list.expanded;
                // Plot text entries point at primitives by index; the clipped copies are new ones.
                part.list.texts.clear();
                append_clipped(&mut b.list, &part.list, &Clip::Poly(&poly));
                // XCLIPFRAME: 0 = no frame, 1 = shown and plotted, 2 = shown but not plotted.
                let frame = match ctx.d.header.i64("XCLIPFRAME", 2) {
                    0 => false,
                    2 => !b.plotting,
                    _ => true,
                };
                if let (true, Some(c)) = (frame, ins.clip.as_ref()) {
                    let m = ins.transform(blk.base.xy());
                    let mut pts: Vec<Vec2> = c.boundary.iter().map(|p| m.apply(*p)).collect();
                    if let Some(f) = pts.first().copied() {
                        pts.push(f);
                    }
                    b.polyline(ctx, rgb, lw, &pts);
                }
            }
            None => insert_contents(b, ctx, e, ins, blk),
        }
    }
    if b.opts.text {
        // ATTMODE (ATTDISP): 0 shows no attributes, 1 the visible ones, 2 all of them.
        let attmode = ctx.d.header.i64("ATTMODE", 1);
        // The reference's transparency; each attribute draws with its own.
        let reference = b.transparency;
        let mut vals = field_values(ctx.d, e.handle);
        for a in &ins.attribs {
            // Field values are counted over every attribute, shown or not.
            let with = with_fields(&a.text.value, &mut vals, false).map(|value| {
                let mut a = a.clone();
                a.text.value = value;
                a
            });
            let a = with.as_ref().unwrap_or(a);
            if attmode == 0 || (a.invisible && attmode != 2) {
                continue;
            }
            // A multiline attribute is drawn as its multiline text.
            let multiline = a.multiline_text();
            let sh = if multiline.is_none() { Some(place_text_entity(ctx.d, &a.text, &a.text.value).0) } else { None };
            // Properties of the attribute's definition (as AutoCAD copies them to the attribute
            // on insertion), except those set on the attribute itself: ByBlock = the reference's,
            // layer 0 = the reference's layer.
            let def = ctx.d.block(&ins.block).and_then(|blk| blk.entities.iter().find(|be| attdef_tag(be, &a.tag)));
            let (ink, lw, shown, t) = match (def, a.props.is_empty()) {
                (None, true) => (rgb, resolve(ctx, e, b.plotting).1, true, reference),
                (def, _) => {
                    let c = a.props.over(&def.map(|d| d.common.clone()).unwrap_or_else(by_block));
                    let (ink, lw, _, _, shown, layer_t) = resolve_common(&sub_ctx(ctx, e, Mat3::IDENTITY), &c, b.plotting);
                    let t = if b.opts.transparency { transparency(c.transparency, layer_t, reference) } else { 0 };
                    (ink, lw, shown, t)
                }
            };
            if shown {
                b.transparency = t;
                let lw = if b.opts.lineweights { lw } else { 0.0 };
                match (&multiline, &sh) {
                    (Some(m), _) => mtext(b, ctx, m, ink, lw),
                    (None, Some(sh)) if qtext(ctx.d) => qtext_box(b, ctx, ink, lw, &[sh], a.text.rotation),
                    (None, Some(sh)) => b.shaped(ctx, ink, lw, sh),
                    (None, None) => {}
                }
            }
        }
        b.transparency = reference;
    }
}

/// Properties that follow the block reference's (an attribute without a definition).
fn by_block() -> cadaddy_doc::Common {
    cadaddy_doc::Common {
        color: Color::ByBlock,
        linetype: "ByBlock".into(),
        lineweight: Lineweight::ByBlock,
        transparency: cadaddy_doc::Transparency::ByBlock,
        ..cadaddy_doc::Common::default()
    }
}

/// The attribute definition with tag `tag` (any case).
fn attdef_tag(e: &Entity, tag: &str) -> bool {
    matches!(&e.kind, EntityKind::AttDef(d) if d.tag.eq_ignore_ascii_case(tag))
}

/// The overall scale of a leader drawn with dimension style `st`: its DIMSCALE, else the
/// drawing's.
fn leader_scale(d: &Drawing, st: Option<&cadaddy_doc::DimStyle>) -> f64 {
    let dimscale = d.header.f64("DIMSCALE", 1.0);
    st.map(|s| s.effective_scale(dimscale)).unwrap_or_else(|| cadaddy_doc::DimStyle::default().effective_scale(dimscale))
}

#[cfg(test)]
mod tests;
