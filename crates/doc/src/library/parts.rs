//! CADaddy's parts library: over a thousand parametric blocks drawn here in code — architectural
//! (doors, windows, stairs, furniture, sanitary, kitchen), MEP (diffusers, grilles, VAV boxes,
//! fans, valves, pumps, panels, light fixtures), structural (steel shapes by designation, pipe,
//! lumber, rebar), site and annotation (north arrows, section and elevation marks, scale bars,
//! title blocks).
//!
//! Nothing is copied from another program's library. Each part comes from a family drawn here and
//! a size from public dimensions: nominal sizes, and for steel and rebar the published section
//! tables (dimensions are facts). Every block carries constant hidden `TRADE`, `TYPE` and `PART`
//! attributes, so counts and schedules can group placed parts.
//!
//! Where it makes sense a part is a dynamic block: a width or length that stretches (a linear
//! parameter with a stretch action, snapped to sensible steps) and named visibility states
//! (a door's swing side, a mark's direction). Furniture, cabinets, equipment and stairs also
//! carry a simple 3D body (3DFACE boxes) above their plan view.
//!
//! Sizes are inches unless a part says it is metric (millimetres). A *real-size* part's insertion
//! point is where it meets the wall (doors, windows, fixtures: the middle of the back edge or the
//! opening's start) or its centre; an *annotation* part (marks, tags, valves) is drawn at its
//! plotted size and placed at the drawing's scale.

use std::f64::consts::{PI, TAU};
use std::sync::Arc;

use cadaddy_geom::{PolyVertex, Vec2, Vec3};

use crate::dynblock::ValueSet;
use crate::dynblock::author::{self, NewActionKind, NewParameter};
use crate::{Attrib, Block, Circle, Drawing, Ellipse, Entity, EntityKind, Face3d, HAlign, Handle, Line, LwPolyline, Solid, Text, VAlign};

mod annotation;
mod arch;
mod mep;
mod site;
mod structural;

/// The trade a part belongs to: the palette's first level and the hidden `TRADE` value.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Discipline {
    Architectural,
    Mechanical,
    Electrical,
    Plumbing,
    Structural,
    Site,
    Annotation,
}

impl Discipline {
    pub const ALL: [Discipline; 7] = [
        Discipline::Architectural,
        Discipline::Mechanical,
        Discipline::Electrical,
        Discipline::Plumbing,
        Discipline::Structural,
        Discipline::Site,
        Discipline::Annotation,
    ];
    /// What the palette shows.
    pub fn label(self) -> &'static str {
        match self {
            Discipline::Architectural => "Architectural",
            Discipline::Mechanical => "Mechanical",
            Discipline::Electrical => "Electrical",
            Discipline::Plumbing => "Plumbing",
            Discipline::Structural => "Structural",
            Discipline::Site => "Site",
            Discipline::Annotation => "Annotation",
        }
    }
    /// The block-name prefix (US National CAD Standard discipline letters).
    pub fn prefix(self) -> &'static str {
        match self {
            Discipline::Architectural => "A",
            Discipline::Mechanical => "M",
            Discipline::Electrical => "E",
            Discipline::Plumbing => "P",
            Discipline::Structural => "S",
            Discipline::Site => "C",
            Discipline::Annotation => "G",
        }
    }
    /// The hidden `TRADE` value.
    pub fn trade(self) -> &'static str {
        match self {
            Discipline::Architectural => "ARCHITECTURAL",
            Discipline::Mechanical => "MECHANICAL",
            Discipline::Electrical => "ELECTRICAL",
            Discipline::Plumbing => "PLUMBING",
            Discipline::Structural => "STRUCTURAL",
            Discipline::Site => "SITE",
            Discipline::Annotation => "ANNOTATION",
        }
    }
    /// The discipline named `s` (its label or prefix, any case).
    pub fn parse(s: &str) -> Option<Discipline> {
        let s = s.trim();
        Discipline::ALL.into_iter().find(|d| d.label().eq_ignore_ascii_case(s) || d.prefix().eq_ignore_ascii_case(s))
    }
}

/// A family's numbers for one part (sizes, counts), and a designation or style word.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct Args {
    pub n: [f64; 6],
    pub s: &'static str,
}

impl Args {
    fn get(&self, i: usize) -> f64 {
        self.n.get(i).copied().unwrap_or(0.0)
    }
}

fn args(n: &[f64]) -> Args {
    let mut a = Args::default();
    for (slot, v) in a.n.iter_mut().zip(n) {
        *slot = *v;
    }
    a
}

fn args_s(s: &'static str, n: &[f64]) -> Args {
    Args { s, ..args(n) }
}

/// One library part.
#[derive(Clone, Debug)]
pub struct Part {
    /// The block name it is placed as (`A_DOOR_SGL_36_LH`): its discipline letter, then words.
    pub name: String,
    /// What the palette shows ("Door, single swing 36\" left hand").
    pub label: String,
    pub discipline: Discipline,
    /// Its group within the discipline ("Doors").
    pub category: &'static str,
    /// Words that find it besides its name, label and category.
    pub keywords: String,
    /// Drawn at plotted size, placed at the drawing's scale.
    pub annotation: bool,
    /// Drawn in millimetres (else inches).
    pub metric: bool,
    draw: fn(&mut Kit, &Args),
    args: Args,
}

/// A part's stretchable distance: a linear parameter from `base` to `end` and the stretch action
/// moving what lies in `frame`, its values from `min` to `max` in steps of `step`.
#[derive(Clone, Debug, PartialEq)]
pub struct Stretch {
    pub label: &'static str,
    pub base: Vec2,
    pub end: Vec2,
    /// Two opposite corners.
    pub frame: (Vec2, Vec2),
    pub min: f64,
    pub max: f64,
    pub step: f64,
}

/// A part drawn: its objects (handle 0), the visibility state each belongs to (`None`: always
/// shown), its stretches and its visibility parameter's label and state names.
#[derive(Clone, Debug, Default)]
pub struct Built {
    pub entities: Vec<Entity>,
    pub states_of: Vec<Option<u8>>,
    pub stretches: Vec<Stretch>,
    pub visibility: Option<(&'static str, Vec<&'static str>)>,
}

impl Part {
    /// The part's objects and dynamic behaviour, in block coordinates (insertion point at the
    /// origin).
    pub fn build(&self) -> Built {
        let mut k = Kit::new(if self.metric { 25.4 } else { 1.0 });
        (self.draw)(&mut k, &self.args);
        k.group = None;
        k.linetype = None;
        let h = if self.annotation { 0.05 } else { 2.0 } * k.unit;
        k.attdef_constant("TRADE", self.discipline.trade(), (0.0, 0.0), h);
        k.attdef_constant("TYPE", &self.category.to_ascii_uppercase(), (0.0, -h * 1.5), h);
        k.attdef_constant("PART", &self.name, (0.0, -h * 3.0), h);
        let (entities, states_of) = k.out.into_iter().unzip();
        Built { entities, states_of, stretches: k.stretches, visibility: k.visibility }
    }
    /// The objects a preview shows: the plan view in its first visibility state, without
    /// attribute definitions or the 3D body.
    pub fn preview_entities(&self) -> Vec<Entity> {
        let b = self.build();
        b.entities
            .into_iter()
            .zip(b.states_of)
            .filter(|(e, g)| g.is_none_or(|g| g == 0) && !matches!(e.kind, EntityKind::AttDef(_) | EntityKind::Face3d(_)))
            .map(|(e, _)| e)
            .collect()
    }
    /// Whether the part has a 3D body.
    pub fn has_3d(&self) -> bool {
        self.build().entities.iter().any(|e| matches!(e.kind, EntityKind::Face3d(_)))
    }
    /// Whether the part is a dynamic block (stretches or visibility states).
    pub fn is_dynamic(&self) -> bool {
        let b = self.build();
        !b.stretches.is_empty() || b.visibility.is_some()
    }
    /// Whether `query` (words, any case) finds this part: every word in its name, label,
    /// discipline, category or keywords.
    pub fn matches(&self, query: &str) -> bool {
        let hay = format!("{} {} {} {} {}", self.name, self.label, self.discipline.label(), self.category, self.keywords).to_ascii_lowercase();
        query.split_whitespace().all(|w| hay.contains(&w.to_ascii_lowercase()))
    }
}

/// Every part, in palette order (discipline, then category, then size).
pub fn parts() -> &'static [Part] {
    static ALL: std::sync::LazyLock<Vec<Part>> = std::sync::LazyLock::new(|| {
        let mut c = Catalog::default();
        arch::add(&mut c);
        mep::add(&mut c);
        structural::add(&mut c);
        site::add(&mut c);
        annotation::add(&mut c);
        let mut v = c.parts;
        v.sort_by_key(|p| p.discipline);
        v
    });
    &ALL
}

/// The part named `name` (its block name, any case).
pub fn part(name: &str) -> Option<&'static Part> {
    static INDEX: std::sync::LazyLock<std::collections::HashMap<String, usize>> =
        std::sync::LazyLock::new(|| parts().iter().enumerate().map(|(i, p)| (p.name.to_ascii_uppercase(), i)).collect());
    let n = name.trim();
    if n.len() > 64 {
        return None;
    }
    INDEX.get(&n.to_ascii_uppercase()).and_then(|&i| parts().get(i))
}

/// The categories of `discipline`, in palette order.
pub fn categories(discipline: Discipline) -> Vec<&'static str> {
    let mut v: Vec<&'static str> = Vec::new();
    for p in parts().iter().filter(|p| p.discipline == discipline) {
        if !v.contains(&p.category) {
            v.push(p.category);
        }
    }
    v
}

/// Drawing units per part unit: a part drawn in inches (or millimetres) measures the same in a
/// drawing of any units (INSUNITS; unitless follows MEASUREMENT).
pub fn units_scale(d: &Drawing, part: &Part) -> f64 {
    let per_inch = match d.header.i64("INSUNITS", 0) {
        2 => 1.0 / 12.0,
        4 => 25.4,
        5 => 2.54,
        6 => 0.0254,
        1 => 1.0,
        _ if d.header.i64("MEASUREMENT", 0) == 1 => 25.4,
        _ => 1.0,
    };
    if part.metric { per_inch / 25.4 } else { per_inch }
}

/// Define `part`'s block in `d` (its objects, then its stretches and visibility states) unless a
/// block of that name is there already. Returns the block's name.
pub fn define(d: &mut Drawing, part: &Part) -> Result<String, String> {
    if let Some(b) = d.block(&part.name) {
        return Ok(b.name.clone());
    }
    let built = part.build();
    // Hidden lines (wall cabinets, overhead doors) need the linetype.
    for lt in ["HIDDEN", "CENTER"] {
        if d.linetype(lt).is_none() && built.entities.iter().any(|e| e.common.linetype == lt) {
            let def = super::standard_linetypes_for(d).into_iter().find(|l| l.name == lt);
            d.linetypes.extend(def);
        }
    }
    let mut blk = Block::new(&part.name);
    blk.base = Vec3::ZERO;
    blk.description = format!("{} (CADaddy parts library)", part.label);
    let mut placed: Vec<(Handle, Option<u8>, Entity)> = Vec::with_capacity(built.entities.len());
    for (mut e, g) in built.entities.into_iter().zip(built.states_of) {
        e.handle = d.new_handle();
        // A reference shows the first visibility state until it is given another: the other
        // states' objects start hidden (changing the state shows them on the reference's copy).
        if g.is_some_and(|g| g != 0) {
            e.common.visible = false;
        }
        placed.push((e.handle, g, e.clone()));
        blk.entities.push(e);
    }
    d.blocks.insert(part.name.clone(), Arc::new(blk));
    for st in &built.stretches {
        let values = ValueSet { flags: 1 | 2 | 4, minimum: st.min, maximum: st.max, increment: st.step, list: Vec::new() };
        let p = NewParameter::Linear { base: st.base.to3(0.0), end: st.end.to3(0.0), values, from_midpoint: false };
        let (node, _) = author::add_parameter(d, &part.name, &p, Some(st.label), "")?;
        let (a, b) = st.frame;
        let frame = vec![Vec2::new(a.x, a.y), Vec2::new(b.x, a.y), Vec2::new(b.x, b.y), Vec2::new(a.x, b.y)];
        let (selection, vertices) = stretch_selection(&placed, a, b);
        if selection.is_empty() {
            continue;
        }
        author::add_action(d, &part.name, node, &NewActionKind::Stretch { frame, vertices }, &selection, st.end.to3(0.0), None)?;
    }
    if let Some((label, names)) = &built.visibility {
        let states: Vec<(String, Vec<Handle>)> = names
            .iter()
            .enumerate()
            .map(|(i, n)| (n.to_string(), placed.iter().filter(|(_, g, _)| g.is_some_and(|g| usize::from(g) == i)).map(|(h, _, _)| *h).collect()))
            .collect();
        author::add_parameter(d, &part.name, &NewParameter::Visibility { at: Vec3::ZERO, states }, Some(label), "")?;
    }
    Ok(part.name.clone())
}

fn inside(a: Vec2, b: Vec2, p: Vec2) -> bool {
    p.x >= a.x.min(b.x) && p.x <= a.x.max(b.x) && p.y >= a.y.min(b.y) && p.y <= a.y.max(b.y)
}

/// What a stretch frame moves: lines and polylines with a vertex in the frame (with the vertices
/// that move when not all do), and other objects whose reference point is in it. 3D faces move
/// the corners in the frame as a whole object would not.
fn stretch_selection(placed: &[(Handle, Option<u8>, Entity)], a: Vec2, b: Vec2) -> (Vec<Handle>, Vec<(Handle, Vec<i32>)>) {
    let mut sel = Vec::new();
    let mut verts = Vec::new();
    for (h, _, e) in placed {
        let pts: Vec<Vec2> = match &e.kind {
            EntityKind::Line(l) => vec![l.a.xy(), l.b.xy()],
            EntityKind::LwPolyline(p) => p.vertices.iter().map(|v| v.p).collect(),
            EntityKind::Circle(c) => vec![c.center.xy()],
            EntityKind::Arc(c) => vec![c.center.xy()],
            EntityKind::Ellipse(c) => vec![c.center.xy()],
            EntityKind::Text(t) => vec![t.align_pt.unwrap_or(t.insert).xy()],
            EntityKind::Solid(s) => s.corners.iter().map(|c| c.xy()).collect(),
            // Bodies, attribute definitions: left as they are.
            _ => continue,
        };
        let idx: Vec<i32> = pts.iter().enumerate().filter(|(_, p)| inside(a, b, **p)).filter_map(|(i, _)| i32::try_from(i).ok()).collect();
        if idx.is_empty() {
            continue;
        }
        let partial = idx.len() < pts.len();
        if partial && !matches!(e.kind, EntityKind::Line(_) | EntityKind::LwPolyline(_)) {
            // A solid half in the frame would move whole: leave it.
            continue;
        }
        sel.push(*h);
        if partial {
            verts.push((*h, idx));
        }
    }
    (sel, verts)
}

/// Collects parts as the family modules add them.
#[derive(Default)]
pub(crate) struct Catalog {
    parts: Vec<Part>,
}

impl Catalog {
    /// Add a part. `name` is made a block name: upper case, with anything but letters, digits
    /// and `-` as `_`.
    #[allow(clippy::too_many_arguments)]
    fn add(
        &mut self,
        discipline: Discipline,
        category: &'static str,
        name: &str,
        label: String,
        keywords: &str,
        draw: fn(&mut Kit, &Args),
        args: Args,
        flags: Flags,
    ) {
        let raw = format!("{}_{}", discipline.prefix(), name);
        let mut name: String = raw.chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' { c.to_ascii_uppercase() } else { '_' }).collect();
        while name.contains("__") {
            name = name.replace("__", "_");
        }
        self.parts.push(Part {
            name: name.trim_end_matches('_').to_string(),
            label,
            discipline,
            category,
            keywords: keywords.to_string(),
            annotation: flags.annotation,
            metric: flags.metric,
            draw,
            args,
        });
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct Flags {
    pub annotation: bool,
    pub metric: bool,
}

pub(crate) const REAL: Flags = Flags { annotation: false, metric: false };
pub(crate) const METRIC: Flags = Flags { annotation: false, metric: true };
pub(crate) const ANNO: Flags = Flags { annotation: true, metric: false };

/// Inches as a label: `36"`, `1'-6"` above a foot is not used; fractions as `1-1/2"`.
pub(crate) fn inches(v: f64) -> String {
    let whole = v.trunc();
    let frac = v - whole;
    let f = match (frac * 16.0).round() as i64 {
        0 => String::new(),
        n => {
            let (mut num, mut den) = (n, 16);
            while num % 2 == 0 && den > 1 {
                num /= 2;
                den /= 2;
            }
            format!("{num}/{den}")
        }
    };
    match (whole as i64, f.is_empty()) {
        (0, false) => format!("{f}\""),
        (w, true) => format!("{w}\""),
        (w, false) => format!("{w}-{f}\""),
    }
}

/// A number for a block name: `36`, `112_5` (no dot).
pub(crate) fn num(v: f64) -> String {
    let s = format!("{v}");
    s.replace('.', "_")
}

/// Draws one part. Lengths given to it are in the part's units; `unit` is how many of them make
/// an inch (1, or 25.4 for metric parts) for the few sizes the kit picks itself (text in tags).
pub struct Kit {
    out: Vec<(Entity, Option<u8>)>,
    unit: f64,
    /// The visibility state the next objects belong to.
    group: Option<u8>,
    /// The linetype the next objects get.
    linetype: Option<&'static str>,
    stretches: Vec<Stretch>,
    visibility: Option<(&'static str, Vec<&'static str>)>,
}

fn p3(p: (f64, f64)) -> Vec3 {
    Vec3::new(p.0, p.1, 0.0)
}

impl Kit {
    fn new(unit: f64) -> Kit {
        Kit { out: Vec::new(), unit, group: None, linetype: None, stretches: Vec::new(), visibility: None }
    }
    fn add(&mut self, kind: EntityKind) {
        let mut e = Entity::new(Handle(0), kind);
        if let Some(lt) = self.linetype {
            e.common.linetype = lt.into();
        }
        self.out.push((e, self.group));
    }
    /// The next objects belong to visibility state `g` (`None`: shown in every state).
    pub(crate) fn state(&mut self, g: Option<u8>) {
        self.group = g;
    }
    /// The next objects are drawn with hidden lines (`true`) or continuous ones.
    pub(crate) fn hidden(&mut self, on: bool) {
        self.linetype = on.then_some("HIDDEN");
    }
    pub(crate) fn center_lines(&mut self, on: bool) {
        self.linetype = on.then_some("CENTER");
    }
    pub(crate) fn visibility(&mut self, label: &'static str, states: &[&'static str]) {
        self.visibility = Some((label, states.to_vec()));
    }
    /// A stretchable distance from `base` to `end`, moving what lies in the frame `a`–`b`.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn stretch(
        &mut self,
        label: &'static str,
        base: (f64, f64),
        end: (f64, f64),
        a: (f64, f64),
        b: (f64, f64),
        min: f64,
        max: f64,
        step: f64,
    ) {
        self.stretches.push(Stretch {
            label,
            base: Vec2::new(base.0, base.1),
            end: Vec2::new(end.0, end.1),
            frame: (Vec2::new(a.0, a.1), Vec2::new(b.0, b.1)),
            min,
            max,
            step,
        });
    }
    pub(crate) fn line(&mut self, a: (f64, f64), b: (f64, f64)) {
        self.add(EntityKind::Line(Line { a: p3(a), b: p3(b) }));
    }
    pub(crate) fn circle(&mut self, c: (f64, f64), r: f64) {
        self.add(EntityKind::Circle(Circle { center: p3(c), radius: r }));
    }
    /// An arc from `a0` to `a1` degrees, counterclockwise.
    pub(crate) fn arc(&mut self, c: (f64, f64), r: f64, a0: f64, a1: f64) {
        self.add(EntityKind::Arc(crate::Arc { center: p3(c), radius: r, start: a0.to_radians(), end: a1.to_radians() }));
    }
    /// A full ellipse with half-widths `rx` (along X) and `ry`.
    pub(crate) fn ellipse(&mut self, c: (f64, f64), rx: f64, ry: f64) {
        let (major, ratio) = if rx >= ry { (Vec3::new(rx, 0.0, 0.0), ry / rx) } else { (Vec3::new(0.0, ry, 0.0), rx / ry) };
        self.add(EntityKind::Ellipse(Ellipse { center: p3(c), major, ratio, start: 0.0, end: TAU }));
    }
    /// A polyline through `pts` ((x, y, bulge to the next point)).
    pub(crate) fn pline(&mut self, pts: &[(f64, f64, f64)], closed: bool) {
        let vertices = pts.iter().map(|&(x, y, b)| PolyVertex { p: Vec2::new(x, y), bulge: b, start_width: 0.0, end_width: 0.0 }).collect();
        self.add(EntityKind::LwPolyline(LwPolyline { vertices, closed, const_width: 0.0, elevation: 0.0, plinegen: false }));
    }
    /// A closed polyline through `pts`.
    pub(crate) fn poly(&mut self, pts: &[(f64, f64)]) {
        let v: Vec<(f64, f64, f64)> = pts.iter().map(|&(x, y)| (x, y, 0.0)).collect();
        self.pline(&v, true);
    }
    /// Give the polyline drawn last a constant width `w` (a bar drawn along its centre line).
    pub(crate) fn set_last_width(&mut self, w: f64) {
        if let Some((Entity { kind: EntityKind::LwPolyline(p), .. }, _)) = self.out.last_mut() {
            p.const_width = w;
            for v in &mut p.vertices {
                v.start_width = w;
                v.end_width = w;
            }
        }
    }
    /// An open polyline through `pts`.
    pub(crate) fn path(&mut self, pts: &[(f64, f64)]) {
        let v: Vec<(f64, f64, f64)> = pts.iter().map(|&(x, y)| (x, y, 0.0)).collect();
        self.pline(&v, false);
    }
    pub(crate) fn rect(&mut self, a: (f64, f64), b: (f64, f64)) {
        self.poly(&[a, (b.0, a.1), b, (a.0, b.1)]);
    }
    /// A rectangle with rounded corners of radius `r`.
    pub(crate) fn round_rect(&mut self, a: (f64, f64), b: (f64, f64), r: f64) {
        let (x0, x1, y0, y1) = (a.0.min(b.0), a.0.max(b.0), a.1.min(b.1), a.1.max(b.1));
        let r = r.min((x1 - x0) / 2.0).min((y1 - y0) / 2.0).max(0.0);
        if r <= 0.0 {
            return self.rect(a, b);
        }
        let k = (PI / 8.0).tan();
        self.pline(
            &[
                (x0 + r, y0, 0.0),
                (x1 - r, y0, k),
                (x1, y0 + r, 0.0),
                (x1, y1 - r, k),
                (x1 - r, y1, 0.0),
                (x0 + r, y1, k),
                (x0, y1 - r, 0.0),
                (x0, y0 + r, k),
            ],
            true,
        );
    }
    /// A filled triangle or quadrilateral (a 2D solid; corners in drawing order).
    pub(crate) fn fill(&mut self, pts: &[(f64, f64)]) {
        let c: Vec<Vec3> = pts.iter().map(|p| p3(*p)).collect();
        let (Some(a), Some(b), Some(cc)) = (c.first(), c.get(1), c.get(2)) else { return };
        let d = c.get(3).copied().unwrap_or(*cc);
        // SOLID's corner order: 1, 2, 4, 3 for a convex quadrilateral.
        self.add(EntityKind::Solid(Solid { corners: [*a, *b, d, *cc] }));
    }
    /// A filled dot.
    pub(crate) fn dot(&mut self, c: (f64, f64), r: f64) {
        let h = r / 2.0;
        let vertices = [(c.0 - h, c.1), (c.0 + h, c.1)]
            .iter()
            .map(|&(x, y)| PolyVertex { p: Vec2::new(x, y), bulge: 1.0, start_width: r, end_width: r })
            .collect();
        self.add(EntityKind::LwPolyline(LwPolyline { vertices, closed: true, const_width: r, elevation: 0.0, plinegen: false }));
    }
    fn text_with(&mut self, at: (f64, f64), height: f64, value: &str, h: HAlign, v: VAlign, rotation: f64) {
        let p = p3(at);
        self.add(EntityKind::Text(Text {
            insert: p,
            align_pt: Some(p),
            height,
            value: value.into(),
            rotation: rotation.to_radians(),
            width_factor: 1.0,
            oblique: 0.0,
            style: "Standard".into(),
            halign: h,
            valign: v,
        }));
    }
    /// Text centred on `at`.
    pub(crate) fn text(&mut self, at: (f64, f64), height: f64, value: &str) {
        self.text_with(at, height, value, HAlign::Center, VAlign::Middle, 0.0);
    }
    /// Text from `at`, left aligned on its middle.
    pub(crate) fn text_left(&mut self, at: (f64, f64), height: f64, value: &str) {
        self.text_with(at, height, value, HAlign::Left, VAlign::Middle, 0.0);
    }
    fn attrib(tag: &str, prompt: &str, default: &str, at: (f64, f64), height: f64, centred: bool) -> Attrib {
        let p = p3(at);
        Attrib {
            tag: tag.into(),
            text: Text {
                insert: p,
                align_pt: centred.then_some(p),
                height,
                value: default.into(),
                rotation: 0.0,
                width_factor: 1.0,
                oblique: 0.0,
                style: "Standard".into(),
                halign: if centred { HAlign::Center } else { HAlign::Left },
                valign: if centred { VAlign::Middle } else { VAlign::Baseline },
            },
            invisible: false,
            constant: false,
            prompt: prompt.into(),
            props: Default::default(),
            multiline: None,
            modes: Default::default(),
        }
    }
    /// A visible attribute the user fills in, left aligned from `at`.
    pub(crate) fn attdef(&mut self, tag: &str, prompt: &str, default: &str, at: (f64, f64), height: f64) {
        let a = Self::attrib(tag, prompt, default, at, height, false);
        self.add(EntityKind::AttDef(Box::new(a)));
    }
    /// A visible attribute the user fills in, centred on `at` (tags in bubbles).
    pub(crate) fn attdef_c(&mut self, tag: &str, prompt: &str, default: &str, at: (f64, f64), height: f64) {
        let a = Self::attrib(tag, prompt, default, at, height, true);
        self.add(EntityKind::AttDef(Box::new(a)));
    }
    /// An attribute the user fills in that doesn't show (schedules, extraction).
    pub(crate) fn attdef_hidden(&mut self, tag: &str, prompt: &str, default: &str, at: (f64, f64), height: f64) {
        let mut a = Self::attrib(tag, prompt, default, at, height, false);
        a.invisible = true;
        self.add(EntityKind::AttDef(Box::new(a)));
    }
    fn attdef_constant(&mut self, tag: &str, value: &str, at: (f64, f64), height: f64) {
        let mut a = Self::attrib(tag, "", value, at, height, false);
        a.invisible = true;
        a.constant = true;
        self.add(EntityKind::AttDef(Box::new(a)));
    }
    /// A 3D face through 3 or 4 corners.
    pub(crate) fn face(&mut self, c: [Vec3; 4]) {
        self.add(EntityKind::Face3d(Face3d { corners: c, hidden_edges: 0 }));
    }
    /// A box from plan corner `a` to `b`, from height `z0` to `z1` (six faces).
    pub(crate) fn cube(&mut self, a: (f64, f64), b: (f64, f64), z0: f64, z1: f64) {
        let (x0, x1, y0, y1) = (a.0.min(b.0), a.0.max(b.0), a.1.min(b.1), a.1.max(b.1));
        let v = |x: f64, y: f64, z: f64| Vec3::new(x, y, z);
        self.face([v(x0, y0, z0), v(x1, y0, z0), v(x1, y1, z0), v(x0, y1, z0)]);
        self.face([v(x0, y0, z1), v(x1, y0, z1), v(x1, y1, z1), v(x0, y1, z1)]);
        self.face([v(x0, y0, z0), v(x1, y0, z0), v(x1, y0, z1), v(x0, y0, z1)]);
        self.face([v(x1, y0, z0), v(x1, y1, z0), v(x1, y1, z1), v(x1, y0, z1)]);
        self.face([v(x1, y1, z0), v(x0, y1, z0), v(x0, y1, z1), v(x1, y1, z1)]);
        self.face([v(x0, y1, z0), v(x0, y0, z0), v(x0, y0, z1), v(x0, y1, z1)]);
    }
    /// An upright cylinder of `sides` faces (and its two caps as fans of faces).
    pub(crate) fn cylinder(&mut self, c: (f64, f64), r: f64, z0: f64, z1: f64) {
        let sides = 16;
        let at = |i: usize, z: f64| {
            let a = TAU * i as f64 / sides as f64;
            Vec3::new(c.0 + r * a.cos(), c.1 + r * a.sin(), z)
        };
        let mid0 = Vec3::new(c.0, c.1, z0);
        let mid1 = Vec3::new(c.0, c.1, z1);
        for i in 0..sides {
            let j = i + 1;
            self.face([at(i, z0), at(j, z0), at(j, z1), at(i, z1)]);
            self.face([mid0, at(i, z0), at(j, z0), at(j, z0)]);
            self.face([mid1, at(i, z1), at(j, z1), at(j, z1)]);
        }
    }
    /// Inches in the part's units.
    pub(crate) fn inch(&self, v: f64) -> f64 {
        v * self.unit
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The task's bar: over a thousand distinct blocks.
    #[test]
    fn the_library_has_over_a_thousand_distinct_parts() {
        let n = parts().len();
        assert!(n >= 1000, "only {n} parts");
        let mut names = std::collections::HashSet::new();
        let mut labels = std::collections::HashSet::new();
        for p in parts() {
            assert!(names.insert(p.name.as_str()), "duplicate name {}", p.name);
            assert!(labels.insert(p.label.as_str()), "duplicate label {} ({})", p.label, p.name);
        }
        // Every discipline is there in strength.
        for d in Discipline::ALL {
            let k = parts().iter().filter(|p| p.discipline == d).count();
            eprintln!("{}: {k}", d.label());
            assert!(k >= 40, "{} has only {k} parts", d.label());
        }
        eprintln!("total: {n}");
        // Distinct drawings too, not one drawing under many names: no two parts draw the same.
        let mut drawn = std::collections::HashMap::new();
        for p in parts() {
            let key = format!("{:?}", p.preview_entities().iter().map(|e| &e.kind).collect::<Vec<_>>());
            if let Some(other) = drawn.insert(key, p.name.clone()) {
                panic!("{} draws the same as {other}", p.name);
            }
        }
    }

    #[test]
    fn every_part_is_a_valid_block_with_tags_and_sane_geometry() {
        let d = Drawing::new_imperial();
        let symbol_names: Vec<&str> = super::super::symbols::symbols().iter().map(|s| s.name).collect();
        for p in parts() {
            assert!(p.name.chars().all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_' || c == '-'), "{}", p.name);
            assert!(p.name.len() <= 64, "{} too long", p.name);
            assert!(p.name.starts_with(&format!("{}_", p.discipline.prefix())), "{}", p.name);
            assert!(!symbol_names.contains(&p.name.as_str()), "{} clashes with a symbol", p.name);
            assert!(!p.label.is_empty() && !p.category.is_empty(), "{}", p.name);
            let b = p.build();
            assert_eq!(b.entities.len(), b.states_of.len());
            let drawn = b.entities.iter().filter(|e| !matches!(e.kind, EntityKind::AttDef(_))).count();
            assert!(drawn >= 1, "{} draws nothing", p.name);
            let tags: Vec<&str> =
                b.entities.iter().filter_map(|e| if let EntityKind::AttDef(a) = &e.kind { Some(a.tag.as_str()) } else { None }).collect();
            for t in ["TRADE", "TYPE", "PART"] {
                assert!(tags.contains(&t), "{} lacks {t}", p.name);
            }
            let bounds = b.entities.iter().fold(cadaddy_geom::Bounds2::EMPTY, |acc, e| acc.union(&crate::extents::entity_bounds(&d, e, 1)));
            // A 1:1000 scale bar is 40 m long in the model.
            let limit = if p.metric { 60_000.0 } else { 1_600.0 };
            assert!(bounds.min.is_finite() && bounds.max.is_finite(), "{} bounds {bounds:?}", p.name);
            assert!(bounds.width() > 0.0 && bounds.width() < limit && bounds.height() < limit, "{} bounds {bounds:?}", p.name);
            if let Some((_, states)) = &b.visibility {
                assert!(states.len() >= 2, "{}", p.name);
                for i in 0..states.len() {
                    assert!(b.states_of.iter().any(|g| g.is_some_and(|g| usize::from(g) == i)), "{} state {i} shows nothing", p.name);
                }
            } else {
                assert!(b.states_of.iter().all(Option::is_none), "{} has states but no visibility parameter", p.name);
            }
            for s in &b.stretches {
                assert!(s.min > 0.0 && s.min <= s.max && s.step > 0.0, "{} {s:?}", p.name);
                let now = (s.end - s.base).len();
                assert!(now >= s.min - 1e-9 && now <= s.max + 1e-9, "{} {} {now} outside {}..{}", p.name, s.label, s.min, s.max);
            }
        }
        assert!(part("a_door_sgl_36_lh").is_some());
        assert!(part("nope").is_none());
    }

    #[test]
    fn many_parts_are_dynamic_and_some_have_3d_bodies() {
        let dynamic = parts().iter().filter(|p| p.is_dynamic()).count();
        let bodies = parts().iter().filter(|p| p.has_3d()).count();
        assert!(dynamic >= 280, "{dynamic} dynamic parts");
        assert!(bodies >= 150, "{bodies} parts with 3D");
    }

    #[test]
    fn search_finds_parts_by_words() {
        let found = |q: &str| parts().iter().filter(|p| p.matches(q)).map(|p| p.name.as_str()).collect::<Vec<_>>();
        assert!(found("w8x31").contains(&"S_W8X31"), "{:?}", found("w8x31"));
        assert!(found("vav reheat 8").iter().any(|n| n.starts_with("M_VAV")));
        assert!(found("north arrow").len() >= 6);
        assert!(found("door 36").iter().any(|n| n.starts_with("A_DOOR")));
        assert!(found("zzzz").is_empty());
        assert_eq!(Discipline::parse("mechanical"), Some(Discipline::Mechanical));
        assert_eq!(Discipline::parse("S"), Some(Discipline::Structural));
        assert!(categories(Discipline::Structural).contains(&"Steel W shapes"));
    }

    fn place(d: &mut Drawing, name: &str) -> Handle {
        let i = crate::Insert {
            block: name.into(),
            insert: Vec3::ZERO,
            scale: Vec3::new(1.0, 1.0, 1.0),
            rotation: 0.0,
            attribs: Vec::new(),
            cols: 1,
            rows: 1,
            col_spacing: 0.0,
            row_spacing: 0.0,
            clip: None,
        };
        d.add(&crate::Space::Model, crate::Common::default(), EntityKind::Insert(i)).unwrap()
    }

    #[test]
    fn define_makes_a_dynamic_block_whose_width_stretches() {
        let mut d = Drawing::new_imperial();
        let p = part("A_CAB_BASE_24").unwrap();
        let name = define(&mut d, p).unwrap();
        assert_eq!(name, "A_CAB_BASE_24");
        // Defined once.
        let n = d.blocks.len();
        define(&mut d, p).unwrap();
        assert_eq!(d.blocks.len(), n);
        let m = crate::dynblock::dynamic_block(&d, &name).unwrap();
        assert!(m.parameters.iter().any(|p| p.kind.label() == Some("Width")), "{m:?}");
        assert_eq!(m.actions.len(), 1);
        // Place it and widen it to 36: the right side moves 12, the left stays.
        let ins = place(&mut d, &name);
        crate::dynblock::set_property(&mut d, ins, "Width", "36").unwrap();
        let EntityKind::Insert(i) = &d.entity(ins).unwrap().kind else { panic!() };
        let blk = d.block(&i.block).unwrap();
        let b = blk.entities.iter().fold(cadaddy_geom::Bounds2::EMPTY, |acc, e| {
            if matches!(e.kind, EntityKind::AttDef(_) | EntityKind::Face3d(_)) { acc } else { acc.union(&crate::extents::entity_bounds(&d, e, 1)) }
        });
        assert!((b.min.x - 0.0).abs() < 1e-6 && (b.max.x - 36.0).abs() < 1e-6, "{b:?}");
    }

    #[test]
    fn define_makes_visibility_states() {
        let mut d = Drawing::new_imperial();
        let p = part("A_DOOR_SGL_36_LH").unwrap();
        let name = define(&mut d, p).unwrap();
        let m = crate::dynblock::dynamic_block(&d, &name).unwrap();
        let states = m
            .parameters
            .iter()
            .find_map(|p| if let crate::dynblock::ParameterKind::Visibility { states, .. } = &p.kind { Some(states.len()) } else { None });
        assert_eq!(states, Some(2));
        // Only the first state shows on a new reference.
        let blk = d.block(&name).unwrap();
        let shown_arcs = blk.entities.iter().filter(|e| matches!(e.kind, EntityKind::Arc(_)) && e.common.visible).count();
        assert_eq!(shown_arcs, 1);
        let ins = place(&mut d, &name);
        crate::dynblock::set_property(&mut d, ins, "Swing", "Swing out").unwrap();
        // The reference's own copy now shows the other swing only.
        let EntityKind::Insert(i) = &d.entity(ins).unwrap().kind else { panic!() };
        let arcs: Vec<(bool, f64)> = d
            .block(&i.block)
            .unwrap()
            .entities
            .iter()
            .filter_map(|e| if let EntityKind::Arc(a) = &e.kind { Some((e.common.visible, a.start)) } else { None })
            .collect();
        assert_eq!(arcs.iter().filter(|(v, _)| *v).count(), 1, "{arcs:?}");
        assert!(arcs.iter().any(|(v, start)| *v && *start < std::f64::consts::PI), "the out swing is above the wall: {arcs:?}");
        assert!(crate::dynblock::set_property(&mut d, ins, "Swing", "Sideways").is_err());
    }

    #[test]
    fn metric_parts_scale_into_imperial_drawings_and_back() {
        let imp = Drawing::new_imperial();
        let mut met = Drawing::new_imperial();
        met.header.set_i64("INSUNITS", 4);
        let ipe = part("S_IPE200").unwrap();
        let w = part("S_W8X31").unwrap();
        assert!(ipe.metric && !w.metric);
        assert!((units_scale(&met, ipe) - 1.0).abs() < 1e-12);
        assert!((units_scale(&imp, ipe) - 1.0 / 25.4).abs() < 1e-12);
        assert!((units_scale(&met, w) - 25.4).abs() < 1e-12);
    }

    #[test]
    fn labels_for_sizes() {
        assert_eq!(inches(36.0), "36\"");
        assert_eq!(inches(1.5), "1-1/2\"");
        assert_eq!(inches(0.375), "3/8\"");
        assert_eq!(num(112.5), "112_5");
    }
}
