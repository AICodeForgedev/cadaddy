//! What a sheet is drawn with: shapes in paper inches on named layers, kept until the sheet is
//! placed on its layout (so the views are worked out from the model without holding the session).

use std::collections::BTreeMap;

use cadaddy_doc::{Common, Drawing, EntityKind, RawTag, Space, Table};
use cadaddy_geom::{PolyVertex, Vec2, Vec3};

/// Most shapes one sheet holds (a hostile model can't make a sheet without end).
pub(super) const MAX_SHAPES: usize = 400_000;
/// Longest text drawn.
const MAX_TEXT: usize = 240;

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum Align {
    Left,
    Center,
}

#[derive(Clone, Debug)]
pub(super) enum Shape {
    Line([f64; 2], [f64; 2]),
    /// Vertices with the bulge to the next one.
    Pline(Vec<([f64; 2], f64)>, bool),
    Text {
        at: [f64; 2],
        text: String,
        h: f64,
        align: Align,
        rot: f64,
    },
    Circle([f64; 2], f64),
    /// Centre, radius, start and end angles (radians, counter-clockwise).
    Arc([f64; 2], f64, f64, f64),
    /// A linear dimension: the two points, where its line goes, its text, vertical or not.
    Dim {
        a: [f64; 2],
        b: [f64; 2],
        at: [f64; 2],
        text: String,
        vertical: bool,
    },
    Table(Box<Table>),
}

/// The shapes of one sheet, by layer, and what was drawn (`walls`, `door_tags`, …).
#[derive(Default)]
pub(super) struct Pen {
    pub shapes: Vec<(String, Shape)>,
    pub counts: BTreeMap<String, usize>,
    layer: String,
    pub full: bool,
}

pub(super) fn ok(p: [f64; 2]) -> bool {
    p[0].is_finite() && p[1].is_finite()
}

fn clean(t: &str) -> String {
    t.chars().filter(|c| !c.is_control()).take(MAX_TEXT).collect::<String>().trim().to_string()
}

impl Pen {
    pub fn layer(&mut self, name: &str) {
        name.clone_into(&mut self.layer);
    }
    pub fn count(&mut self, what: &str) {
        *self.counts.entry(what.to_string()).or_default() += 1;
    }
    fn push(&mut self, s: Shape) {
        if self.shapes.len() >= MAX_SHAPES {
            self.full = true;
            return;
        }
        let layer = if self.layer.is_empty() { "0".to_string() } else { self.layer.clone() };
        self.shapes.push((layer, s));
    }
    pub fn line(&mut self, a: [f64; 2], b: [f64; 2]) {
        if ok(a) && ok(b) && (a[0] - b[0]).hypot(a[1] - b[1]) > 1e-6 {
            self.push(Shape::Line(a, b));
        }
    }
    pub fn pline(&mut self, pts: &[[f64; 2]], closed: bool) {
        let v: Vec<([f64; 2], f64)> = pts.iter().copied().filter(|p| ok(*p)).map(|p| (p, 0.0)).collect();
        if v.len() >= 2 {
            self.push(Shape::Pline(v, closed));
        }
    }
    pub fn bulged(&mut self, pts: &[([f64; 2], f64)]) {
        let v: Vec<([f64; 2], f64)> = pts.iter().copied().filter(|(p, b)| ok(*p) && b.is_finite()).collect();
        if v.len() >= 2 {
            self.push(Shape::Pline(v, false));
        }
    }
    pub fn rect(&mut self, a: [f64; 2], b: [f64; 2]) {
        self.pline(&[a, [b[0], a[1]], b, [a[0], b[1]]], true);
    }
    pub fn text(&mut self, at: [f64; 2], t: &str, h: f64) {
        self.text_as(at, t, h, Align::Left, 0.0);
    }
    pub fn ctext(&mut self, at: [f64; 2], t: &str, h: f64) {
        self.text_as(at, t, h, Align::Center, 0.0);
    }
    pub fn text_as(&mut self, at: [f64; 2], t: &str, h: f64, align: Align, rot: f64) {
        let text = clean(t);
        if !text.is_empty() && ok(at) && h.is_finite() && h > 0.0 && rot.is_finite() {
            self.push(Shape::Text { at, text, h, align, rot });
        }
    }
    pub fn circle(&mut self, c: [f64; 2], r: f64) {
        if ok(c) && r.is_finite() && r > 0.0 {
            self.push(Shape::Circle(c, r));
        }
    }
    pub fn arc(&mut self, c: [f64; 2], r: f64, a0: f64, a1: f64) {
        if ok(c) && r.is_finite() && r > 0.0 && a0.is_finite() && a1.is_finite() {
            self.push(Shape::Arc(c, r, a0, a1));
        }
    }
    pub fn dim(&mut self, a: [f64; 2], b: [f64; 2], at: [f64; 2], text: &str, vertical: bool) {
        let along = if vertical { (a[1] - b[1]).abs() } else { (a[0] - b[0]).abs() };
        if ok(a) && ok(b) && ok(at) && along > 1e-3 {
            self.push(Shape::Dim { a, b, at, text: clean(text), vertical });
            self.count("dimensions");
        }
    }
    pub fn table(&mut self, t: Table) {
        self.push(Shape::Table(Box::new(t)));
    }
    /// A tag: the text in a circle (`circle`), a box, a diamond or a hexagon around `at`.
    pub fn tag(&mut self, at: [f64; 2], text: &str, h: f64, shape: TagShape) {
        let text = clean(text);
        if text.is_empty() || !ok(at) {
            return;
        }
        let n = text.chars().count().max(1) as f64;
        let half_w = (n * h * 0.45).max(h * 0.8);
        let half_h = h * 0.9;
        let [x, y] = at;
        match shape {
            TagShape::Circle => self.circle(at, half_w.max(half_h) * 1.05),
            TagShape::Box => self.rect([x - half_w, y - half_h], [x + half_w, y + half_h]),
            TagShape::Diamond => {
                let r = half_w.max(half_h) * 1.35;
                self.pline(&[[x, y + r], [x + r, y], [x, y - r], [x - r, y]], true);
            }
            TagShape::Hexagon => {
                let (w, hh) = (half_w + h * 0.35, half_h);
                self.pline(&[[x - w, y], [x - w + hh, y + hh], [x + w - hh, y + hh], [x + w, y], [x + w - hh, y - hh], [x - w + hh, y - hh]], true);
            }
        }
        self.text_as([x, y - h / 2.0], &text, h, Align::Center, 0.0);
    }
}

#[derive(Clone, Copy, Debug)]
pub(super) enum TagShape {
    Circle,
    Box,
    Diamond,
    Hexagon,
}

fn v3(p: [f64; 2]) -> Vec3 {
    Vec3::new(p[0], p[1], 0.0)
}

/// The text entity for a shape (left baseline, or centred on the baseline).
fn text_kind(at: [f64; 2], text: &str, h: f64, align: Align, rot: f64) -> EntityKind {
    let halign = match align {
        Align::Left => cadaddy_doc::HAlign::Left,
        Align::Center => cadaddy_doc::HAlign::Center,
    };
    EntityKind::Text(cadaddy_doc::Text {
        insert: v3(at),
        align_pt: (align != Align::Left).then(|| v3(at)),
        height: h,
        value: text.to_string(),
        rotation: rot,
        width_factor: 1.0,
        oblique: 0.0,
        style: "Standard".into(),
        halign,
        valign: cadaddy_doc::VAlign::Baseline,
    })
}

/// Adds the pen's shapes (but its dimensions) to the layout `space` straight into the drawing,
/// each tagged with `tag`; the dimensions it leaves for the dimension command.
pub(super) fn put(d: &mut Drawing, space: &Space, shapes: &[(String, Shape)], tag: &[RawTag]) -> Vec<(String, Shape)> {
    let mut dims = Vec::new();
    for (layer, s) in shapes {
        let kind = match s {
            Shape::Line(a, b) => EntityKind::Line(cadaddy_doc::Line { a: v3(*a), b: v3(*b) }),
            Shape::Pline(v, closed) => EntityKind::LwPolyline(cadaddy_doc::LwPolyline {
                vertices: v.iter().map(|(p, b)| PolyVertex { bulge: *b, ..PolyVertex::new(Vec2::new(p[0], p[1])) }).collect(),
                closed: *closed,
                const_width: 0.0,
                elevation: 0.0,
                plinegen: false,
            }),
            Shape::Text { at, text, h, align, rot } => text_kind(*at, text, *h, *align, *rot),
            Shape::Circle(c, r) => EntityKind::Circle(cadaddy_doc::Circle { center: v3(*c), radius: *r }),
            Shape::Arc(c, r, a0, a1) => EntityKind::Arc(cadaddy_doc::Arc { center: v3(*c), radius: *r, start: *a0, end: *a1 }),
            Shape::Table(t) => EntityKind::Table(t.clone()),
            Shape::Dim { .. } => {
                dims.push((layer.clone(), s.clone()));
                continue;
            }
        };
        let common = Common { layer: layer.as_str().into(), xdata: tag.to_vec().into(), ..Common::default() };
        // A shape the drawing refuses is left out; the sheet goes on.
        let _ = d.add(space, common, kind);
    }
    dims
}
