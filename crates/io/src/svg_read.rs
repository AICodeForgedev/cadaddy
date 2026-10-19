//! SVG → Drawing: the static W3C SVG subset that drawing tools emit.
//!
//! Reads shapes (`rect` with rounded corners, `circle`, `ellipse`, `line`, `polyline`,
//! `polygon`), `path` outlines (`M L H V C S Q T A Z`, absolute and relative), `g` grouping with
//! `transform` attributes, `use` references (to any element or a `symbol`, with their `x`/`y`
//! and transforms), `viewBox` and unit-bearing `width`/`height`, Inkscape layers
//! (`inkscape:groupmode="layer"`), CSS rules in `<style>` with simple selectors (`tag`,
//! `.class`, `#id`), fills (a solid hatch in the fill colour; a linear or radial gradient
//! becomes a gradient hatch), text (TEXT, or MTEXT for several lines, with the font size and
//! anchor) and raster images (IMAGE: an embedded one is written out as a file beside the SVG; a
//! linked one is used where it lies). Geometry becomes centre-line entities in millimetres with
//! the page's top-left corner at the origin and Y pointing up — where a laser or pen job wants
//! it.
//!
//! Not read (each is a follow-up): patterns, masks, clip paths and markers, stroke widths and
//! colours, text on a path, gradients' middle stops, and elliptical arcs under non-uniform
//! transforms (those are flattened into line segments instead of losing the shape). A shape
//! whose fill the file never sets is not hatched — SVG paints it black, but line art drawn
//! without styles should stay line art.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use cadaddy_color::{Color, Rgb};
use cadaddy_doc::*;
use cadaddy_geom::{Bounds2, Mat3, PolyVertex, Spline, Vec2, Vec3};
use cadaddy_render::images::Raster;

use crate::svg_style::{self, Anchor, Css, Dom, Kid, Node, Paint, Style};
use crate::{IoError, Result};

/// A decompressed SVGZ payload larger than this is refused (decompression bomb guard).
const MAX_DECOMPRESSED: usize = 64 << 20;
/// Entities per file.
const MAX_ELEMENTS: usize = 500_000;
/// Elements visited (each `use` visits its target again).
const MAX_VISITS: usize = 4_000_000;
/// Path data segments per `d` attribute.
const MAX_PATH_SEGMENTS: usize = 100_000;
/// Vertices in one polyline, and control points in one spline.
const MAX_CURVE_POINTS: usize = 100_001;
/// Vertices in one fill's boundary.
const MAX_FILL_VERTICES: usize = 1_000_000;
/// Layers created from Inkscape layer groups.
const MAX_LAYERS: usize = 256;
/// Nested `<g>`/`<svg>`/`use` transforms.
const MAX_TRANSFORM_DEPTH: usize = 64;
/// Nested `use` references.
const MAX_USE_DEPTH: usize = 16;
/// Flattened chords per elliptical arc under a non-uniform transform.
const MAX_ARC_CHORDS: usize = 10_000;
/// Raster images per file, and the bytes of one.
const MAX_IMAGES: usize = 256;
const MAX_IMAGE_BYTES: usize = 64 << 20;
/// Characters in one text element.
const MAX_TEXT_CHARS: usize = 65_536;
/// Notes about what couldn't be read.
const MAX_NOTES: usize = 32;
/// A drafting text height is its capital letters' height; an SVG font size is the em, of which
/// capitals take about this much in common fonts.
const CAP_HEIGHT: f64 = 0.7;

/// Elements whose whole subtree is skipped where it stands (definitions are reached through
/// `use` and `url(#…)` instead; paint servers, metadata).
const SKIP: &[&str] = &[
    "defs",
    "symbol",
    "clipPath",
    "mask",
    "pattern",
    "marker",
    "linearGradient",
    "radialGradient",
    "filter",
    "style",
    "title",
    "desc",
    "metadata",
    "script",
    "foreignObject",
];

/// Read an SVG document, or a gzip-compressed one (`.svgz`, Inkscape's "Compressed SVG"). `name`
/// is the file's path: embedded images are written beside it, linked ones looked up from its
/// folder (a bare name has no folder: embedded images then show in this session only).
pub fn read(bytes: &[u8], name: &str) -> Result<Drawing> {
    let unpacked;
    let plain: &[u8] = if bytes.starts_with(&[0x1f, 0x8b]) {
        unpacked = gunzip(bytes)?;
        &unpacked
    } else {
        bytes
    };
    let text = std::str::from_utf8(plain).map_err(|_| IoError::Format("SVG files must be UTF-8".into()))?;
    let dom = svg_style::build(text)?;
    let mut parser = Parser::new(name);
    for n in dom.nodes.iter().filter(|n| n.name == "style") {
        if n.attr("type").is_none_or(|t| t.trim().eq_ignore_ascii_case("text/css")) {
            parser.css.add(&n.own_text());
        }
    }
    if let Some(root) = dom.root {
        parser.walk(&dom, root, &Style::default())?;
    }
    let mut d = parser.drawing;
    d.read_notes.extend(parser.notes);
    Ok(d)
}

/// Decompress a gzip member (RFC 1952) with a hard output cap.
fn gunzip(bytes: &[u8]) -> Result<Vec<u8>> {
    let damaged = || IoError::Format("this SVGZ file's gzip stream is damaged".into());
    if bytes.get(2) != Some(&8) {
        return Err(damaged());
    }
    let flg = bytes.get(3).copied().unwrap_or(0);
    let mut pos = 10usize;
    if flg & 0x04 != 0 {
        // FEXTRA
        let extra = bytes.get(pos..pos + 2).ok_or_else(damaged)?;
        pos += 2 + usize::from(u16::from_le_bytes([extra[0], extra[1]]));
    }
    if flg & 0x08 != 0 {
        pos = skip_cstring(bytes, pos).ok_or_else(damaged)?; // FNAME
    }
    if flg & 0x10 != 0 {
        pos = skip_cstring(bytes, pos).ok_or_else(damaged)?; // FCOMMENT
    }
    if flg & 0x02 != 0 {
        pos = pos.saturating_add(2); // FHCRC
    }
    let end = bytes.len().checked_sub(8).filter(|e| *e >= pos).ok_or_else(damaged)?;
    miniz_oxide::inflate::decompress_to_vec_with_limit(&bytes[pos..end], MAX_DECOMPRESSED).map_err(|_| damaged())
}

/// Skip a NUL-terminated field; `None` when the terminator is missing.
fn skip_cstring(bytes: &[u8], from: usize) -> Option<usize> {
    let rest = bytes.get(from..)?;
    let nul = rest.iter().position(|b| *b == 0)?;
    Some(from + nul + 1)
}

fn parse_err<E: std::fmt::Display>(e: E) -> IoError {
    IoError::Format(format!("SVG: {e}"))
}

fn v3(p: Vec2) -> Vec3 {
    Vec3::new(p.x, p.y, 0.0)
}

/// What a fill paint comes to.
enum Fill {
    Solid(Rgb),
    Gradient(Gradient),
}

/// One line of a text element: where its first character sits (user units), what it says, and
/// how it is sized and anchored.
struct TextLine {
    at: Vec2,
    text: String,
    size: f64,
    anchor: Anchor,
}

struct Parser {
    drawing: Drawing,
    /// Root viewBox/unit mapping composed with every open container transform.
    stack: Vec<Mat3>,
    root_seen: bool,
    /// Current Inkscape layer name.
    layer: String,
    layers: HashSet<String>,
    layer_color: u8,
    elements: usize,
    visits: usize,
    css: Css,
    /// The `use` targets being read, innermost last (a reference cycle stops there).
    uses: Vec<usize>,
    /// The SVG's folder (embedded images are written there) and file name without extension.
    dir: Option<std::path::PathBuf>,
    stem: String,
    /// Embedded image bytes already placed → their path.
    images: HashMap<u64, String>,
    image_count: usize,
    notes: Vec<String>,
}

impl Parser {
    fn new(name: &str) -> Self {
        let p = std::path::Path::new(name);
        let dir = p.parent().filter(|d| !d.as_os_str().is_empty() && cfg!(not(target_arch = "wasm32"))).map(std::path::Path::to_path_buf);
        let stem: String = p
            .file_stem()
            .map(|s| s.to_string_lossy().chars().map(|c| if c.is_alphanumeric() || matches!(c, '-' | '_' | ' ' | '.') { c } else { '_' }).collect())
            .filter(|s: &String| !s.trim().is_empty())
            .unwrap_or_else(|| "svg".into());
        Parser {
            drawing: Drawing::new_metric(),
            stack: vec![Mat3::IDENTITY],
            root_seen: false,
            layer: "0".into(),
            layers: HashSet::from(["0".to_string()]),
            layer_color: 1,
            elements: 0,
            visits: 0,
            css: Css::default(),
            uses: Vec::new(),
            dir,
            stem,
            images: HashMap::new(),
            image_count: 0,
            notes: Vec::new(),
        }
    }

    fn note(&mut self, s: String) {
        if self.notes.len() < MAX_NOTES && !self.notes.contains(&s) {
            self.notes.push(s);
        }
    }

    fn walk(&mut self, dom: &Dom, i: usize, parent: &Style) -> Result<()> {
        let Some(n) = dom.node(i) else { return Ok(()) };
        self.visits += 1;
        if self.visits > MAX_VISITS {
            return Err(IoError::Format(format!("SVG: more than {MAX_VISITS} elements to read (through `use` references)")));
        }
        if SKIP.contains(&n.name.as_str()) {
            return Ok(());
        }
        let style = Style::of(n, parent, &self.css);
        if style.hidden {
            return Ok(());
        }
        match n.name.as_str() {
            "g" | "svg" | "a" | "switch" => self.group(dom, n, &style),
            "use" => self.use_ref(dom, n, &style),
            name => {
                // A shape's own transform applies to it alone.
                self.push(n.attr("transform").and_then(parse_transform))?;
                let r = match name {
                    "text" => self.text(dom, n, &style),
                    "image" => self.image(n),
                    _ => self.shape(dom, n, &style),
                };
                self.stack.pop();
                r
            }
        }
    }

    fn kids(&mut self, dom: &Dom, n: &Node, style: &Style) -> Result<()> {
        for k in &n.kids {
            if let Kid::El(c) = k {
                self.walk(dom, *c, style)?;
            }
        }
        Ok(())
    }

    /// Open a transform frame: `local` applied inside the current mapping.
    fn push(&mut self, local: Option<Mat3>) -> Result<()> {
        if self.stack.len() >= MAX_TRANSFORM_DEPTH {
            return Err(IoError::Format(format!("SVG: more than {MAX_TRANSFORM_DEPTH} nested groups")));
        }
        let current = self.cur();
        self.stack.push(local.map_or(current, |l| current.then_before(l)));
        Ok(())
    }

    fn group(&mut self, dom: &Dom, n: &Node, style: &Style) -> Result<()> {
        if n.name == "svg" && !self.root_seen {
            self.root_seen = true;
            if let Some(root) = self.stack.first_mut() {
                *root = viewport(n)?;
            }
        }
        let layer = self.layer.clone();
        if n.name == "g"
            && n.attr("inkscape:groupmode") == Some("layer")
            && let Some(raw) = n.attr("inkscape:label").or_else(|| n.attr("id"))
        {
            self.set_layer(raw);
        }
        self.push(n.attr("transform").and_then(parse_transform))?;
        let r = self.kids(dom, n, style);
        self.stack.pop();
        self.layer = layer;
        r
    }

    /// `use`: the referenced element drawn again, moved by `x`/`y` inside the `use`'s transform
    /// and styled as a child of the `use`. A `symbol`'s `viewBox` maps onto the `use`'s
    /// `width`/`height`.
    fn use_ref(&mut self, dom: &Dom, n: &Node, style: &Style) -> Result<()> {
        let Some(target) = n.href().and_then(|h| dom.by_ref(h)) else { return Ok(()) };
        let Some(t) = dom.node(target) else { return Ok(()) };
        if self.uses.contains(&target) || self.uses.len() >= MAX_USE_DEPTH {
            self.note("SVG: a `use` reference that refers back to itself was left out".into());
            return Ok(());
        }
        let place = Mat3::translate(Vec2::new(num(n, "x", 0.0), num(n, "y", 0.0)));
        let local = n.attr("transform").and_then(parse_transform).map_or(place, |m| m.then_before(place));
        self.push(Some(local))?;
        self.uses.push(target);
        let r = if t.name == "symbol" {
            let st = Style::of(t, style, &self.css);
            match self.push(Some(symbol_viewbox(t, n))) {
                Ok(()) => {
                    let r = if st.hidden { Ok(()) } else { self.kids(dom, t, &st) };
                    self.stack.pop();
                    r
                }
                Err(e) => Err(e),
            }
        } else {
            self.walk(dom, target, style)
        };
        self.uses.pop();
        self.stack.pop();
        r
    }

    fn set_layer(&mut self, raw: &str) {
        let name = sanitize_layer(raw);
        if !self.layers.contains(&name) {
            if self.layers.len() >= MAX_LAYERS {
                return;
            }
            let color = Color::Index(self.layer_color);
            self.layer_color = if self.layer_color >= 7 { 1 } else { self.layer_color + 1 };
            self.drawing.layers.push(Layer { name: name.clone(), color, ..Layer::default() });
            self.layers.insert(name.clone());
        }
        self.layer = name;
    }

    /// The composed mapping for the current nesting: SVG user units → drawing units (mm).
    fn cur(&self) -> Mat3 {
        *self.stack.last().unwrap_or(&Mat3::IDENTITY)
    }

    fn map(&self, p: Vec2) -> Vec2 {
        self.cur().apply(p)
    }

    fn map_vec(&self, v: Vec2) -> Vec2 {
        self.cur().apply_vec(v)
    }

    fn add(&mut self, kind: EntityKind) -> Result<()> {
        self.add_colored(kind, Color::ByLayer)
    }

    fn add_colored(&mut self, kind: EntityKind, color: Color) -> Result<()> {
        self.elements += 1;
        if self.elements > MAX_ELEMENTS {
            return Err(IoError::Format(format!("SVG: more than {MAX_ELEMENTS} elements")));
        }
        let common = Common { layer: self.layer.clone(), color, ..Common::default() };
        self.drawing.add(&Space::Model, common, kind).map(|_| ()).map_err(parse_err)
    }

    // --- shapes -------------------------------------------------------------

    /// A shape: its fill (painted first, so the outline shows over it), then its outline.
    fn shape(&mut self, dom: &Dom, n: &Node, style: &Style) -> Result<()> {
        match n.name.as_str() {
            "rect" => self.rect(dom, n, style),
            "circle" => {
                let r = num(n, "r", 0.0);
                self.round(dom, n, style, r, r)
            }
            "ellipse" => {
                let (rx, ry) = (num(n, "rx", 0.0), num(n, "ry", 0.0));
                self.round(dom, n, style, rx, ry)
            }
            "line" => {
                let a = Vec2::new(num(n, "x1", 0.0), num(n, "y1", 0.0));
                let b = Vec2::new(num(n, "x2", 0.0), num(n, "y2", 0.0));
                self.add(EntityKind::Line(Line { a: v3(self.map(a)), b: v3(self.map(b)) }))
            }
            "polyline" | "polygon" => {
                let Some(raw) = n.attr("points") else { return Ok(()) };
                let closed = n.name == "polygon";
                let nums: Vec<f64> = Numbers::new(raw).collect();
                let mut pts: Vec<Vec2> = nums.as_chunks::<2>().0.iter().map(|[x, y]| Vec2::new(*x, *y)).collect();
                pts.truncate(MAX_CURVE_POINTS);
                if let Some((first, rest)) = pts.split_first() {
                    let sp = Subpath { start: *first, segs: rest.iter().map(|p| Seg::Line(*p)).collect(), closed };
                    self.fill(dom, std::slice::from_ref(&sp), style)?;
                }
                let mapped: Vec<Vec2> = pts.iter().map(|p| self.map(*p)).collect();
                self.add_polyline(&mapped, closed)
            }
            "path" => {
                let Some(d) = n.attr("d") else { return Ok(()) };
                let sps = parse_path(d)?;
                self.fill(dom, &sps, style)?;
                for sp in &sps {
                    self.emit_subpath(sp)?;
                }
                Ok(())
            }
            _ => Ok(()),
        }
    }

    fn rect(&mut self, dom: &Dom, n: &Node, style: &Style) -> Result<()> {
        let x = num(n, "x", 0.0);
        let y = num(n, "y", 0.0);
        let w = num(n, "width", 0.0);
        let h = num(n, "height", 0.0);
        if w <= 0.0 || h <= 0.0 {
            return Ok(());
        }
        // Corner radii: one given stands for both; each at most half the side.
        let given = |k: &str| n.attr(k).and_then(parse_length_mm).filter(|r| *r >= 0.0);
        let (rx, ry) = match (given("rx"), given("ry")) {
            (Some(rx), Some(ry)) => (rx, ry),
            (Some(r), None) | (None, Some(r)) => (r, r),
            (None, None) => (0.0, 0.0),
        };
        let (rx, ry) = (rx.min(w / 2.0), ry.min(h / 2.0));
        if rx > 0.0 && ry > 0.0 {
            let sp = rounded_rect(x, y, w, h, rx, ry);
            self.fill(dom, std::slice::from_ref(&sp), style)?;
            return self.emit_subpath(&sp);
        }
        let pts = [Vec2::new(x, y), Vec2::new(x + w, y), Vec2::new(x + w, y + h), Vec2::new(x, y + h)];
        let sp = Subpath { start: pts[0], segs: pts[1..].iter().map(|p| Seg::Line(*p)).collect(), closed: true };
        self.fill(dom, std::slice::from_ref(&sp), style)?;
        self.add_polyline(&pts.map(|p| self.map(p)), true)
    }

    /// A circle or an ellipse, and its fill.
    fn round(&mut self, dom: &Dom, n: &Node, style: &Style, rx: f64, ry: f64) -> Result<()> {
        if rx <= 0.0 || ry <= 0.0 {
            return Ok(());
        }
        let c = Vec2::new(num(n, "cx", 0.0), num(n, "cy", 0.0));
        let arc = |end: Vec2| Seg::Arc { rx, ry, rot: 0.0, large: false, sweep: true, end };
        let start = c + Vec2::new(rx, 0.0);
        let sp = Subpath { start, segs: vec![arc(c - Vec2::new(rx, 0.0)), arc(start)], closed: true };
        self.fill(dom, std::slice::from_ref(&sp), style)?;
        self.add_rounded(c, rx, ry)
    }

    /// A circle or ellipse through a transform. A circle stays a `CIRCLE` under rotations,
    /// uniform scales and reflections; anything else becomes an `ELLIPSE` holding the axes
    /// of the transformed shape — a linear map sends the unit circle to an ellipse whose
    /// semi-axes are the square roots of the eigenvalues of `A·Aᵀ`.
    fn add_rounded(&mut self, center_user: Vec2, rx: f64, ry: f64) -> Result<()> {
        let center = self.map(center_user);
        let bx = self.map_vec(Vec2::new(rx, 0.0));
        let by = self.map_vec(Vec2::new(0.0, ry));
        let len = |v: Vec2| v.x.hypot(v.y);
        let (lbx, lby, dot) = (len(bx), len(by), bx.x * by.x + bx.y * by.y);
        if lbx.is_finite() && lby.is_finite() && (lbx - lby).abs() <= 1e-9 * (lbx + lby) && dot.abs() <= 1e-9 * lbx * lby {
            return self.add(EntityKind::Circle(Circle { center: v3(center), radius: lbx }));
        }
        let xx = bx.x * bx.x + bx.y * bx.y;
        let yy = by.x * by.x + by.y * by.y;
        let xy = bx.x * by.x + bx.y * by.y;
        let trace = xx + yy;
        let disc = (trace * trace - 4.0 * (xx * yy - xy * xy)).max(0.0).sqrt();
        let l_max = 0.5 * (trace + disc);
        let l_min = 0.5 * (trace - disc);
        let s_max = l_max.max(0.0).sqrt();
        if s_max <= f64::EPSILON {
            return Ok(());
        }
        let axis = if xy.abs() <= 1e-12 * trace.max(1.0) {
            if yy > xx { Vec2::new(0.0, 1.0) } else { Vec2::new(1.0, 0.0) }
        } else {
            let v = Vec2::new(xy, l_max - xx);
            let l = len(v);
            if l <= f64::EPSILON {
                return Ok(());
            }
            v / l
        };
        let ratio = (l_min.max(0.0).sqrt() / s_max).clamp(1e-9, 1.0);
        let major = center + axis * s_max;
        self.add(EntityKind::Ellipse(Ellipse { center: v3(center), major: v3(major), ratio, start: 0.0, end: std::f64::consts::TAU }))
    }

    fn add_polyline(&mut self, pts: &[Vec2], closed: bool) -> Result<()> {
        if pts.len() < 2 {
            return Ok(());
        }
        let vertices = pts.iter().map(|p| PolyVertex::new(*p)).collect();
        self.add(EntityKind::LwPolyline(LwPolyline { vertices, closed, const_width: 0.0, elevation: 0.0, plinegen: false }))
    }

    // --- fills --------------------------------------------------------------

    /// The fill of a shape made of `sps` (user units): a solid hatch in the fill colour, or a
    /// gradient hatch. Open subpaths are closed, as SVG fills them; holes follow the even-odd
    /// rule (a non-zero file's overlapping loops show as holes too).
    fn fill(&mut self, dom: &Dom, sps: &[Subpath], style: &Style) -> Result<()> {
        let fill = match &style.fill {
            None | Some(Paint::None) => return Ok(()),
            Some(Paint::Color(c)) => Fill::Solid(*c),
            Some(Paint::Current) => Fill::Solid(style.color),
            Some(Paint::Url(r, fallback)) => match self.gradient(dom, r, sps) {
                Some(f) => f,
                None => match fallback {
                    Some(c) => Fill::Solid(*c),
                    None => return Ok(()),
                },
            },
        };
        let loops = self.loops(sps);
        if loops.is_empty() {
            return Ok(());
        }
        let (color, gradient) = match fill {
            Fill::Solid(c) => (Color::True(c), None),
            Fill::Gradient(g) => (Color::ByLayer, Some(g)),
        };
        let hatch = Hatch {
            pattern: "SOLID".into(),
            solid: true,
            loops,
            scale: 1.0,
            angle: 0.0,
            associative: false,
            style: 0,
            elevation: 0.0,
            gradient,
            origin: Vec2::ZERO,
            background: None,
            pattern_lines: Vec::new(),
        };
        self.add_colored(EntityKind::Hatch(hatch), color)
    }

    /// The hatch loops of `sps` in drawing units: lines and circular arcs as polyline vertices
    /// (arcs as bulges), curves and elliptical arcs flattened.
    fn loops(&self, sps: &[Subpath]) -> Vec<HatchLoop> {
        let m = self.cur();
        let frame = Frame::of(&m);
        let mut out: Vec<HatchLoop> = Vec::new();
        let mut total = 0usize;
        for sp in sps {
            let mut v: Vec<PolyVertex> = vec![PolyVertex::new(m.apply(sp.start))];
            let mut cur_user = sp.start;
            for seg in &sp.segs {
                let cur = m.apply(cur_user);
                match seg {
                    Seg::Line(p) => v.push(PolyVertex::new(m.apply(*p))),
                    Seg::Cubic(c1, c2, p) => {
                        let (q1, q2, q3) = (m.apply(*c1), m.apply(*c2), m.apply(*p));
                        let reach = cur.dist(q1) + q1.dist(q2) + q2.dist(q3);
                        let steps = if reach.is_finite() { ((reach / 0.5).sqrt().ceil() as usize).clamp(4, 64) } else { 4 };
                        for k in 1..=steps {
                            let t = k as f64 / steps as f64;
                            let s = 1.0 - t;
                            v.push(PolyVertex::new(cur * (s * s * s) + q1 * (3.0 * s * s * t) + q2 * (3.0 * s * t * t) + q3 * (t * t * t)));
                        }
                    }
                    Seg::Arc { rx, ry, rot, large, sweep, end } => {
                        if let Some(delta) = frame
                            .circular(*rx, *ry)
                            .and_then(|r| arc_center_circular(cur, m.apply(*end), r, *large, *sweep != frame.flipped))
                            .map(|(_, d)| d)
                        {
                            if let Some(last) = v.last_mut() {
                                last.bulge = (delta / 4.0).tan();
                            }
                            v.push(PolyVertex::new(m.apply(*end)));
                        } else if *rx > 0.0
                            && *ry > 0.0
                            && let Ok(pts) = flatten_arc(&m, cur_user, *end, *rx, *ry, *rot, *large, *sweep)
                        {
                            v.extend(pts.into_iter().skip(1).map(PolyVertex::new));
                        } else {
                            v.push(PolyVertex::new(m.apply(*end)));
                        }
                    }
                }
                cur_user = seg.end();
                if total + v.len() > MAX_FILL_VERTICES {
                    return Vec::new();
                }
            }
            // The closing vertex repeats the first: the one before it then holds the closing
            // segment's bulge.
            if v.len() > 2 && v.first().zip(v.last()).is_some_and(|(a, b)| a.p.dist(b.p) <= 1e-9 * (1.0 + a.p.x.abs().max(a.p.y.abs()))) {
                v.pop();
            }
            let curved = v.iter().any(|x| x.bulge != 0.0);
            if v.len() >= 3 || (v.len() == 2 && curved) {
                total += v.len();
                out.push(HatchLoop { curves: Vec::new(), vertices: v, outer: out.is_empty(), sources: Vec::new() });
            }
        }
        out
    }

    /// The fill a `url(#…)` paint names: a linear gradient as a LINEAR gradient hatch along the
    /// gradient's direction, a radial one as SPHERICAL (its centre colour in the middle); the
    /// first and last stops are the two colours. One colour (or one stop) is a solid fill.
    fn gradient(&self, dom: &Dom, r: &str, sps: &[Subpath]) -> Option<Fill> {
        let first = dom.by_ref(r)?;
        let kind = dom.node(first)?.name.clone();
        if kind != "linearGradient" && kind != "radialGradient" {
            return None;
        }
        // Gradients inherit attributes and stops through `href`.
        let mut chain = vec![first];
        while chain.len() < 8 {
            let Some(next) = chain.last().and_then(|i| dom.node(*i)).and_then(Node::href).and_then(|h| dom.by_ref(h)) else { break };
            if chain.contains(&next) {
                break;
            }
            chain.push(next);
        }
        let attr = |k: &str| chain.iter().find_map(|i| dom.node(*i).and_then(|n| n.attr(k)));
        let stops: Vec<Rgb> = chain.iter().find_map(|i| {
            let n = dom.node(*i)?;
            let stops: Vec<Rgb> = n
                .kids
                .iter()
                .filter_map(|k| if let Kid::El(s) = k { dom.node(*s) } else { None })
                .filter(|s| s.name == "stop")
                .map(|s| Style::of(s, &Style::default(), &self.css).stop.unwrap_or(Rgb(0, 0, 0)))
                .collect();
            (!stops.is_empty()).then_some(stops)
        })?;
        let (c0, c1) = (*stops.first()?, *stops.last()?);
        if c0 == c1 {
            return Some(Fill::Solid(c0));
        }
        let user_space = attr("gradientUnits") == Some("userSpaceOnUse");
        let gt = attr("gradientTransform").and_then(parse_transform).unwrap_or(Mat3::IDENTITY);
        let bb = Bounds2::from_points(sps.iter().flat_map(Subpath::points));
        if bb.is_empty() {
            return None;
        }
        let coord = |k: &str, default: f64| attr(k).and_then(fraction).unwrap_or(default);
        if kind == "linearGradient" {
            let (x1, y1, x2, y2) = (coord("x1", 0.0), coord("y1", 0.0), coord("x2", 1.0), coord("y2", 0.0));
            let d = gt.apply_vec(Vec2::new(x2 - x1, y2 - y1));
            let d = if user_space { d } else { Vec2::new(d.x * bb.width(), d.y * bb.height()) };
            let dd = self.map_vec(d);
            if !(dd.x.is_finite() && dd.y.is_finite()) || dd.x.hypot(dd.y) <= 1e-12 {
                return Some(Fill::Solid(c1));
            }
            return Some(Fill::Gradient(Gradient {
                name: "LINEAR".into(),
                color1: Color::True(c0),
                color2: Color::True(c1),
                angle: dd.y.atan2(dd.x),
                centered: true,
            }));
        }
        let (cx, cy) = (coord("cx", 0.5), coord("cy", 0.5));
        let centered = if user_space {
            let c = gt.apply(Vec2::new(cx, cy));
            c.dist(bb.center()) <= 0.05 * bb.width().max(bb.height())
        } else {
            let c = gt.apply(Vec2::new(cx, cy));
            (c.x - 0.5).abs() <= 0.05 && (c.y - 0.5).abs() <= 0.05
        };
        Some(Fill::Gradient(Gradient { name: "SPHERICAL".into(), color1: Color::True(c1), color2: Color::True(c0), angle: 0.0, centered }))
    }

    // --- text ---------------------------------------------------------------

    /// A text element: one line becomes TEXT, several (tspans placed with their own `x` or
    /// `y`, as Inkscape writes lines) one MTEXT. The text height is the font's capital height.
    fn text(&mut self, dom: &Dom, n: &Node, style: &Style) -> Result<()> {
        let at = Vec2::new(
            first_num(n, "x").unwrap_or(0.0) + first_num(n, "dx").unwrap_or(0.0),
            first_num(n, "y").unwrap_or(0.0) + first_num(n, "dy").unwrap_or(0.0),
        );
        let mut lines = vec![TextLine { at, text: String::new(), size: style.font_size, anchor: style.anchor }];
        self.text_runs(dom, n, style, &mut lines, 0);
        let lines: Vec<TextLine> = lines
            .into_iter()
            .map(|mut l| {
                l.text = l.text.split_whitespace().collect::<Vec<_>>().join(" ").chars().take(MAX_TEXT_CHARS).collect();
                l
            })
            .filter(|l| !l.text.is_empty())
            .collect();
        let Some(first) = lines.first() else { return Ok(()) };
        let ex = self.map_vec(Vec2::new(1.0, 0.0));
        let ey = self.map_vec(Vec2::new(0.0, 1.0));
        let (sx, sy) = (ex.x.hypot(ex.y), ey.x.hypot(ey.y));
        let height = CAP_HEIGHT * first.size * sy;
        if !(height.is_finite() && height > 0.0 && sx.is_finite() && sx > 0.0) {
            return Ok(());
        }
        let rotation = ex.y.atan2(ex.x);
        let width_factor = if (sx - sy).abs() <= 1e-9 * sx.max(sy) { 1.0 } else { (sx / sy).clamp(0.01, 100.0) };
        let color = style.fill_rgb().map_or(Color::ByLayer, Color::True);
        let base = self.map(first.at);
        if let [line] = lines.as_slice() {
            let (halign, align_pt) = match line.anchor {
                Anchor::Start => (HAlign::Left, None),
                Anchor::Middle => (HAlign::Center, Some(v3(base))),
                Anchor::End => (HAlign::Right, Some(v3(base))),
            };
            let t = Text {
                insert: v3(base),
                align_pt,
                height,
                value: line.text.clone(),
                rotation,
                width_factor,
                oblique: 0.0,
                style: "Standard".into(),
                halign,
                valign: VAlign::Baseline,
            };
            return self.add_colored(EntityKind::Text(t), color);
        }
        // MTEXT hangs from the top of its first line.
        let up = Vec2::new(-rotation.sin(), rotation.cos());
        let pitch = lines.get(1).map(|l| self.map(l.at).dist(base)).filter(|p| p.is_finite() && *p > 0.0);
        let line_spacing = pitch.map_or(1.0, |p| (p / (height * 5.0 / 3.0)).clamp(0.25, 4.0));
        let attach = match first.anchor {
            Anchor::Start => 1,
            Anchor::Middle => 2,
            Anchor::End => 3,
        };
        let contents = lines.iter().map(|l| mtext_escape(&l.text)).collect::<Vec<_>>().join("\\P");
        let m = MText {
            insert: v3(base + up * height),
            height,
            width: 0.0,
            attach,
            rotation,
            style: "Standard".into(),
            contents,
            line_spacing,
            line_spacing_exact: true,
            columns: None,
            background: None,
        };
        self.add_colored(EntityKind::MText(m), color)
    }

    /// Gather a text element's characters into lines.
    fn text_runs(&self, dom: &Dom, n: &Node, style: &Style, lines: &mut Vec<TextLine>, depth: usize) {
        for k in &n.kids {
            match k {
                Kid::Text(t) => {
                    if let Some(l) = lines.last_mut() {
                        l.text.push_str(t);
                    }
                }
                Kid::El(c) => {
                    let Some(span) = dom.node(*c) else { continue };
                    if !matches!(span.name.as_str(), "tspan" | "a") || depth >= 16 {
                        continue;
                    }
                    let st = Style::of(span, style, &self.css);
                    if st.hidden {
                        continue;
                    }
                    let (x, y) = (first_num(span, "x"), first_num(span, "y"));
                    if x.is_some() || y.is_some() {
                        let prev = lines.last().map_or(Vec2::ZERO, |l| l.at);
                        let at = Vec2::new(x.unwrap_or(prev.x), y.unwrap_or(prev.y) + first_num(span, "dy").unwrap_or(0.0));
                        match lines.last_mut() {
                            Some(l) if l.text.trim().is_empty() => *l = TextLine { at, text: String::new(), size: st.font_size, anchor: st.anchor },
                            _ => {
                                if lines.len() < 1000 {
                                    lines.push(TextLine { at, text: String::new(), size: st.font_size, anchor: st.anchor });
                                }
                            }
                        }
                    }
                    self.text_runs(dom, span, &st, lines, depth + 1);
                }
            }
        }
    }

    // --- images -------------------------------------------------------------

    /// An `image`: placed in its box as `preserveAspectRatio` says (centred and fitted by
    /// default; `slice` clips the overflow).
    fn image(&mut self, n: &Node) -> Result<()> {
        let Some(href) = n.href() else { return Ok(()) };
        if self.image_count >= MAX_IMAGES {
            self.note(format!("SVG: images past the first {MAX_IMAGES} were left out"));
            return Ok(());
        }
        let Some((raster, path)) = (if href.trim_start().starts_with("data:") { self.embedded(href) } else { self.linked(href) }) else {
            return Ok(());
        };
        let (pw, ph) = (f64::from(raster.width), f64::from(raster.height));
        let given = |k: &str| n.attr(k).and_then(parse_length_mm).filter(|v| *v > 0.0);
        let (w, h) = match (given("width"), given("height")) {
            (Some(w), Some(h)) => (w, h),
            (Some(w), None) => (w, w * ph / pw),
            (None, Some(h)) => (h * pw / ph, h),
            (None, None) => (pw, ph),
        };
        let (x, y) = (num(n, "x", 0.0), num(n, "y", 0.0));
        let (ix, iy, iw, ih, clip) = fit_image(x, y, w, h, pw, ph, n.attr("preserveAspectRatio").unwrap_or(""));
        let transparent = raster.rgba.as_chunks::<4>().0.iter().any(|p| p[3] < 255);
        let name =
            std::path::Path::new(path.rsplit(':').next().unwrap_or(&path)).file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
        let clipping = !clip.is_empty();
        let image = Image {
            insert: v3(self.map(Vec2::new(ix, iy + ih))),
            u: v3(self.map_vec(Vec2::new(iw / pw, 0.0))),
            v: v3(self.map_vec(Vec2::new(0.0, -ih / ph))),
            size: Vec2::new(pw, ph),
            path,
            name,
            clip,
            clipping,
            display: 1 | 2 | if clipping { 4 } else { 0 } | if transparent { 8 } else { 0 },
            ..Image::default()
        };
        self.add(EntityKind::Image(image))
    }

    /// A `data:` image: decoded, then written beside the SVG (once per distinct image).
    fn embedded(&mut self, href: &str) -> Option<(Arc<Raster>, String)> {
        let Some((mime, bytes)) = svg_style::data_url(href, MAX_IMAGE_BYTES) else {
            self.note("SVG: an embedded image that isn't base64 data was left out".into());
            return None;
        };
        let Some(raster) = cadaddy_render::images::decode(&bytes).map(Arc::new) else {
            self.note(format!("SVG: an embedded {mime} image couldn't be decoded and was left out"));
            return None;
        };
        let key = {
            use std::hash::{Hash, Hasher};
            let mut hs = std::collections::hash_map::DefaultHasher::new();
            bytes.hash(&mut hs);
            hs.finish()
        };
        if let Some(p) = self.images.get(&key) {
            return Some((raster, p.clone()));
        }
        self.image_count += 1;
        let ext = match mime.as_str() {
            "image/jpeg" | "image/jpg" => "jpg",
            "image/gif" => "gif",
            "image/webp" => "webp",
            "image/bmp" => "bmp",
            _ => "png",
        };
        let file = format!("{}-image-{}.{ext}", self.stem, self.image_count);
        let path = self.write_image(&file, &bytes).unwrap_or_else(|| format!("svg-image:{file}"));
        cadaddy_render::images::insert(&path, Arc::clone(&raster));
        self.images.insert(key, path.clone());
        Some((raster, path))
    }

    /// Write an embedded image's bytes into the SVG's folder; an existing file of that name
    /// with other content is kept (the next free name is used). `None` when there is no
    /// folder or it can't be written.
    #[cfg(not(target_arch = "wasm32"))]
    fn write_image(&mut self, file: &str, bytes: &[u8]) -> Option<String> {
        let dir = self.dir.clone()?;
        let (stem, ext) = file.rsplit_once('.').unwrap_or((file, ""));
        for k in 0..100 {
            let name = if k == 0 { file.to_string() } else { format!("{stem}-{}.{ext}", k + 1) };
            let p = dir.join(&name);
            match std::fs::read(&p) {
                Ok(existing) if existing == bytes => return Some(p.to_string_lossy().into_owned()),
                Ok(_) => continue,
                Err(_) => {
                    return match std::fs::write(&p, bytes) {
                        Ok(()) => Some(p.to_string_lossy().into_owned()),
                        Err(e) => {
                            self.note(format!(
                                "SVG: the embedded images couldn't be written to {}: {e}; they show in this session only",
                                dir.display()
                            ));
                            None
                        }
                    };
                }
            }
        }
        None
    }

    #[cfg(target_arch = "wasm32")]
    fn write_image(&mut self, _file: &str, _bytes: &[u8]) -> Option<String> {
        None
    }

    /// A linked image file, looked up from the SVG's folder.
    #[cfg(not(target_arch = "wasm32"))]
    fn linked(&mut self, href: &str) -> Option<(Arc<Raster>, String)> {
        let raw = href.trim();
        let raw = raw.strip_prefix("file://").unwrap_or(raw);
        if raw.contains("://") {
            self.note(format!("SVG: the image at {raw} is on the web and was left out"));
            return None;
        }
        let decoded = raw.replace("%20", " ");
        let p = std::path::Path::new(&decoded);
        let full = if p.is_absolute() { p.to_path_buf() } else { self.dir.as_ref()?.join(p) };
        let full = cadaddy_geom::filename::resolve(full);
        let readable = std::fs::metadata(&full).is_ok_and(|m| m.is_file() && m.len() <= MAX_IMAGE_BYTES as u64);
        let Some(raster) = readable.then(|| std::fs::read(&full).ok()).flatten().and_then(|b| cadaddy_render::images::decode(&b)).map(Arc::new)
        else {
            self.note(format!("SVG: the linked image {raw} couldn't be read and was left out"));
            return None;
        };
        self.image_count += 1;
        let path = full.to_string_lossy().into_owned();
        cadaddy_render::images::insert(&path, Arc::clone(&raster));
        Some((raster, path))
    }

    #[cfg(target_arch = "wasm32")]
    fn linked(&mut self, href: &str) -> Option<(Arc<Raster>, String)> {
        self.note(format!("SVG: the linked image {} can't be read on the web and was left out", href.trim()));
        None
    }

    // --- path data ----------------------------------------------------------

    /// Emit one subpath: runs of lines and arcs become an `LWPOLYLINE` (an arc becomes a
    /// bulge vertex, exactly like DXF's polyline bulge), runs of Bézier curves become a
    /// `SPLINE`. Under a non-uniform transform, or for an elliptical arc, the arc is no longer
    /// circular, so it is flattened into line vertices instead.
    fn emit_subpath(&mut self, sp: &Subpath) -> Result<()> {
        let m = self.cur();
        let frame = Frame::of(&m);

        let mut run: Vec<PolyVertex> = Vec::new();
        let mut ctrl: Vec<Vec2> = Vec::new();
        let mut cur_user = sp.start;
        let mut cur = m.apply(sp.start);
        let curved = sp.segs.iter().any(|s| matches!(s, Seg::Cubic(..)));

        for seg in &sp.segs {
            match seg {
                Seg::Line(p) => {
                    if run.is_empty() {
                        run.push(PolyVertex::new(cur));
                    }
                    cur = m.apply(*p);
                    run.push(PolyVertex::new(cur));
                    cur_user = *p;
                }
                Seg::Cubic(c1, c2, p) => {
                    self.flush_run(&mut run, false)?;
                    if ctrl.is_empty() {
                        ctrl.push(cur);
                    }
                    ctrl.push(m.apply(*c1));
                    ctrl.push(m.apply(*c2));
                    cur = m.apply(*p);
                    ctrl.push(cur);
                    cur_user = *p;
                    if ctrl.len() > MAX_CURVE_POINTS {
                        return Err(IoError::Format("SVG: one path has too many curve points".into()));
                    }
                }
                Seg::Arc { rx, ry, rot, large, sweep, end } => {
                    if let Some(r) = frame.circular(*rx, *ry) {
                        if let Some((_, delta)) = arc_center_circular(cur, m.apply(*end), r, *large, *sweep != frame.flipped) {
                            if run.is_empty() {
                                run.push(PolyVertex::new(cur));
                            }
                            if let Some(last) = run.last_mut() {
                                last.bulge = (delta / 4.0).tan();
                            }
                            run.push(PolyVertex::new(m.apply(*end)));
                            cur = m.apply(*end);
                            cur_user = *end;
                        }
                    } else if *rx > 0.0 && *ry > 0.0 {
                        self.flush_ctrl(&mut ctrl)?;
                        for p in flatten_arc(&m, cur_user, *end, *rx, *ry, *rot, *large, *sweep)? {
                            if run.is_empty() {
                                run.push(PolyVertex::new(cur));
                            }
                            run.push(PolyVertex::new(p));
                        }
                        cur = m.apply(*end);
                        cur_user = *end;
                    }
                }
            }
            if run.len() > MAX_CURVE_POINTS {
                return Err(IoError::Format("SVG: one path has too many vertices".into()));
            }
        }
        self.flush_run(&mut run, sp.closed && !curved)?;
        self.flush_ctrl(&mut ctrl)?;
        Ok(())
    }

    fn flush_run(&mut self, run: &mut Vec<PolyVertex>, closed: bool) -> Result<()> {
        // A closed run that comes back to its start: the repeated vertex goes, and the one
        // before it holds the closing segment's bulge.
        if closed && run.len() > 2 && run.first().zip(run.last()).is_some_and(|(a, b)| a.p.dist(b.p) <= 1e-9 * (1.0 + a.p.x.abs().max(a.p.y.abs()))) {
            run.pop();
        }
        if run.len() >= 2 {
            let vertices = std::mem::take(run);
            self.add(EntityKind::LwPolyline(LwPolyline { vertices, closed, const_width: 0.0, elevation: 0.0, plinegen: false }))?;
        } else {
            run.clear();
        }
        Ok(())
    }

    fn flush_ctrl(&mut self, ctrl: &mut Vec<Vec2>) -> Result<()> {
        if ctrl.len() >= 4 {
            let spline = Spline::from_control(std::mem::take(ctrl), 3);
            self.add(EntityKind::Spline(spline))?;
        } else {
            ctrl.clear();
        }
        Ok(())
    }
}

/// How a transform treats circles: whether it keeps them circles (a uniform scale with any
/// rotation or reflection), its scale and whether it mirrors.
struct Frame {
    uniform: bool,
    scale: f64,
    flipped: bool,
}

impl Frame {
    fn of(m: &Mat3) -> Frame {
        let ex = m.apply_vec(Vec2::new(1.0, 0.0));
        let ey = m.apply_vec(Vec2::new(0.0, 1.0));
        let mag = (ex.x * ex.x + ex.y * ex.y + ey.x * ey.x + ey.y * ey.y).max(1.0);
        let uniform =
            (ex.x * ex.x + ex.y * ex.y - (ey.x * ey.x + ey.y * ey.y)).abs() <= 1e-9 * mag && (ex.x * ey.x + ex.y * ey.y).abs() <= 1e-9 * mag;
        let det = m.det();
        Frame { uniform, scale: det.abs().sqrt(), flipped: det < 0.0 }
    }

    /// The drawing radius of an arc with radii `rx`, `ry`, when it stays circular.
    fn circular(&self, rx: f64, ry: f64) -> Option<f64> {
        let r = rx * self.scale;
        (self.uniform && self.scale > 0.0 && (rx - ry).abs() <= 1e-9 * rx.max(ry) && r > 0.0 && r.is_finite()).then_some(r)
    }
}

/// A rectangle with rounded corners as a path, clockwise on the page from the top edge.
fn rounded_rect(x: f64, y: f64, w: f64, h: f64, rx: f64, ry: f64) -> Subpath {
    let arc = |end: Vec2| Seg::Arc { rx, ry, rot: 0.0, large: false, sweep: true, end };
    let mut segs = Vec::with_capacity(8);
    let line = |segs: &mut Vec<Seg>, from: Vec2, to: Vec2| {
        if from.dist(to) > 0.0 {
            segs.push(Seg::Line(to));
        }
    };
    let start = Vec2::new(x + rx, y);
    line(&mut segs, start, Vec2::new(x + w - rx, y));
    segs.push(arc(Vec2::new(x + w, y + ry)));
    line(&mut segs, Vec2::new(x + w, y + ry), Vec2::new(x + w, y + h - ry));
    segs.push(arc(Vec2::new(x + w - rx, y + h)));
    line(&mut segs, Vec2::new(x + w - rx, y + h), Vec2::new(x + rx, y + h));
    segs.push(arc(Vec2::new(x, y + h - ry)));
    line(&mut segs, Vec2::new(x, y + h - ry), Vec2::new(x, y + ry));
    segs.push(arc(start));
    Subpath { start, segs, closed: true }
}

/// A `symbol`'s `viewBox` fitted into the referencing `use`'s `width`/`height` (centred, aspect
/// kept unless `preserveAspectRatio="none"`); without both, just the `viewBox` origin.
fn symbol_viewbox(sym: &Node, user: &Node) -> Mat3 {
    let vb: Vec<f64> = sym.attr("viewBox").map(|v| Numbers::new(v).collect()).unwrap_or_default();
    let [vx, vy, vw, vh] = vb.as_slice() else { return Mat3::IDENTITY };
    let size = |k: &str| user.attr(k).filter(|v| !v.contains('%')).and_then(parse_length_mm).filter(|v| *v > 0.0);
    match (size("width"), size("height")) {
        (Some(w), Some(h)) if *vw > 0.0 && *vh > 0.0 => {
            let (sx, sy) = if sym.attr("preserveAspectRatio").is_some_and(|p| p.trim().starts_with("none")) {
                (w / vw, h / vh)
            } else {
                let s = (w / vw).min(h / vh);
                (s, s)
            };
            let (tx, ty) = ((w - vw * sx) / 2.0 - vx * sx, (h - vh * sy) / 2.0 - vy * sy);
            Mat3 { a: sx, b: 0.0, c: 0.0, d: sy, e: tx, f: ty }
        }
        _ => Mat3::translate(Vec2::new(-vx, -vy)),
    }
}

/// An image's displayed rectangle inside its box `(x, y, w, h)` for `preserveAspectRatio`, and
/// the clip boundary (in pixels) when `slice` lets it overflow the box.
fn fit_image(x: f64, y: f64, w: f64, h: f64, pw: f64, ph: f64, par: &str) -> (f64, f64, f64, f64, Vec<Vec2>) {
    let par = par.trim();
    if par.starts_with("none") {
        return (x, y, w, h, Vec::new());
    }
    let slice = par.contains("slice");
    let s = if slice { (w / pw).max(h / ph) } else { (w / pw).min(h / ph) };
    let ax = if par.contains("xMin") {
        0.0
    } else if par.contains("xMax") {
        1.0
    } else {
        0.5
    };
    let ay = if par.contains("YMin") {
        0.0
    } else if par.contains("YMax") {
        1.0
    } else {
        0.5
    };
    let (iw, ih) = (pw * s, ph * s);
    let (ix, iy) = (x + (w - iw) * ax, y + (h - ih) * ay);
    let clip = if slice && (iw > w * (1.0 + 1e-9) || ih > h * (1.0 + 1e-9)) {
        vec![Vec2::new((x - ix) / s, (y - iy) / s), Vec2::new((x + w - ix) / s, (y + h - iy) / s)]
    } else {
        Vec::new()
    };
    (ix, iy, iw, ih, clip)
}

/// MTEXT contents: backslashes and braces are formatting codes, so literal ones are escaped.
fn mtext_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        if matches!(c, '\\' | '{' | '}') {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

fn num(n: &Node, name: &str, fallback: f64) -> f64 {
    n.attr(name).and_then(parse_length_mm).unwrap_or(fallback)
}

/// The first number of a list attribute (a text element's `x` may hold one per character).
fn first_num(n: &Node, name: &str) -> Option<f64> {
    n.attr(name).and_then(|v| Numbers::new(v).next())
}

/// A gradient coordinate: a number, or a percentage as a fraction.
fn fraction(s: &str) -> Option<f64> {
    let s = s.trim();
    let v = match s.strip_suffix('%') {
        Some(p) => p.trim().parse::<f64>().ok()? / 100.0,
        None => s.parse::<f64>().ok()?,
    };
    v.is_finite().then_some(v)
}

/// A bare number, or a number with an absolute CSS/SVG unit: millimetres here, everything
/// else (px, the default; pt, pc, in, cm) through the CSS reference pixel (96 per inch).
fn parse_length_mm(s: &str) -> Option<f64> {
    let s = s.trim();
    let cut = s.find(|c: char| !(c.is_ascii_digit() || c == '.' || c == '-' || c == '+' || c == 'e' || c == 'E')).unwrap_or(s.len());
    let (n, unit) = s.split_at(cut);
    let value: f64 = n.trim().parse().ok()?;
    if !value.is_finite() {
        return None;
    }
    match unit.trim() {
        "" | "px" => Some(value),
        "mm" => Some(value),
        "cm" => Some(value * 10.0),
        "in" => Some(value * 25.4),
        "pt" => Some(value * 25.4 / 72.0),
        "pc" => Some(value * 25.4 / 6.0),
        _ => Some(value * 25.4 / 96.0),
    }
}

/// The root mapping: `viewBox` and unit-bearing `width`/`height` become millimetres per user
/// unit, and the page's Y axis is flipped so the drawing keeps its appearance with Y up.
fn viewport(e: &Node) -> Result<Mat3> {
    let vb = e.attr("viewBox").and_then(|raw| {
        let nums: Vec<f64> = Numbers::new(raw).collect();
        match nums.as_slice() {
            [x, y, w, h] if w.is_finite() && h.is_finite() && *w > 0.0 && *h > 0.0 => Some((*x, *y, *w, *h)),
            _ => None,
        }
    });
    let w_mm = e.attr("width").and_then(parse_length_mm);
    let h_mm = e.attr("height").and_then(parse_length_mm);
    let unit = match (vb, w_mm, h_mm) {
        (Some((_, _, vw, _)), Some(w), _) if w > 0.0 => w / vw,
        (Some((_, _, _, vh)), _, Some(h)) if h > 0.0 => h / vh,
        (Some(_), _, _) => 25.4 / 96.0,
        (None, _, _) => 25.4 / 96.0,
    };
    let (vx, vy, vh) = match vb {
        Some((x, y, _, h)) => (x, y, h),
        None => (0.0, 0.0, 0.0),
    };
    Ok(Mat3 { a: unit, b: 0.0, c: 0.0, d: -unit, e: -vx * unit, f: (vy + vh) * unit })
}

/// `transform="…"`: a list of functions, applied left to right.
fn parse_transform(s: &str) -> Option<Mat3> {
    let mut out: Option<Mat3> = None;
    let mut rest = s.trim();
    while !rest.is_empty() {
        let open = rest.find('(')?;
        let name = rest[..open].trim();
        let close = rest[open..].find(')')? + open;
        let args: Vec<f64> = Numbers::new(&rest[open + 1..close]).collect();
        let m = match (name, args.as_slice()) {
            ("matrix", [a, b, c, d, e, f]) => Mat3 { a: *a, b: *b, c: *c, d: *d, e: *e, f: *f },
            ("translate", [x]) => Mat3::translate(Vec2::new(*x, 0.0)),
            ("translate", [x, y]) => Mat3::translate(Vec2::new(*x, *y)),
            ("scale", [s]) => Mat3::scale(*s, *s),
            ("scale", [sx, sy]) => Mat3::scale(*sx, *sy),
            ("rotate", [a]) => Mat3::rotate(a.to_radians()),
            ("rotate", [a, cx, cy]) => Mat3::rotate_about(Vec2::new(*cx, *cy), a.to_radians()),
            ("skewX", [a]) => Mat3 { a: 1.0, b: 0.0, c: a.to_radians().tan(), d: 1.0, e: 0.0, f: 0.0 },
            ("skewY", [a]) => Mat3 { a: 1.0, b: a.to_radians().tan(), c: 0.0, d: 1.0, e: 0.0, f: 0.0 },
            _ => return None,
        };
        out = Some(match out {
            Some(prev) => prev.then_before(m),
            None => m,
        });
        rest = rest[close + 1..].trim_start_matches(|c: char| c == ',' || c.is_whitespace());
    }
    out
}

fn sanitize_layer(raw: &str) -> String {
    let cleaned: String =
        raw.trim().chars().map(|c| if c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '$') { c } else { '_' }).take(128).collect();
    let cleaned = cleaned.trim_matches('_').to_string();
    if cleaned.is_empty() { "SVG".into() } else { cleaned }
}

/// A scanner over an SVG number list: separators are whitespace and commas, and a sign
/// starts a new number, so `10-20` is two numbers and `1e-5 2` is two numbers.
struct Numbers<'a> {
    s: &'a [u8],
    i: usize,
}

impl<'a> Numbers<'a> {
    fn new(s: &'a str) -> Self {
        Numbers { s: s.as_bytes(), i: 0 }
    }

    fn skip_seps(&mut self) {
        while matches!(self.s.get(self.i), Some(b' ' | b'\t' | b'\r' | b'\n' | b',')) {
            self.i += 1;
        }
    }

    fn next_num(&mut self) -> Option<f64> {
        self.skip_seps();
        let start = self.i;
        if matches!(self.s.get(self.i), Some(b'+' | b'-')) {
            self.i += 1;
        }
        let mut digits = 0;
        let mut dot = false;
        let mut exp = false;
        while let Some(c) = self.s.get(self.i) {
            match c {
                b'0'..=b'9' => {
                    digits += 1;
                    self.i += 1;
                }
                b'.' if !dot && !exp => {
                    dot = true;
                    self.i += 1;
                }
                b'e' | b'E' if !exp && digits > 0 => {
                    exp = true;
                    self.i += 1;
                    if matches!(self.s.get(self.i), Some(b'+' | b'-')) {
                        self.i += 1;
                    }
                }
                _ => break,
            }
        }
        if digits == 0 {
            self.i = start;
            return None;
        }
        std::str::from_utf8(&self.s[start..self.i]).ok()?.parse::<f64>().ok().filter(|v| v.is_finite())
    }
}

impl Iterator for Numbers<'_> {
    type Item = f64;
    fn next(&mut self) -> Option<f64> {
        self.next_num()
    }
}

/// One `M …` run of a `d` attribute.
struct Subpath {
    start: Vec2,
    segs: Vec<Seg>,
    closed: bool,
}

enum Seg {
    Line(Vec2),
    Cubic(Vec2, Vec2, Vec2),
    Arc { rx: f64, ry: f64, rot: f64, large: bool, sweep: bool, end: Vec2 },
}

impl Seg {
    fn end(&self) -> Vec2 {
        match self {
            Seg::Line(p) | Seg::Cubic(_, _, p) => *p,
            Seg::Arc { end, .. } => *end,
        }
    }
}

impl Subpath {
    /// Points the subpath passes through or near (its vertices, curve control points and
    /// samples along its arcs), enough for its bounds.
    fn points(&self) -> Vec<Vec2> {
        let mut out = vec![self.start];
        let mut cur = self.start;
        for seg in &self.segs {
            match seg {
                Seg::Line(p) => out.push(*p),
                Seg::Cubic(a, b, p) => out.extend([*a, *b, *p]),
                Seg::Arc { rx, ry, rot, large, sweep, end } => {
                    if let Some((c, theta, delta)) = elliptical_params(cur, *end, *rx, *ry, rot.to_radians(), *large, *sweep) {
                        let axes = Mat3::rotate(rot.to_radians());
                        out.extend((0..=16).map(|k| {
                            let t = theta + delta * f64::from(k) / 16.0;
                            c + axes.apply_vec(Vec2::new(rx * t.cos(), ry * t.sin()))
                        }));
                    }
                    out.push(*end);
                }
            }
            cur = seg.end();
        }
        out
    }
}

/// Tokens of a `d` attribute: command letters and numbers, in order.
enum Tok {
    Cmd(u8),
    Num(f64),
}

struct Tokenizer<'a> {
    s: &'a [u8],
    i: usize,
}

impl<'a> Tokenizer<'a> {
    fn new(s: &'a str) -> Self {
        Tokenizer { s: s.as_bytes(), i: 0 }
    }

    fn next_tok(&mut self) -> Option<Tok> {
        let mut nums = Numbers { s: self.s, i: self.i };
        nums.skip_seps();
        self.i = nums.i;
        let c = self.s.get(self.i).copied()?;
        if c.is_ascii_alphabetic() {
            self.i += 1;
            return Some(Tok::Cmd(c));
        }
        let tok = nums.next_num().map(Tok::Num);
        self.i = nums.i;
        tok
    }
}

/// Split `d` into subpaths, resolving relative commands and the S/T control reflections.
fn parse_path(d: &str) -> Result<Vec<Subpath>> {
    let mut tk = Tokenizer::new(d);
    let mut lookahead: Option<Tok> = None;
    let mut cmd = 0u8;
    let mut cur = Vec2::ZERO;
    let mut start = Vec2::ZERO;
    let mut segs: Vec<Seg> = Vec::new();
    let mut out: Vec<Subpath> = Vec::new();
    let closed = false;
    let mut prev_cubic: Option<Vec2> = None;
    let mut prev_quad: Option<Vec2> = None;
    let mut segments = 0usize;

    let next_num = |tk: &mut Tokenizer, look: &mut Option<Tok>| -> Result<f64> {
        match look.take() {
            Some(Tok::Num(v)) => Ok(v),
            Some(other @ Tok::Cmd(_)) => {
                *look = Some(other);
                Err(IoError::Format("SVG: path command is missing coordinates".into()))
            }
            None => match tk.next_tok() {
                Some(Tok::Num(v)) => Ok(v),
                Some(other @ Tok::Cmd(_)) => {
                    *look = Some(other);
                    Err(IoError::Format("SVG: path command is missing coordinates".into()))
                }
                None => Err(IoError::Format("SVG: path command is missing coordinates".into())),
            },
        }
    };

    while let Some(tok) = lookahead.take().or_else(|| tk.next_tok()) {
        let rel = match tok {
            Tok::Cmd(c) => {
                if c.eq_ignore_ascii_case(&b'z') {
                    if !segs.is_empty() {
                        out.push(Subpath { start, segs, closed: true });
                        segs = Vec::new();
                    }
                    cur = start;
                    cmd = 0;
                    continue;
                }
                cmd = c;
                c.is_ascii_lowercase()
            }
            tok @ Tok::Num(_) => {
                if cmd == 0 {
                    return Err(IoError::Format("SVG: path data does not start with a command".into()));
                }
                lookahead = Some(tok);
                cmd.is_ascii_lowercase()
            }
        };
        let abs = cmd.to_ascii_uppercase();
        let count = match abs {
            b'M' | b'L' => 2,
            b'H' | b'V' | b'T' => 1,
            b'C' => 6,
            b'S' | b'Q' => 4,
            b'A' => 7,
            _ => return Err(IoError::Format(format!("SVG: unknown path command `{}`", cmd as char))),
        };
        let mut args = [0.0f64; 7];
        for slot in args.iter_mut().take(count) {
            *slot = next_num(&mut tk, &mut lookahead)?;
        }
        let point = |x: f64, y: f64| if rel { cur + Vec2::new(x, y) } else { Vec2::new(x, y) };
        match abs {
            b'M' => {
                if !segs.is_empty() {
                    out.push(Subpath { start, segs, closed: false });
                    segs = Vec::new();
                }
                cur = point(args[0], args[1]);
                start = cur;
                cmd = if rel { b'l' } else { b'L' };
                prev_cubic = None;
                prev_quad = None;
            }
            b'L' => {
                cur = point(args[0], args[1]);
                segs.push(Seg::Line(cur));
                prev_cubic = None;
                prev_quad = None;
            }
            b'H' => {
                cur = Vec2::new(if rel { cur.x + args[0] } else { args[0] }, cur.y);
                segs.push(Seg::Line(cur));
                prev_cubic = None;
                prev_quad = None;
            }
            b'V' => {
                cur = Vec2::new(cur.x, if rel { cur.y + args[0] } else { args[0] });
                segs.push(Seg::Line(cur));
                prev_cubic = None;
                prev_quad = None;
            }
            b'C' => {
                let (c1, c2, p) = (point(args[0], args[1]), point(args[2], args[3]), point(args[4], args[5]));
                segs.push(Seg::Cubic(c1, c2, p));
                prev_cubic = Some(c2);
                prev_quad = None;
                cur = p;
            }
            b'S' => {
                let c1 = prev_cubic.map_or(cur, |c2| cur * 2.0 - c2);
                let (c2, p) = (point(args[0], args[1]), point(args[2], args[3]));
                segs.push(Seg::Cubic(c1, c2, p));
                prev_cubic = Some(c2);
                prev_quad = None;
                cur = p;
            }
            b'Q' => {
                let (q, p) = (point(args[0], args[1]), point(args[2], args[3]));
                segs.push(Seg::Cubic(cur + (q - cur) * (2.0 / 3.0), p + (q - p) * (2.0 / 3.0), p));
                prev_quad = Some(q);
                prev_cubic = None;
                cur = p;
            }
            b'T' => {
                let reflect = prev_quad.map_or(cur, |q| cur * 2.0 - q);
                let p = point(args[0], args[1]);
                segs.push(Seg::Cubic(cur + (reflect - cur) * (2.0 / 3.0), p + (reflect - p) * (2.0 / 3.0), p));
                prev_quad = Some(reflect);
                prev_cubic = None;
                cur = p;
            }
            b'A' => {
                let end = point(args[5], args[6]);
                segs.push(Seg::Arc { rx: args[0].abs(), ry: args[1].abs(), rot: args[2], large: args[3] != 0.0, sweep: args[4] != 0.0, end });
                cur = end;
                prev_cubic = None;
                prev_quad = None;
            }
            _ => return Err(IoError::Format("SVG: unsupported path command".into())),
        }
        segments += 1;
        if segments > MAX_PATH_SEGMENTS {
            return Err(IoError::Format("SVG: path has too many segments".into()));
        }
        if segs.len() > MAX_CURVE_POINTS {
            return Err(IoError::Format("SVG: path has too many segments".into()));
        }
    }
    if !segs.is_empty() {
        out.push(Subpath { start, segs, closed });
    }
    Ok(out)
}

/// The centre and signed sweep of a circular arc from `p0` to `p1` in a Y-up frame, per the
/// SVG endpoint parameterisation (F.6.5). Radii that cannot span the chord are scaled up,
/// as the specification requires.
fn arc_center_circular(p0: Vec2, p1: Vec2, r: f64, large: bool, sweep: bool) -> Option<(Vec2, f64)> {
    if r <= 0.0 || !r.is_finite() {
        return None;
    }
    let d = p1 - p0;
    let dist = d.x.hypot(d.y);
    let r = if dist > 2.0 * r { dist / 2.0 } else { r };
    if dist <= f64::EPSILON {
        return None;
    }
    let mid = (p0 + p1) * 0.5;
    let h = (r * r - (dist * dist) / 4.0).max(0.0).sqrt();
    let n = Vec2::new(-d.y / dist, d.x / dist);
    let sign = if large != sweep { 1.0 } else { -1.0 };
    let center = mid + n * (h * sign);
    let a0 = (p0.y - center.y).atan2(p0.x - center.x);
    let a1 = (p1.y - center.y).atan2(p1.x - center.x);
    let ccw = (a1 - a0).rem_euclid(std::f64::consts::TAU);
    Some((center, pick_sweep(ccw, large, sweep)))
}

/// The signed sweep (CCW positive) for a chord sweep `ccw` in `[0, 2π)`: the centre is
/// already chosen so that the magnitude matches `large`, so only the direction is left.
fn pick_sweep(ccw: f64, large: bool, sweep: bool) -> f64 {
    let tau = std::f64::consts::TAU;
    let eps = f64::EPSILON;
    if sweep {
        if ccw <= eps { if large { tau } else { eps } } else { ccw }
    } else if ccw >= tau - eps {
        if large { -tau } else { -eps }
    } else {
        ccw - tau
    }
}

/// Flatten a general elliptical arc (any `rx`/`ry`/rotation) from `p0` to `p1` in user
/// space, then map every sample through `m`.
fn flatten_arc(m: &Mat3, p0: Vec2, p1: Vec2, rx: f64, ry: f64, rot_deg: f64, large: bool, sweep: bool) -> Result<Vec<Vec2>> {
    let (c, theta, delta) = elliptical_params(p0, p1, rx, ry, rot_deg.to_radians(), large, sweep)
        .ok_or_else(|| IoError::Format("SVG: elliptical arc cannot be converted".into()))?;
    let axes = Mat3::rotate(rot_deg.to_radians());
    let rmax = rx.max(ry);
    let tol = 0.05_f64;
    let steps = if rmax > tol {
        let step = 2.0 * (1.0 - (tol / rmax).min(1.0)).acos().max(1e-6);
        ((delta.abs() / step).ceil() as usize).clamp(8, MAX_ARC_CHORDS)
    } else {
        8
    };
    let mut out = Vec::with_capacity(steps + 1);
    for k in 0..=steps {
        let t = theta + delta * (k as f64) / (steps as f64);
        let local = Vec2::new(rx * t.cos(), ry * t.sin());
        out.push(m.apply(c + axes.apply_vec(local)));
    }
    Ok(out)
}

/// Endpoint → centre parameterisation for a rotated elliptical arc (SVG F.6.5), in the
/// frame the points are given in. Returns the centre, the start parameter and the signed
/// sweep.
fn elliptical_params(p0: Vec2, p1: Vec2, rx: f64, ry: f64, phi: f64, large: bool, sweep: bool) -> Option<(Vec2, f64, f64)> {
    if rx <= 0.0 || ry <= 0.0 {
        return None;
    }
    let (sphi, cphi) = phi.sin_cos();
    let dx = (p0.x - p1.x) / 2.0;
    let dy = (p0.y - p1.y) / 2.0;
    let x1 = cphi * dx + sphi * dy;
    let y1 = -sphi * dx + cphi * dy;
    let (rx, ry) = {
        let lambda = x1 * x1 / (rx * rx) + y1 * y1 / (ry * ry);
        if lambda > 1.0 {
            let s = lambda.sqrt();
            (rx * s, ry * s)
        } else {
            (rx, ry)
        }
    };
    let num = rx * rx * ry * ry - rx * rx * y1 * y1 - ry * ry * x1 * x1;
    let den = rx * rx * y1 * y1 + ry * ry * x1 * x1;
    let coef = if sweep != large { 1.0 } else { -1.0 } * (num.max(0.0) / den.max(f64::MIN_POSITIVE)).sqrt();
    let cxp = coef * rx * y1 / ry;
    let cyp = -coef * ry * x1 / rx;
    let center = Vec2::new(cphi * cxp - sphi * cyp + (p0.x + p1.x) / 2.0, sphi * cxp + cphi * cyp + (p0.y + p1.y) / 2.0);
    let angle = |ux: f64, uy: f64, vx: f64, vy: f64| {
        let dot = ux * vx + uy * vy;
        let det = ux * vy - uy * vx;
        let denom = (ux.hypot(uy) * vx.hypot(vy)).max(f64::MIN_POSITIVE);
        (dot / denom).clamp(-1.0, 1.0).acos().copysign(det)
    };
    let (ux, uy) = ((x1 - cxp) / rx, (y1 - cyp) / ry);
    let (vx, vy) = ((-x1 - cxp) / rx, (-y1 - cyp) / ry);
    let theta = angle(1.0, 0.0, ux, uy);
    let ccw = angle(ux, uy, vx, vy).rem_euclid(std::f64::consts::TAU);
    Some((center, theta, pick_sweep(ccw, large, sweep)))
}
