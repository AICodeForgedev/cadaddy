//! IMAGETRACE accuracy (Q209): our own synthetic plans rendered to images by the app's own
//! rasteriser at several resolutions, then dirtied as scans are (noise, a slight rotation, JPEG
//! artefacts), placed back where the plan was and traced. What the trace gives is scored against
//! the source drawing: lines, arcs, circles, wall pairs and text boxes found or missed, how far off
//! they are, and what was traced that isn't there. The thresholds are the tests; the table is
//! printed (`cargo test --test imagetrace_accuracy -- --nocapture`). The worst cases found when
//! this was written are kept as their own named cases at the end.

use cadaddy_doc::{Arc, Circle, EntityKind, Image, Line, LwPolyline, Space, Text};
use cadaddy_engine::Session;
use cadaddy_geom::{Bounds2, PolyVertex, Vec2, Vec3};
use serde_json::json;

// ---------------- the source plans ----------------

/// What a plan is made of, in drawing units (centimetres; the plan is 10 m × 7 m).
#[derive(Clone, Debug, Default)]
struct Plan {
    lines: Vec<(Vec2, Vec2)>,
    circles: Vec<(Vec2, f64)>,
    /// Quarter arcs (door swings): centre, radius, start angle (radians).
    arcs: Vec<(Vec2, f64, f64)>,
    /// Wall centre lines and thickness.
    walls: Vec<(Vec2, Vec2, f64)>,
    /// Text: where (lower left), height, contents.
    texts: Vec<(Vec2, f64, String)>,
}

fn v(x: f64, y: f64) -> Vec2 {
    Vec2::new(x, y)
}

/// A flat with rooms, walls, columns, door swings, a diagonal and room names (made-up names).
fn flat() -> Plan {
    let mut p = Plan::default();
    // Outer walls, 25 cm, and two inner ones, 12 cm.
    for (a, b) in
        [(v(50.0, 50.0), v(950.0, 50.0)), (v(950.0, 50.0), v(950.0, 650.0)), (v(950.0, 650.0), v(50.0, 650.0)), (v(50.0, 650.0), v(50.0, 50.0))]
    {
        p.walls.push((a, b, 25.0));
    }
    p.walls.push((v(450.0, 63.0), v(450.0, 400.0), 12.0));
    p.walls.push((v(63.0, 400.0), v(700.0, 400.0), 12.0));
    // Fittings drawn in thin lines: a kitchen run, a stair's treads, a diagonal brace.
    p.lines.push((v(500.0, 120.0), v(900.0, 120.0)));
    p.lines.push((v(900.0, 120.0), v(900.0, 360.0)));
    for i in 0..6 {
        let x = 120.0 + 40.0 * f64::from(i);
        p.lines.push((v(x, 460.0), v(x, 600.0)));
    }
    p.lines.push((v(100.0, 460.0), v(340.0, 460.0)));
    p.lines.push((v(560.0, 450.0), v(880.0, 610.0)));
    // Columns and door swings.
    p.circles.push((v(700.0, 520.0), 15.0));
    p.circles.push((v(820.0, 250.0), 20.0));
    p.arcs.push((v(456.0, 300.0), 80.0, std::f64::consts::FRAC_PI_2));
    p.arcs.push((v(600.0, 406.0), 80.0, 0.0));
    // Room names.
    p.texts.push((v(150.0, 250.0), 25.0, "LIVING".into()));
    p.texts.push((v(600.0, 230.0), 25.0, "KITCHEN".into()));
    p.texts.push((v(500.0, 520.0), 18.0, "BED 1".into()));
    p
}

/// Thin lines only, at many angles: a test of straightness and angle.
fn lines_at_angles() -> Plan {
    let mut p = Plan::default();
    for i in 0..12 {
        let a = std::f64::consts::PI * f64::from(i) / 12.0 + 0.05;
        let c = v(250.0 + 500.0 * f64::from(i % 2), 175.0 + 350.0 * f64::from((i / 2) % 2));
        let d = Vec2::new(a.cos(), a.sin()) * 140.0;
        p.lines.push((c - d, c + d));
    }
    p.circles.push((v(500.0, 350.0), 60.0));
    p
}

fn drawing(p: &Plan) -> Session {
    let mut s = Session::new();
    let z = |q: Vec2| Vec3::new(q.x, q.y, 0.0);
    for (a, b) in &p.lines {
        s.add_entity(EntityKind::Line(Line { a: z(*a), b: z(*b) })).unwrap();
    }
    for (c, r) in &p.circles {
        s.add_entity(EntityKind::Circle(Circle { center: z(*c), radius: *r })).unwrap();
    }
    for (c, r, a0) in &p.arcs {
        s.add_entity(EntityKind::Arc(Arc { center: z(*c), radius: *r, start: *a0, end: a0 + std::f64::consts::FRAC_PI_2 })).unwrap();
    }
    for (a, b, t) in &p.walls {
        let vertices = vec![PolyVertex { p: *a, ..Default::default() }, PolyVertex { p: *b, ..Default::default() }];
        s.add_entity(EntityKind::LwPolyline(LwPolyline { vertices, closed: false, const_width: *t, elevation: 0.0, plinegen: false })).unwrap();
    }
    for (at, h, text) in &p.texts {
        s.add_entity(EntityKind::Text(Text {
            insert: z(*at),
            align_pt: None,
            height: *h,
            value: text.clone(),
            rotation: 0.0,
            width_factor: 1.0,
            oblique: 0.0,
            style: "Standard".into(),
            halign: Default::default(),
            valign: Default::default(),
        }))
        .unwrap();
    }
    s
}

/// The sheet the plan is printed on: 10 m × 7 m at 1:50, 200 mm × 140 mm.
const FRAME: (f64, f64, f64, f64) = (0.0, 0.0, 1000.0, 700.0);
const PAPER_MM: f64 = 200.0;

// ---------------- rendering and dirtying ----------------

/// RGBA, y down.
#[derive(Clone)]
struct Img {
    w: usize,
    h: usize,
    px: Vec<u8>,
}

/// The plan as printed at `dpi`: black on white, thin lines 0.25 mm.
fn render(s: &Session, dpi: f64) -> Img {
    let d = s.doc().unwrap();
    let w = (PAPER_MM / 25.4 * dpi).round() as u32;
    let h = (f64::from(w) * (FRAME.3 - FRAME.1) / (FRAME.2 - FRAME.0)).round() as u32;
    let frame = Bounds2::new(v(FRAME.0, FRAME.1), v(FRAME.2, FRAME.3));
    let view = cadaddy_render::raster::View::fit(&frame, w, h, 0.0);
    let list = cadaddy_render::build(d, &Space::Model, &cadaddy_render::Options { tolerance: 0.25 / view.scale, ..Default::default() });
    let o = cadaddy_render::raster::RasterOptions {
        background: cadaddy_color::Rgb(255, 255, 255),
        hairline: (0.25 / 25.4 * dpi).max(1.0) as f32,
        px_per_mm: 0.0,
        antialias: true,
    };
    let png = cadaddy_render::raster::render_png(&list, &view, &o).unwrap();
    let r = cadaddy_render::images::decode(&png).unwrap();
    Img { w: r.width as usize, h: r.height as usize, px: r.rgba.clone() }
}

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn unit(&mut self) -> f64 {
        (self.next() >> 11) as f64 / (1u64 << 53) as f64
    }
}

/// Scanner noise: every pixel jittered a little, and specks (0.3% black, 0.3% white).
fn noise(img: &mut Img, seed: u64) {
    let mut r = Rng(seed | 1);
    for px in img.px.as_chunks_mut::<4>().0 {
        let u = r.unit();
        let g = if u < 0.003 {
            0.0
        } else if u < 0.006 {
            255.0
        } else {
            f64::from(px[0]) + (r.unit() - 0.5) * 50.0
        };
        let g = g.clamp(0.0, 255.0) as u8;
        px[..3].copy_from_slice(&[g, g, g]);
    }
}

/// Turned by `deg` (counter-clockwise as seen) about the image centre, bilinear, paper outside.
fn rotate(img: &Img, deg: f64) -> Img {
    let (c, s) = (deg.to_radians().cos(), deg.to_radians().sin());
    let (cx, cy) = (img.w as f64 / 2.0, img.h as f64 / 2.0);
    let grey = |x: i64, y: i64| -> f64 {
        if x < 0 || y < 0 || x >= img.w as i64 || y >= img.h as i64 { 255.0 } else { f64::from(img.px[(y as usize * img.w + x as usize) * 4]) }
    };
    let mut out = Img { w: img.w, h: img.h, px: vec![255; img.px.len()] };
    for y in 0..img.h {
        for x in 0..img.w {
            // y down on screen: a counter-clockwise turn as seen is clockwise in these axes.
            let (dx, dy) = (x as f64 + 0.5 - cx, y as f64 + 0.5 - cy);
            let (sx, sy) = (c * dx - s * dy + cx - 0.5, s * dx + c * dy + cy - 0.5);
            let (x0, y0) = (sx.floor() as i64, sy.floor() as i64);
            let (fx, fy) = (sx - x0 as f64, sy - y0 as f64);
            let g = grey(x0, y0) * (1.0 - fx) * (1.0 - fy)
                + grey(x0 + 1, y0) * fx * (1.0 - fy)
                + grey(x0, y0 + 1) * (1.0 - fx) * fy
                + grey(x0 + 1, y0 + 1) * fx * fy;
            let g = g.round().clamp(0.0, 255.0) as u8;
            out.px[(y * img.w + x) * 4..][..3].copy_from_slice(&[g, g, g]);
        }
    }
    out
}

/// Saved as a JPEG at `quality` and read back.
fn jpeg(img: &Img, quality: u8) -> Img {
    let rgb: Vec<u8> = img.px.as_chunks::<4>().0.iter().flat_map(|p| [p[0], p[1], p[2]]).collect();
    let mut buf = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut buf, quality)
        .encode(&rgb, img.w as u32, img.h as u32, image::ExtendedColorType::Rgb8)
        .unwrap();
    let back = image::load_from_memory(&buf).unwrap().to_rgba8();
    Img { w: img.w, h: img.h, px: back.into_raw() }
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Dirt {
    Clean,
    Noise,
    Rotated,
    Jpeg,
    All,
}

// ---------------- tracing ----------------

#[derive(Default, Debug)]
struct Traced {
    lines: Vec<(Vec2, Vec2)>,
    circles: Vec<(Vec2, f64)>,
    /// Centre, radius and sweep (radians).
    arcs: Vec<(Vec2, f64, f64)>,
    wall_faces: Vec<(Vec2, Vec2)>,
    boxes: Vec<Bounds2>,
}

/// Trace `img`, placed so its pixels land where the plan was drawn (undoing the `deg` turn).
fn trace(img: &Img, deg: f64) -> Traced {
    trace_with(img, deg, json!({}))
}

fn trace_with(img: &Img, deg: f64, params: serde_json::Value) -> Traced {
    trace_in(img, deg, FRAME, params)
}

/// Trace `img` placed over `frame` (drawing units), the frame it was rendered from.
fn trace_in(img: &Img, deg: f64, frame: (f64, f64, f64, f64), params: serde_json::Value) -> Traced {
    let path = format!("trace-accuracy-{}-{}.png", std::process::id(), TRACE_ID.fetch_add(1, std::sync::atomic::Ordering::SeqCst));
    cadaddy_render::images::insert(&path, cadaddy_render::images::Raster::new(img.w as u32, img.h as u32, img.px.clone()).unwrap());
    let k = (frame.2 - frame.0) / img.w as f64;
    // Image pixels (y up from the lower left) to the drawing: centre to centre, turned back.
    let (c, s) = ((-deg).to_radians().cos(), (-deg).to_radians().sin());
    let turn = |p: Vec2| Vec2::new(c * p.x - s * p.y, s * p.x + c * p.y);
    let centre_px = v(img.w as f64 / 2.0, img.h as f64 / 2.0);
    let centre = v((frame.0 + frame.2) / 2.0, (frame.1 + frame.3) / 2.0);
    let u = turn(v(k, 0.0));
    let vv = turn(v(0.0, k));
    let o = centre + turn(-centre_px) * k;
    let mut sess = Session::new();
    sess.add_entity(EntityKind::Image(Box::new(Image {
        insert: Vec3::new(o.x, o.y, 0.0),
        u: Vec3::new(u.x, u.y, 0.0),
        v: Vec3::new(vv.x, vv.y, 0.0),
        size: v(img.w as f64, img.h as f64),
        path: path.clone(),
        ..Image::default()
    })))
    .unwrap();
    sess.execute("imagetrace", &params).unwrap();
    let mut t = Traced::default();
    for e in sess.doc().unwrap().model.iter() {
        match (&e.kind, e.common.layer.as_str()) {
            (EntityKind::Line(l), "TRACED") => t.lines.push((l.a.xy(), l.b.xy())),
            (EntityKind::Line(l), "TRACED-WALLS") => t.wall_faces.push((l.a.xy(), l.b.xy())),
            (EntityKind::Circle(c), _) => t.circles.push((c.center.xy(), c.radius)),
            (EntityKind::Arc(a), _) => t.arcs.push((a.center.xy(), a.radius, (a.end - a.start).rem_euclid(std::f64::consts::TAU))),
            (EntityKind::LwPolyline(p), "TRACED-TEXT") => t.boxes.push(Bounds2::from_points(p.vertices.iter().map(|q| q.p))),
            _ => {}
        }
    }
    cadaddy_render::images::forget(&path);
    t
}

static TRACE_ID: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

// ---------------- scoring ----------------

fn seg_dist(p: Vec2, a: Vec2, b: Vec2) -> f64 {
    let ab = b - a;
    let l2 = ab.dot(ab);
    let t = if l2 > 0.0 { ((p - a).dot(ab) / l2).clamp(0.0, 1.0) } else { 0.0 };
    p.dist(a + ab * t)
}

fn angle_between(a: (Vec2, Vec2), b: (Vec2, Vec2)) -> f64 {
    let (da, db) = (a.1 - a.0, b.1 - b.0);
    let x = (da.cross(db).abs()).atan2(da.dot(db).abs());
    x.to_degrees()
}

/// How well a source segment is covered by traced segments within `tol` (at most 5° apart):
/// the covered fraction and the mean distance of its covered samples.
fn coverage(src: (Vec2, Vec2), traced: &[(Vec2, Vec2)], tol: f64) -> (f64, f64) {
    let near: Vec<&(Vec2, Vec2)> = traced.iter().filter(|t| angle_between(src, **t) <= 5.0).collect();
    let n = ((src.1 - src.0).len() / (tol / 2.0)).ceil().max(2.0) as usize;
    let (mut hit, mut sum) = (0usize, 0.0);
    for i in 0..=n {
        let p = src.0 + (src.1 - src.0) * (i as f64 / n as f64);
        let d = near.iter().map(|t| seg_dist(p, t.0, t.1)).fold(f64::INFINITY, f64::min);
        if d <= tol {
            hit += 1;
            sum += d;
        }
    }
    (hit as f64 / (n + 1) as f64, if hit > 0 { sum / hit as f64 } else { f64::NAN })
}

#[derive(Clone, Debug, Default)]
struct Score {
    lines: (usize, usize),
    line_err: f64,
    line_len_err: f64,
    extra_lines: usize,
    circles: (usize, usize),
    circle_err: f64,
    extra_circles: usize,
    arcs: (usize, usize),
    extra_arcs: usize,
    walls: (usize, usize),
    wall_err: f64,
    extra_walls: usize,
    texts: (usize, usize),
    extra_texts: usize,
}

impl Score {
    fn rate(f: (usize, usize)) -> f64 {
        if f.1 == 0 { 1.0 } else { f.0 as f64 / f.1 as f64 }
    }
}

/// Total length of traced segments within `tol` and ≤5° of `src`.
fn matched_traced_length(src: (Vec2, Vec2), traced: &[(Vec2, Vec2)], tol: f64) -> f64 {
    let near: Vec<&(Vec2, Vec2)> = traced.iter().filter(|t| angle_between(src, **t) <= 5.0).collect();
    let mut total = 0.0;
    for t in &near {
        // Clip traced segment to the projection along the source, within tol.
        let ab = src.1 - src.0;
        let l2 = ab.dot(ab);
        if l2 < 1e-12 {
            continue;
        }
        let project = |p: Vec2| (p - src.0).dot(ab) / l2;
        let (t0, t1) = (project(t.0), project(t.1));
        // The traced segment's endpoints must be within tol of the source line.
        if seg_dist(t.0, src.0, src.1) > tol && seg_dist(t.1, src.0, src.1) > tol {
            continue;
        }
        let len_along = (t1 - t0).abs() * ab.len();
        total += len_along;
    }
    total
}

fn score(p: &Plan, t: &Traced, px_per_unit: f64) -> Score {
    // Two pixels, or a millimetre on paper, whichever is more.
    let tol = (2.0 / px_per_unit).max(5.0);
    let mut s = Score::default();
    // Lines: found when 80% of their length is traced.
    let mut errs = Vec::new();
    let mut len_errs = Vec::new();
    for l in &p.lines {
        let (frac, err) = coverage(*l, &t.lines, tol);
        if frac >= 0.8 {
            s.lines.0 += 1;
            errs.push(err);
            // Length error: |traced matched length - source length| in pixels.
            let src_len = (l.1 - l.0).len();
            let traced_len = matched_traced_length(*l, &t.lines, tol);
            len_errs.push((traced_len - src_len).abs() * px_per_unit);
        }
    }
    s.lines.1 = p.lines.len();
    s.line_err = mean(&errs) * px_per_unit;
    s.line_len_err = mean(&len_errs);
    // Extra lines: traced where the source has nothing. A traced line is on the drawing only
    // when both its ends and its middle lie on a source item; any of the three off it is extra.
    let on_source = |q: Vec2| {
        p.lines.iter().any(|l| seg_dist(q, l.0, l.1) <= tol)
            || p.walls.iter().any(|w| seg_dist(q, w.0, w.1) <= w.2 / 2.0 + tol)
            || p.circles.iter().any(|c| (q.dist(c.0) - c.1).abs() <= tol)
            || p.arcs.iter().any(|a| (q.dist(a.0) - a.1).abs() <= tol)
            || p.texts.iter().any(|x| Bounds2::new(x.0, x.0 + v(x.1 * x.2.len() as f64, x.1)).expand(tol).contains(q))
    };
    s.extra_lines = t.lines.iter().filter(|l| !on_source(l.0) || !on_source(l.1) || !on_source((l.0 + l.1) * 0.5)).count();
    // Circles and arcs: centre and radius within the tolerance.
    let mut cerr = Vec::new();
    // A circle broken by a speck may come back as an arc of most of it (three quarters or more).
    let whole = t.circles.iter().copied().chain(t.arcs.iter().filter(|a| a.2 >= 1.5 * std::f64::consts::PI).map(|a| (a.0, a.1)));
    let whole: Vec<(Vec2, f64)> = whole.collect();
    let mut matched_circles: Vec<bool> = vec![false; whole.len()];
    for c in &p.circles {
        if let Some((idx, e)) = whole
            .iter()
            .enumerate()
            .map(|(i, q)| (i, q.0.dist(c.0).max((q.1 - c.1).abs())))
            .filter(|(_, e)| *e <= tol)
            .min_by(|a, b| a.1.total_cmp(&b.1))
        {
            s.circles.0 += 1;
            cerr.push(e);
            matched_circles[idx] = true;
        }
    }
    s.circles.1 = p.circles.len();
    s.circle_err = if cerr.is_empty() { f64::NAN } else { mean(&cerr) * px_per_unit };
    // Extra circles: traced "whole" items (circles + big arcs) not matching any source circle
    // or source arc.
    s.extra_circles = whole
        .iter()
        .enumerate()
        .filter(|(i, _)| !matched_circles[*i])
        .filter(|(_, c)| !p.arcs.iter().any(|a| c.0.dist(a.0) <= tol * 2.0 && (c.1 - a.1).abs() <= tol * 2.0))
        .count();
    for a in &p.arcs {
        // A door swing may come back as an arc, or as short lines along it.
        let as_arc = t.arcs.iter().any(|q| q.0.dist(a.0) <= tol * 2.0 && (q.1 - a.1).abs() <= tol * 2.0);
        if as_arc {
            s.arcs.0 += 1;
        }
    }
    s.arcs.1 = p.arcs.len();
    // Extra arcs: traced arcs (not already counted as whole circles) that match no source arc
    // and are not fragments of a source circle.
    let big_arc_sweep = 1.5 * std::f64::consts::PI;
    s.extra_arcs = t
        .arcs
        .iter()
        .filter(|ta| {
            // Skip arcs that count as whole circles (already checked above).
            if ta.2 >= big_arc_sweep {
                return false;
            }
            let near_src_arc = p.arcs.iter().any(|a| ta.0.dist(a.0) <= tol * 2.0 && (ta.1 - a.1).abs() <= tol * 2.0);
            let near_src_circle = p.circles.iter().any(|c| ta.0.dist(c.0) <= tol * 2.0 && (ta.1 - c.1).abs() <= tol * 2.0);
            !near_src_arc && !near_src_circle
        })
        .count();
    // Walls: both faces (half the thickness each side) traced along 80% of the wall.
    let mut werr = Vec::new();
    let mut all_src_faces: Vec<(Vec2, Vec2)> = Vec::new();
    for (a, b, th) in &p.walls {
        let d = (*b - *a).normalized();
        let n = Vec2::new(-d.y, d.x) * (th / 2.0);
        // The faces stop short of the corners where other walls meet them.
        let (a, b) = (*a + d * *th, *b - d * *th);
        let faces = [(a + n, b + n), (a - n, b - n)];
        all_src_faces.extend_from_slice(&faces);
        let cov: Vec<(f64, f64)> = faces.iter().map(|f| coverage(*f, &t.wall_faces, tol)).collect();
        if cov.iter().all(|c| c.0 >= 0.8) {
            s.walls.0 += 1;
            werr.extend(cov.iter().map(|c| c.1));
        }
    }
    s.walls.1 = p.walls.len();
    s.wall_err = if werr.is_empty() { f64::NAN } else { mean(&werr) * px_per_unit };
    // Extra wall faces: traced wall faces whose midpoint is far from every source wall's centre
    // line (midpoint-only: corner fragments at T-joints are near a source wall even if their
    // endpoints overshoot).
    s.extra_walls = t
        .wall_faces
        .iter()
        .filter(|wf| {
            let mid = (wf.0 + wf.1) * 0.5;
            !p.walls.iter().any(|(a, b, th)| seg_dist(mid, *a, *b) <= th / 2.0 + tol)
        })
        .count();
    // Text: a traced box overlapping the text's own box by at least a third.
    let mut matched_texts: Vec<bool> = vec![false; t.boxes.len()];
    for (at, h, text) in &p.texts {
        let src = Bounds2::new(*at, *at + v(h * 0.9 * text.len() as f64, *h));
        if let Some(idx) =
            t.boxes.iter().enumerate().filter(|(_, b)| iou(&src, b) >= 0.33).max_by(|a, b| iou(&src, a.1).total_cmp(&iou(&src, b.1))).map(|(i, _)| i)
        {
            s.texts.0 += 1;
            matched_texts[idx] = true;
        }
    }
    s.texts.1 = p.texts.len();
    // Extra text boxes: traced boxes not matching any source text.
    s.extra_texts = matched_texts.iter().filter(|m| !**m).count();
    s
}

fn mean(v: &[f64]) -> f64 {
    if v.is_empty() { 0.0 } else { v.iter().sum::<f64>() / v.len() as f64 }
}

fn iou(a: &Bounds2, b: &Bounds2) -> f64 {
    let i = a.intersection(b);
    if i.is_empty() {
        return 0.0;
    }
    let area = |r: &Bounds2| r.width() * r.height();
    area(&i) / (area(a) + area(b) - area(&i))
}

// ---------------- the matrix ----------------

fn run(plan: &Plan, dpi: f64, dirt: Dirt) -> Score {
    let s = drawing(plan);
    let mut img = render(&s, dpi);
    let mut deg = 0.0;
    if matches!(dirt, Dirt::Rotated | Dirt::All) {
        deg = 1.5;
        img = rotate(&img, deg);
    }
    if matches!(dirt, Dirt::Noise | Dirt::All) {
        noise(&mut img, 0x5EED ^ dpi as u64);
    }
    if matches!(dirt, Dirt::Jpeg | Dirt::All) {
        img = jpeg(&img, 40);
    }
    let t = trace(&img, deg);
    score(plan, &t, img.w as f64 / (FRAME.2 - FRAME.0))
}

fn err_str(e: f64) -> String {
    if e.is_nan() { "-".to_string() } else { format!("{e:.2}") }
}

fn row(name: &str, dpi: f64, dirt: Dirt, s: &Score) -> String {
    let f = |x: (usize, usize)| format!("{}/{}", x.0, x.1);
    format!(
        "| {name} | {dpi} | {dirt:?} | {} | {} | {} | {} | {} | {} | {} | {} | {} | {} | {} | {} | {} | {} |",
        f(s.lines),
        err_str(s.line_err),
        err_str(s.line_len_err),
        s.extra_lines,
        f(s.circles),
        err_str(s.circle_err),
        s.extra_circles,
        f(s.arcs),
        s.extra_arcs,
        f(s.walls),
        err_str(s.wall_err),
        s.extra_walls,
        f(s.texts),
        s.extra_texts,
    )
}

const HEADER: &str = "| plan | dpi | dirt | lines | line err px | line len err px | extra lines | circles | circle err px | extra circles | arcs | extra arcs | walls | wall err px | extra walls | text | extra texts |\n|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|";

#[test]
fn score_table_and_thresholds() {
    let mut table = vec![HEADER.to_string()];
    let mut worst: Vec<(f64, String)> = Vec::new();
    for (name, plan) in [("flat", flat()), ("angles", lines_at_angles())] {
        for dpi in [100.0, 200.0, 300.0] {
            for dirt in [Dirt::Clean, Dirt::Noise, Dirt::Rotated, Dirt::Jpeg, Dirt::All] {
                let s = run(&plan, dpi, dirt);
                table.push(row(name, dpi, dirt, &s));
                worst.push((Score::rate(s.lines) + Score::rate(s.walls) + Score::rate(s.circles), format!("{name} {dpi} {dirt:?}")));
                check(name, dpi, dirt, &s);
            }
        }
    }
    worst.sort_by(|a, b| a.0.total_cmp(&b.0));
    eprintln!("{}\n\nworst: {:?}", table.join("\n"), worst.iter().take(4).collect::<Vec<_>>());
}

/// Measured on this bench (the score table): invented circles, arcs and text boxes at most, and
/// circles and arcs found at least, per case: (plan, dpi, dirt, extra circles, extra arcs, extra
/// texts, circles, arcs). The invented circles and arcs are fragments: a line through a column
/// leaves a piece of the column as an arc, and at 300 dpi the clean column's outline traces as a
/// circle and an arc of it (at most one of each); noise at 100 dpi groups into text boxes.
const MEASURED: &[(&str, u32, Dirt, usize, usize, usize, usize, usize)] = &[
    ("flat", 100, Dirt::Clean, 0, 0, 0, 2, 2),
    ("flat", 100, Dirt::Noise, 0, 0, 3, 0, 1),
    ("flat", 100, Dirt::Rotated, 0, 0, 15, 0, 0),
    ("flat", 100, Dirt::Jpeg, 0, 0, 1, 1, 2),
    ("flat", 100, Dirt::All, 0, 0, 19, 0, 0),
    ("flat", 200, Dirt::Clean, 0, 0, 0, 2, 2),
    ("flat", 200, Dirt::Noise, 0, 0, 0, 2, 2),
    ("flat", 200, Dirt::Rotated, 0, 1, 0, 2, 2),
    ("flat", 200, Dirt::Jpeg, 0, 0, 0, 2, 2),
    ("flat", 200, Dirt::All, 1, 0, 0, 2, 2),
    ("flat", 300, Dirt::Clean, 1, 1, 0, 2, 2),
    ("flat", 300, Dirt::Noise, 0, 2, 0, 2, 2),
    ("flat", 300, Dirt::Rotated, 1, 0, 0, 2, 2),
    ("flat", 300, Dirt::Jpeg, 1, 1, 0, 2, 2),
    ("flat", 300, Dirt::All, 0, 1, 0, 2, 2),
    ("angles", 100, Dirt::Clean, 1, 0, 0, 1, 0),
    ("angles", 100, Dirt::Noise, 0, 0, 0, 1, 0),
    ("angles", 100, Dirt::Rotated, 1, 0, 0, 1, 0),
    ("angles", 100, Dirt::Jpeg, 1, 0, 0, 1, 0),
    ("angles", 100, Dirt::All, 0, 0, 0, 1, 0),
    ("angles", 200, Dirt::Clean, 0, 0, 0, 1, 0),
    ("angles", 200, Dirt::Noise, 1, 0, 0, 1, 0),
    ("angles", 200, Dirt::Rotated, 1, 0, 0, 1, 0),
    ("angles", 200, Dirt::Jpeg, 0, 0, 0, 1, 0),
    ("angles", 200, Dirt::All, 0, 0, 1, 1, 0),
    ("angles", 300, Dirt::Clean, 1, 0, 0, 1, 0),
    ("angles", 300, Dirt::Noise, 0, 0, 0, 1, 0),
    ("angles", 300, Dirt::Rotated, 1, 0, 0, 1, 0),
    ("angles", 300, Dirt::Jpeg, 1, 0, 0, 1, 0),
    ("angles", 300, Dirt::All, 0, 0, 0, 1, 0),
];

/// The thresholds every case must meet. At 200 dpi and finer (a 1:50 plan's 30 cm column is 47
/// pixels across) nearly everything is found, whatever the dirt; at 100 dpi (12 pixels) thin
/// lines and small circles can break up once the scan is turned or dirty.
fn check(name: &str, dpi: f64, dirt: Dirt, s: &Score) {
    let case = format!("{name} {dpi} dpi {dirt:?}: {s:?}");
    let fine = dpi >= 200.0;
    // Lines: found, where they belong, and nothing made up.
    assert!(Score::rate(s.lines) >= if fine { 1.0 } else { 0.7 }, "lines found, {case}");
    if s.lines.0 > 0 {
        assert!(!s.line_err.is_nan(), "line_err NaN with found lines, {case}");
        assert!(s.line_err <= 1.0, "line position error in pixels, {case}");
        assert!(!s.line_len_err.is_nan(), "line_len_err NaN with found lines, {case}");
        // On clean scans, traced lines closely match source lengths; on dirty/rotated scans,
        // broken lines and rotation clipping cause larger errors. Measured worst: ~28 px
        // (flat 100 dpi All, 280 du lines broken by rotation+noise+JPEG).
        let len_limit = if dirt == Dirt::Clean { 8.0 } else { 30.0 };
        assert!(s.line_len_err <= len_limit, "line length error in pixels, {case}");
    }
    assert_eq!(s.extra_lines, 0, "lines traced where the plan has none, {case}");
    // Walls: both faces of every wall, within a pixel. Error must not be vacuously 0.
    assert_eq!(s.walls.0, s.walls.1, "walls found, {case}");
    if s.walls.0 > 0 {
        assert!(!s.wall_err.is_nan(), "wall_err NaN with found walls, {case}");
        assert!(s.wall_err <= 1.0, "wall face error in pixels, {case}");
    }
    assert_eq!(s.extra_walls, 0, "extra wall faces, {case}");
    // Text: every label boxed, no invented boxes on clean scans. On dirty scans, noise specks
    // may form small clusters the tracer groups as text.
    assert_eq!(s.texts.0, s.texts.1, "text boxes, {case}");
    if dirt == Dirt::Clean {
        assert_eq!(s.extra_texts, 0, "extra text boxes on clean scan, {case}");
    }
    // Circles (or arcs of three quarters of them): all on clean scans; all on dirty ≥200 dpi
    // (measured: every case finds all circles); at 100 dpi dirty, at least as measured (below).
    if dirt == Dirt::Clean {
        assert_eq!(s.circles.0, s.circles.1, "circles found, {case}");
        assert_eq!(s.arcs.0, s.arcs.1, "arcs found, {case}");
    } else if fine {
        assert_eq!(s.circles.0, s.circles.1, "circles found (fine dirty), {case}");
        assert_eq!(s.arcs.0, s.arcs.1, "arcs found (fine dirty), {case}");
    }
    if s.circles.0 > 0 {
        assert!(!s.circle_err.is_nan(), "circle_err NaN with found circles, {case}");
        assert!(s.circle_err <= if dirt == Dirt::Clean { 1.0 } else { 3.0 }, "circle error in pixels, {case}");
    } else if s.circles.1 > 0 {
        // If we expect circles but found none, circle_err must be NaN, not 0.
        assert!(s.circle_err.is_nan(), "circle_err should be NaN when no circles found, {case}");
    }
    // Invented circles, arcs and text boxes at most as measured (none where none were), and at
    // 100 dpi on a dirty scan at least the circles and arcs measured.
    let &(.., xc, xa, xt, circles, arcs) =
        MEASURED.iter().find(|m| m.0 == name && f64::from(m.1) == dpi && m.2 == dirt).unwrap_or_else(|| panic!("no measured row, {case}"));
    assert!(s.extra_circles <= xc, "extra circles (measured {xc}), {case}");
    assert!(s.extra_arcs <= xa, "extra arcs (measured {xa}), {case}");
    assert!(s.extra_texts <= xt, "extra text boxes (measured {xt}), {case}");
    if !fine && dirt != Dirt::Clean {
        assert!(s.circles.0 >= circles, "circles found at 100 dpi dirty (measured {circles}), {case}");
        assert!(s.arcs.0 >= arcs, "arcs found at 100 dpi dirty (measured {arcs}), {case}");
    }
}

// ---------------- regression fixtures ----------------
// The worst cases when this was written, and the faults they showed, each kept as its own case.

/// A turned scan: walls were snapped to the image's axes instead of the plan's, so 5 of 6 walls
/// came back skewed against the lines that weren't snapped (walls 1/6, line error 2.7 px).
#[test]
fn a_turned_scan_keeps_its_walls_straight_and_joined() {
    for dpi in [200.0, 300.0] {
        let s = run(&flat(), dpi, Dirt::Rotated);
        assert_eq!(s.walls, (6, 6), "{dpi}: {s:?}");
        assert!(s.line_err <= 0.6 && s.wall_err <= 0.6, "{dpi}: {s:?}");
    }
}

/// A grid line through a column: thinning left a 2 × 2 block at the shallow crossing that wasn't
/// taken for a junction, so line and circle came back as one chain of 14 short lines.
#[test]
fn a_column_crossed_by_a_line_is_still_a_circle() {
    let s = run(&flat(), 200.0, Dirt::Clean);
    assert_eq!(s.circles, (2, 2), "{s:?}");
    assert!(s.circle_err <= 0.6, "{s:?}");
}

/// Room names 5 mm tall on paper were taller than the text limit (2.5% of the image's short
/// side) and were traced as about a hundred short lines.
#[test]
fn room_names_are_text_boxes_not_lines() {
    for dpi in [100.0, 200.0, 300.0] {
        let s = run(&flat(), dpi, Dirt::Clean);
        assert_eq!(s.texts, (3, 3), "{dpi}: {s:?}");
        assert_eq!(s.extra_lines, 0, "{dpi}: {s:?}");
    }
}

/// Scanner specks inside solid walls made rings of them: 2 of 6 walls found at 300 dpi.
#[test]
fn specks_inside_walls_do_not_break_them() {
    let s = run(&flat(), 300.0, Dirt::Noise);
    assert_eq!(s.walls, (6, 6), "{s:?}");
}

/// The worst still: 100 dpi, turned 1.5°, noisy and saved as JPEG. Thin lines break into pieces
/// and the small columns are lost; walls and text survive. A floor, not a goal.
#[test]
fn the_worst_case_floor() {
    let s = run(&flat(), 100.0, Dirt::All);
    assert!(s.lines.0 >= 7, "{s:?}");
    assert_eq!(s.walls, (6, 6), "{s:?}");
    assert_eq!(s.texts, (3, 3), "{s:?}");
}

/// A row of four device symbols on a full-size sheet (36 × 24 in at 200 dpi: 4800 px on its
/// short side), about 75 px on centre: traced as circles, not boxed as text. Ordinary letters
/// there were allowed up to 2.5% of the short side (120 px), so a lone symbol was a letter. Two
/// symbol sizes: 40 px and 32 px outlines (5 and 4 mm).
#[test]
fn fixture_symbols_on_a_full_size_sheet_are_not_text() {
    // A strip with the sheet's short side; drawing units are pixels.
    let (strip_w, strip_h) = (4900.0, 4800.0);
    for diameter in [40.0, 32.0] {
        let mut plan = Plan::default();
        let (cx, cy) = (strip_w / 2.0, strip_h / 2.0);
        for i in 0..4 {
            plan.circles.push((v(cx - 112.5 + 75.0 * f64::from(i), cy), diameter / 2.0));
        }
        let sess = drawing(&plan);
        let d = sess.doc().unwrap();
        let frame = Bounds2::new(v(0.0, 0.0), v(strip_w, strip_h));
        let view = cadaddy_render::raster::View::fit(&frame, strip_w as u32, strip_h as u32, 0.0);
        let list = cadaddy_render::build(d, &Space::Model, &cadaddy_render::Options { tolerance: 0.25 / view.scale, ..Default::default() });
        // The sheet's fine pen: 0.25 mm at 200 dpi.
        let o =
            cadaddy_render::raster::RasterOptions { background: cadaddy_color::Rgb(255, 255, 255), hairline: 2.0, px_per_mm: 0.0, antialias: true };
        let png = cadaddy_render::raster::render_png(&list, &view, &o).unwrap();
        let r_img = cadaddy_render::images::decode(&png).unwrap();
        let img = Img { w: r_img.width as usize, h: r_img.height as usize, px: r_img.rgba.clone() };
        // Placed over the frame it was rendered from, so the plan and the trace share units.
        let t = trace_in(&img, 0.0, (0.0, 0.0, strip_w, strip_h), json!({}));
        let s = score(&plan, &t, img.w as f64 / strip_w);
        assert_eq!(s.circles, (4, 4), "{diameter} px: {s:?} {:?} {:?}", t.circles, t.boxes);
        assert_eq!(s.extra_texts, 0, "{diameter} px symbols boxed as text: {:?}", t.boxes);
        assert_eq!(s.extra_circles, 0, "{diameter} px: {s:?}");
    }
}

/// A title 8 mm tall on a 300 dpi print (about 95 px) is still boxed as text, not traced as
/// strokes: the letter limits follow the scan's scale.
#[test]
fn a_title_8_mm_tall_at_300_dpi_is_text() {
    // The sheet is 1000 units across 200 mm of paper: 8 mm is 40 units (95 px at 300 dpi).
    let mut plan = Plan::default();
    plan.texts.push((v(250.0, 330.0), 40.0, "SITE PLAN".to_string()));
    let s = run(&plan, 300.0, Dirt::Clean);
    assert_eq!(s.texts, (plan.texts.len(), plan.texts.len()), "{s:?}");
    assert_eq!(s.extra_texts, 0, "{s:?}");
    assert_eq!(s.extra_lines, 0, "{s:?}");
}

/// A deliberately invented circle/wall/text box in a unit-level score must fail (not pass
/// vacuously). This proves the extras columns catch invented shapes.
#[test]
fn invented_shapes_are_caught_by_scoring() {
    let p = Plan::default(); // empty plan
    let mut t = Traced::default();
    // An invented circle.
    t.circles.push((v(100.0, 100.0), 30.0));
    let s = score(&p, &t, 1.0);
    assert_eq!(s.extra_circles, 1, "invented circle must be extra: {s:?}");
    // An invented wall face.
    let mut t2 = Traced::default();
    t2.wall_faces.push((v(0.0, 0.0), v(100.0, 0.0)));
    let s2 = score(&p, &t2, 1.0);
    assert_eq!(s2.extra_walls, 1, "invented wall face must be extra: {s2:?}");
    // An invented text box.
    let mut t3 = Traced::default();
    t3.boxes.push(Bounds2::new(v(0.0, 0.0), v(50.0, 20.0)));
    let s3 = score(&p, &t3, 1.0);
    assert_eq!(s3.extra_texts, 1, "invented text box must be extra: {s3:?}");
    // A traced line crossing a source line at 45°: its middle is on it, its ends are not.
    let mut p4 = Plan::default();
    p4.lines.push((v(0.0, 0.0), v(200.0, 0.0)));
    let mut t4 = Traced::default();
    t4.lines.push((v(50.0, -50.0), v(150.0, 50.0)));
    let s4 = score(&p4, &t4, 1.0);
    assert_eq!(s4.extra_lines, 1, "a wrong-angle line across a source line is extra: {s4:?}");
    // The same line traced where it is: not extra.
    let mut t5 = Traced::default();
    t5.lines.push((v(0.0, 0.0), v(200.0, 0.0)));
    assert_eq!(score(&p4, &t5, 1.0).extra_lines, 0);
    // An empty-match error metric must be NaN, not 0.
    assert!(s.circle_err.is_nan(), "circle_err must be NaN when no circles found: {}", s.circle_err);
    assert!(s2.wall_err.is_nan(), "wall_err must be NaN when no walls found: {}", s2.wall_err);
}
