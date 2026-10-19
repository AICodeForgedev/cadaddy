//! The prompt sequences of the solid commands. Each gathers what the JSON form takes and runs it.

use cadaddy_doc::{EntityKind, Handle, Solid3d, SolidShape};
use cadaddy_geom::{Vec2, Vec3};
use serde_json::{Value, json};

use super::super::machines::{SelOutcome, SelectPhase, number};
use crate::{Accept, EngineError, Input, Interactive, Prompt, Result, Session, Step};

fn need(what: &str) -> EngineError {
    EngineError::Other(format!("Requires {what}."))
}

/// A distance typed, or picked as the distance from `base`.
fn distance(i: &Input, base: Option<Vec2>) -> Option<f64> {
    match i {
        Input::Text(t) => number(t),
        Input::Point(p) => base.map(|b| b.dist(*p)),
        _ => None,
    }
}

/// Echo what a command made and end it.
fn finish(s: &mut Session, r: Result<Value>) -> Result<Step> {
    let v = r?;
    if let Some(m) = v.get("message").and_then(Value::as_str) {
        s.echo(m.to_string());
    }
    Ok(Step::Done)
}

fn hexes(hs: &[Handle]) -> Vec<String> {
    hs.iter().map(|h| h.hex()).collect()
}

fn v3(p: Vec2) -> Vec3 {
    Vec3::new(p.x, p.y, 0.0)
}

/// The last height a solid command used (the `<default>`), kept in the session.
fn last_height(s: &Session) -> f64 {
    s.last_used.solid_height.filter(|h| h.is_finite() && *h != 0.0).unwrap_or(1.0)
}

fn height_prompt(s: &Session, msg: &str, base: Option<Vec2>) -> Prompt {
    Prompt::new(msg, Accept::POINT_OR_NUMBER).base_opt(base).default(format!("{:.4}", last_height(s)))
}

/// A height: Enter takes the last one; a pick, the distance from the base.
fn height(s: &mut Session, i: &Input, base: Option<Vec2>) -> Result<Option<f64>> {
    let h = match i {
        Input::Enter => Some(last_height(s)),
        _ => distance(i, base),
    };
    match h {
        Some(h) if h.is_finite() && h != 0.0 => {
            s.last_used.solid_height = Some(h);
            Ok(Some(h))
        }
        Some(_) => Err(EngineError::Other("Value must be nonzero.".into())),
        None => Ok(None),
    }
}

// ---------------- BOX / WEDGE ----------------

pub struct BoxM {
    wedge: bool,
    center: bool,
    first: Option<Vec2>,
    /// Base length and width (signed: the direction from the first corner).
    size: Option<(f64, f64)>,
    cube: bool,
    asking: Option<&'static str>,
    length: Option<f64>,
}

impl BoxM {
    pub fn new(wedge: bool) -> Self {
        BoxM { wedge, center: false, first: None, size: None, cube: false, asking: None, length: None }
    }
    fn params(&self, h: f64) -> Option<Value> {
        let f = self.first?;
        let (l, w) = self.size?;
        let h = if self.cube { l.abs() } else { h };
        Some(if self.center {
            json!({ "center": [f.x, f.y, 0.0], "length": l.abs() * 2.0, "width": w.abs() * 2.0, "height": h })
        } else {
            json!({ "corner": [f.x.min(f.x + l), f.y.min(f.y + w), 0.0], "length": l.abs(), "width": w.abs(), "height": h })
        })
    }
    fn run(&self, s: &mut Session, h: f64) -> Result<Step> {
        let p = self.params(h).ok_or_else(|| need("a base"))?;
        let r = if self.wedge { super::run_wedge(s, &p) } else { super::run_box(s, &p) };
        finish(s, r)
    }
}

impl Interactive for BoxM {
    fn name(&self) -> &'static str {
        if self.wedge { "WEDGE" } else { "BOX" }
    }
    fn prompt(&self, s: &Session) -> Prompt {
        match (self.first, self.size, self.asking) {
            (None, _, _) if self.center => Prompt::new("Specify center", Accept::POINT),
            (None, _, _) => Prompt::new("Specify first corner", Accept::POINT).kw(&["Center"]),
            (Some(f), None, Some("length")) => Prompt::new("Specify length", Accept::POINT_OR_NUMBER).base(f),
            (Some(f), None, Some("width")) => Prompt::new("Specify width", Accept::POINT_OR_NUMBER).base(f),
            (Some(f), None, _) => Prompt::new("Specify other corner", Accept::POINT).kw(&["Cube", "Length"]).corner(f),
            (Some(f), Some(_), _) => height_prompt(s, "Specify height", Some(f)),
        }
    }
    fn input(&mut self, s: &mut Session, i: Input) -> Result<Step> {
        if matches!(i, Input::Cancel) {
            return Ok(Step::Cancel);
        }
        match (self.first, self.size) {
            (None, _) => match i {
                Input::Point(p) => self.first = Some(p),
                Input::Keyword(k) | Input::Text(k) if k.to_ascii_lowercase().starts_with('c') => self.center = true,
                Input::Enter => return Ok(Step::Done),
                _ => {}
            },
            (Some(f), None) => match (self.asking, i) {
                (None, Input::Point(p)) => {
                    let d = p - f;
                    if d.x.abs() < 1e-12 || d.y.abs() < 1e-12 {
                        return Err(EngineError::Other("The corners must not line up.".into()));
                    }
                    self.size = Some((d.x, d.y));
                }
                (None, Input::Keyword(k) | Input::Text(k)) => match k.to_ascii_lowercase().chars().next() {
                    Some('c') => {
                        self.cube = true;
                        self.asking = Some("length");
                    }
                    Some('l') => self.asking = Some("length"),
                    _ => {}
                },
                (Some("length"), i) => {
                    let l = distance(&i, Some(f)).filter(|v| v.is_finite() && *v != 0.0).ok_or_else(|| need("a nonzero distance"))?;
                    if self.cube {
                        self.size = Some((l, l));
                        return self.run(s, l.abs());
                    }
                    self.length = Some(l);
                    self.asking = Some("width");
                }
                (Some(_), i) => {
                    let w = distance(&i, Some(f)).filter(|v| v.is_finite() && *v != 0.0).ok_or_else(|| need("a nonzero distance"))?;
                    self.size = Some((self.length.unwrap_or(w), w));
                    self.asking = None;
                }
                _ => {}
            },
            (Some(f), Some(_)) => {
                if let Some(h) = height(s, &i, Some(f))? {
                    return self.run(s, h);
                }
            }
        }
        Ok(Step::Continue)
    }
    fn preview(&self, _s: &Session, c: Vec2) -> Vec<EntityKind> {
        let Some(f) = self.first else { return Vec::new() };
        match self.size {
            None if self.asking.is_none() => {
                let (a, b) = if self.center { (f - (c - f), c) } else { (f, c) };
                vec![super::super::helpers::lwpoly(super::super::helpers::rect_vertices(a, b), true)]
            }
            Some((l, w)) => {
                let h = f.dist(c).max(1e-9);
                let Some(p) = self.params(h) else { return Vec::new() };
                let Some(o) = p.get("corner").or(p.get("center")).and_then(super::point3_value) else { return Vec::new() };
                let (l, w) = if self.center { (l.abs() * 2.0, w.abs() * 2.0) } else { (l.abs(), w.abs()) };
                let o = if self.center { o - Vec3::new(l / 2.0, w / 2.0, h / 2.0) } else { o };
                let shape = if self.wedge {
                    SolidShape::Wedge { length: l, width: w, height: h }
                } else {
                    SolidShape::Box { length: l, width: w, height: h }
                };
                vec![EntityKind::Solid3d(Box::new(super::placed(shape, o, 0.0, false)))]
            }
            None => Vec::new(),
        }
    }
}

// ---------------- CYLINDER / CONE / SPHERE / TORUS / PYRAMID ----------------

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Round {
    Cylinder,
    Cone,
    Sphere,
    Torus,
    Pyramid,
}

pub struct RoundM {
    kind: Round,
    center: Option<Vec2>,
    radius: Option<f64>,
    diameter: bool,
    /// Asking for the cone's or pyramid's top radius before the height.
    top: Option<f64>,
    asking_top: bool,
    sides: u32,
    asking_sides: bool,
}

impl RoundM {
    pub fn new(kind: Round) -> Self {
        RoundM { kind, center: None, radius: None, diameter: false, top: None, asking_top: false, sides: 4, asking_sides: false }
    }
    fn params(&self, last: Option<f64>) -> Option<Value> {
        let c = self.center?;
        let r = self.radius?;
        let mut p = json!({ "center": [c.x, c.y, 0.0], "radius": r });
        let o = p.as_object_mut()?;
        match self.kind {
            Round::Torus => {
                o.insert("tube".into(), json!(last?));
            }
            Round::Sphere => {}
            _ => {
                o.insert("height".into(), json!(last?));
            }
        }
        if let Some(t) = self.top {
            o.insert("topRadius".into(), json!(t));
        }
        if self.kind == Round::Pyramid {
            o.insert("sides".into(), json!(self.sides));
        }
        Some(p)
    }
    fn run(&self, s: &mut Session, last: Option<f64>) -> Result<Step> {
        let p = self.params(last).ok_or_else(|| need("a size"))?;
        let r = match self.kind {
            Round::Cylinder => super::run_cylinder(s, &p),
            Round::Cone => super::run_cone(s, &p),
            Round::Sphere => super::run_sphere(s, &p),
            Round::Torus => super::run_torus(s, &p),
            Round::Pyramid => super::run_pyramid(s, &p),
        };
        finish(s, r)
    }
    fn solid(&self, last: f64) -> Option<Solid3d> {
        let c = v3(self.center?);
        let r = self.radius?;
        let shape = match self.kind {
            Round::Cylinder => SolidShape::Frustum { base_radius: r, top_radius: r, height: last },
            Round::Cone => SolidShape::Frustum { base_radius: r, top_radius: self.top.unwrap_or(0.0), height: last },
            Round::Sphere => SolidShape::Sphere { radius: r },
            Round::Torus => SolidShape::Torus { radius: r, tube: last.min(r * 0.999) },
            Round::Pyramid => {
                let k = 1.0 / (std::f64::consts::PI / f64::from(self.sides)).cos();
                SolidShape::Pyramid { sides: self.sides, base_radius: r * k, top_radius: self.top.unwrap_or(0.0) * k, height: last }
            }
        };
        let rot = if self.kind == Round::Pyramid { std::f64::consts::PI / f64::from(self.sides) } else { 0.0 };
        Some(super::placed(shape, c, rot, false))
    }
}

impl Interactive for RoundM {
    fn name(&self) -> &'static str {
        match self.kind {
            Round::Cylinder => "CYLINDER",
            Round::Cone => "CONE",
            Round::Sphere => "SPHERE",
            Round::Torus => "TORUS",
            Round::Pyramid => "PYRAMID",
        }
    }
    fn prompt(&self, s: &Session) -> Prompt {
        if self.asking_sides {
            return Prompt::new("Enter number of sides", Accept::NUMBER).default(self.sides.to_string());
        }
        let Some(c) = self.center else {
            return match self.kind {
                Round::Pyramid => {
                    Prompt::new(format!("{} sides Circumscribed\nSpecify center point of base", self.sides), Accept::POINT).kw(&["Sides"])
                }
                Round::Sphere | Round::Torus => Prompt::new("Specify center point", Accept::POINT),
                _ => Prompt::new("Specify center point of base", Accept::POINT),
            };
        };
        let Some(r) = self.radius else {
            let what = if self.diameter { "diameter" } else { "radius" };
            let base = matches!(self.kind, Round::Cylinder | Round::Cone | Round::Pyramid);
            let msg = format!("Specify {}{what}", if base { "base " } else { "" });
            let p = Prompt::new(msg, Accept::POINT_OR_NUMBER).base(c);
            return if self.diameter { p } else { p.kw(&["Diameter"]) };
        };
        if self.asking_top {
            return Prompt::new("Specify top radius", Accept::POINT_OR_NUMBER).base(c).default("0.0000");
        }
        match self.kind {
            Round::Torus => Prompt::new("Specify tube radius", Accept::POINT_OR_NUMBER).base(c + Vec2::new(r, 0.0)),
            Round::Cone | Round::Pyramid => height_prompt(s, "Specify height", Some(c)).kw(&["Top radius"]),
            _ => height_prompt(s, "Specify height", Some(c)),
        }
    }
    fn input(&mut self, s: &mut Session, i: Input) -> Result<Step> {
        if matches!(i, Input::Cancel) {
            return Ok(Step::Cancel);
        }
        let kw = |i: &Input| match i {
            Input::Keyword(k) => Some(k.to_ascii_lowercase()),
            Input::Text(t) if number(t).is_none() => Some(t.trim().to_ascii_lowercase()),
            _ => None,
        };
        if self.asking_sides {
            let n = match &i {
                Input::Enter => Some(f64::from(self.sides)),
                Input::Text(t) => number(t),
                _ => None,
            };
            match n {
                Some(n) if (3.0..=32.0).contains(&n) && n.fract() == 0.0 => {
                    self.sides = n as u32;
                    self.asking_sides = false;
                }
                _ => return Err(EngineError::Other("Requires an integer between 3 and 32.".into())),
            }
            return Ok(Step::Continue);
        }
        let Some(c) = self.center else {
            match i {
                Input::Point(p) => self.center = Some(p),
                Input::Enter => return Ok(Step::Done),
                ref other if self.kind == Round::Pyramid && kw(other).is_some_and(|k| k.starts_with('s')) => self.asking_sides = true,
                _ => {}
            }
            return Ok(Step::Continue);
        };
        let Some(r) = self.radius else {
            if kw(&i).is_some_and(|k| k.starts_with('d')) {
                self.diameter = true;
                return Ok(Step::Continue);
            }
            let v = distance(&i, Some(c)).filter(|v| v.is_finite() && *v > 0.0).ok_or_else(|| need("a positive distance"))?;
            self.radius = Some(if self.diameter && matches!(i, Input::Text(_)) { v / 2.0 } else { v });
            if self.kind == Round::Sphere {
                return self.run(s, None);
            }
            return Ok(Step::Continue);
        };
        if self.asking_top {
            let t = match &i {
                Input::Enter => Some(0.0),
                _ => distance(&i, Some(c)),
            };
            self.top = Some(t.filter(|v| v.is_finite() && *v >= 0.0).ok_or_else(|| need("a distance"))?);
            self.asking_top = false;
            return Ok(Step::Continue);
        }
        if matches!(self.kind, Round::Cone | Round::Pyramid) && kw(&i).is_some_and(|k| k.starts_with('t')) {
            self.asking_top = true;
            return Ok(Step::Continue);
        }
        if self.kind == Round::Torus {
            let t = distance(&i, Some(c + Vec2::new(r, 0.0))).filter(|v| v.is_finite() && *v > 0.0).ok_or_else(|| need("a positive distance"))?;
            return self.run(s, Some(t));
        }
        if let Some(h) = height(s, &i, Some(c))? {
            return self.run(s, Some(h));
        }
        Ok(Step::Continue)
    }
    fn preview(&self, _s: &Session, cur: Vec2) -> Vec<EntityKind> {
        let Some(c) = self.center else { return Vec::new() };
        match self.radius {
            None if !self.asking_sides => {
                let r = c.dist(cur);
                if r <= 1e-12 {
                    return Vec::new();
                }
                vec![super::super::helpers::circle(c, r)]
            }
            Some(r) if !self.asking_top => {
                let last = if self.kind == Round::Torus { (c + Vec2::new(r, 0.0)).dist(cur) } else { c.dist(cur) };
                self.solid(last.max(1e-9)).map(|so| vec![EntityKind::Solid3d(Box::new(so))]).unwrap_or_default()
            }
            _ => Vec::new(),
        }
    }
}

// ---------------- EXTRUDE ----------------

#[derive(Default)]
pub struct ExtrudeM {
    sel: SelectPhase,
    picked: Option<Vec<Handle>>,
    taper: f64,
    asking_taper: bool,
    asking_path: Option<SelectPhase>,
}

impl ExtrudeM {
    fn run(&self, s: &mut Session, extra: Value) -> Result<Step> {
        let mut p = json!({ "handles": hexes(self.picked.as_deref().unwrap_or(&[])), "taper": self.taper });
        if let (Some(o), Some(x)) = (p.as_object_mut(), extra.as_object()) {
            o.extend(x.clone());
        }
        let r = super::run_extrude(s, &p);
        s.set_selection(Vec::new());
        finish(s, r)
    }
}

impl Interactive for ExtrudeM {
    fn name(&self) -> &'static str {
        "EXTRUDE"
    }
    fn begin(&mut self, s: &mut Session) -> Result<Step> {
        self.sel = SelectPhase::begin(s);
        if self.sel.done {
            self.picked = Some(self.sel.picked.clone());
        }
        Ok(Step::Continue)
    }
    fn prompt(&self, s: &Session) -> Prompt {
        if self.picked.is_none() {
            return Prompt::new("Select objects to extrude", Accept::SELECT);
        }
        if self.asking_path.is_some() {
            return Prompt::new("Select extrusion path", Accept::SELECT);
        }
        if self.asking_taper {
            return Prompt::new("Specify angle of taper for extrusion", Accept::NUMBER).default(format!("{}", self.taper));
        }
        height_prompt(s, "Specify height of extrusion", None).kw(&["Path", "Taper angle"])
    }
    fn input(&mut self, s: &mut Session, i: Input) -> Result<Step> {
        if matches!(i, Input::Cancel) {
            return Ok(Step::Cancel);
        }
        if self.picked.is_none() {
            match self.sel.feed(s, &i)? {
                SelOutcome::More => {}
                SelOutcome::Empty => return Ok(Step::Done),
                SelOutcome::Done(hs) => self.picked = Some(hs),
            }
            return Ok(Step::Continue);
        }
        if let Some(mut ph) = self.asking_path.take() {
            match ph.feed(s, &i)? {
                SelOutcome::More => self.asking_path = Some(ph),
                SelOutcome::Empty => {}
                SelOutcome::Done(hs) => {
                    let path = hs.first().map(|h| h.hex()).ok_or_else(|| need("a path"))?;
                    return self.run(s, json!({ "path": path }));
                }
            }
            return Ok(Step::Continue);
        }
        if self.asking_taper {
            let a = match &i {
                Input::Enter => Some(self.taper),
                Input::Text(t) => t.trim().parse::<f64>().ok(),
                _ => None,
            };
            self.taper = a.filter(|a| a.is_finite() && a.abs() < 90.0).ok_or_else(|| need("an angle between -90 and 90"))?;
            self.asking_taper = false;
            return Ok(Step::Continue);
        }
        let key = match &i {
            Input::Keyword(k) => Some(k.to_ascii_lowercase()),
            Input::Text(t) if number(t).is_none() => Some(t.trim().to_ascii_lowercase()),
            _ => None,
        };
        match key.as_deref().and_then(|k| k.chars().next()) {
            Some('p') => {
                self.asking_path = Some(SelectPhase::single());
                return Ok(Step::Continue);
            }
            Some('t') => {
                self.asking_taper = true;
                return Ok(Step::Continue);
            }
            _ => {}
        }
        // A picked height: from the first profile's first point.
        let base = s.doc().ok().and_then(|d| {
            let h = self.picked.as_ref()?.first()?;
            let e = d.entity(*h)?;
            super::profile_of(d, e).and_then(|p| p.loops.first()?.first().map(|v| v.p))
        });
        if let Some(h) = height(s, &i, base)? {
            return self.run(s, json!({ "height": h }));
        }
        Ok(Step::Continue)
    }
}

// ---------------- REVOLVE ----------------

#[derive(Default)]
pub struct RevolveM {
    sel: SelectPhase,
    picked: Option<Vec<Handle>>,
    start: Option<Vec2>,
    end: Option<Vec2>,
    picking_axis: Option<SelectPhase>,
}

impl RevolveM {
    fn run(&self, s: &mut Session, angle: f64) -> Result<Step> {
        let (Some(a), Some(b)) = (self.start, self.end) else { return Err(need("an axis")) };
        let p = json!({ "handles": hexes(self.picked.as_deref().unwrap_or(&[])), "axisStart": [a.x, a.y], "axisEnd": [b.x, b.y], "angle": angle });
        let r = super::run_revolve(s, &p);
        s.set_selection(Vec::new());
        finish(s, r)
    }
}

impl Interactive for RevolveM {
    fn name(&self) -> &'static str {
        "REVOLVE"
    }
    fn begin(&mut self, s: &mut Session) -> Result<Step> {
        self.sel = SelectPhase::begin(s);
        if self.sel.done {
            self.picked = Some(self.sel.picked.clone());
        }
        Ok(Step::Continue)
    }
    fn prompt(&self, _s: &Session) -> Prompt {
        if self.picked.is_none() {
            return Prompt::new("Select objects to revolve", Accept::SELECT);
        }
        if self.picking_axis.is_some() {
            return Prompt::new("Select an object", Accept::SELECT);
        }
        match (self.start, self.end) {
            (None, _) => Prompt::new("Specify axis start point or define axis by", Accept::POINT).kw(&["Object", "X", "Y"]),
            (Some(a), None) => Prompt::new("Specify axis endpoint", Accept::POINT).base(a),
            _ => Prompt::new("Specify angle of revolution", Accept::NUMBER).default("360"),
        }
    }
    fn input(&mut self, s: &mut Session, i: Input) -> Result<Step> {
        if matches!(i, Input::Cancel) {
            return Ok(Step::Cancel);
        }
        if self.picked.is_none() {
            match self.sel.feed(s, &i)? {
                SelOutcome::More => {}
                SelOutcome::Empty => return Ok(Step::Done),
                SelOutcome::Done(hs) => self.picked = Some(hs),
            }
            return Ok(Step::Continue);
        }
        if let Some(mut ph) = self.picking_axis.take() {
            match ph.feed(s, &i)? {
                SelOutcome::More => self.picking_axis = Some(ph),
                SelOutcome::Empty => {}
                SelOutcome::Done(hs) => {
                    let d = s.doc()?;
                    let line = hs.first().and_then(|h| d.entity(*h)).and_then(|e| match &e.kind {
                        EntityKind::Line(l) => Some((l.a.xy(), l.b.xy())),
                        EntityKind::LwPolyline(p) => Some((p.vertices.first()?.p, p.vertices.get(1)?.p)),
                        _ => None,
                    });
                    let (a, b) = line.ok_or_else(|| need("a line or polyline segment"))?;
                    self.start = Some(a);
                    self.end = Some(b);
                }
            }
            return Ok(Step::Continue);
        }
        match (self.start, self.end, i) {
            (None, _, Input::Point(p)) => self.start = Some(p),
            (None, _, Input::Keyword(k) | Input::Text(k)) => match k.trim().to_ascii_lowercase().as_str() {
                "o" | "object" => self.picking_axis = Some(SelectPhase::single()),
                "x" => {
                    self.start = Some(Vec2::ZERO);
                    self.end = Some(Vec2::new(1.0, 0.0));
                }
                "y" => {
                    self.start = Some(Vec2::ZERO);
                    self.end = Some(Vec2::new(0.0, 1.0));
                }
                _ => {}
            },
            (Some(a), None, Input::Point(p)) if !p.near(a, 1e-12) => self.end = Some(p),
            (Some(_), Some(_), Input::Enter) => return self.run(s, 360.0),
            (Some(_), Some(_), Input::Text(t)) => {
                let a = t.trim().parse::<f64>().ok().filter(|a| a.is_finite()).ok_or_else(|| need("an angle"))?;
                return self.run(s, a);
            }
            _ => {}
        }
        Ok(Step::Continue)
    }
}

// ---------------- SWEEP ----------------

#[derive(Default)]
pub struct SweepM {
    sel: SelectPhase,
    picked: Option<Vec<Handle>>,
    path: SelectPhase,
}

impl Interactive for SweepM {
    fn name(&self) -> &'static str {
        "SWEEP"
    }
    fn begin(&mut self, s: &mut Session) -> Result<Step> {
        self.sel = SelectPhase::begin(s);
        if self.sel.done {
            self.picked = Some(self.sel.picked.clone());
        }
        self.path = SelectPhase::single();
        Ok(Step::Continue)
    }
    fn prompt(&self, _s: &Session) -> Prompt {
        if self.picked.is_none() { Prompt::new("Select objects to sweep", Accept::SELECT) } else { Prompt::new("Select sweep path", Accept::SELECT) }
    }
    fn input(&mut self, s: &mut Session, i: Input) -> Result<Step> {
        if matches!(i, Input::Cancel) {
            return Ok(Step::Cancel);
        }
        if self.picked.is_none() {
            match self.sel.feed(s, &i)? {
                SelOutcome::More => {}
                SelOutcome::Empty => return Ok(Step::Done),
                SelOutcome::Done(hs) => self.picked = Some(hs),
            }
            return Ok(Step::Continue);
        }
        match self.path.feed(s, &i)? {
            SelOutcome::More => Ok(Step::Continue),
            SelOutcome::Empty => Ok(Step::Done),
            SelOutcome::Done(hs) => {
                let path = hs.first().map(|h| h.hex()).ok_or_else(|| need("a path"))?;
                let p = json!({ "handles": hexes(self.picked.as_deref().unwrap_or(&[])), "path": path });
                let r = super::run_sweep(s, &p);
                s.set_selection(Vec::new());
                finish(s, r)
            }
        }
    }
}

// ---------------- LOFT ----------------

#[derive(Default)]
pub struct LoftM {
    sel: SelectPhase,
}

impl Interactive for LoftM {
    fn name(&self) -> &'static str {
        "LOFT"
    }
    fn prompt(&self, _s: &Session) -> Prompt {
        Prompt::new("Select cross sections in lofting order", Accept::SELECT)
    }
    fn input(&mut self, s: &mut Session, i: Input) -> Result<Step> {
        if matches!(i, Input::Cancel) {
            return Ok(Step::Cancel);
        }
        match self.sel.feed(s, &i)? {
            SelOutcome::More => Ok(Step::Continue),
            SelOutcome::Empty => Ok(Step::Done),
            SelOutcome::Done(hs) => {
                let r = super::run_loft(s, &json!({ "handles": hexes(&hs) }));
                s.set_selection(Vec::new());
                finish(s, r)
            }
        }
    }
}

// ---------------- PRESSPULL ----------------

#[derive(Default)]
pub struct PresspullM {
    at: Option<Vec2>,
}

impl Interactive for PresspullM {
    fn name(&self) -> &'static str {
        "PRESSPULL"
    }
    fn prompt(&self, s: &Session) -> Prompt {
        match self.at {
            None => Prompt::new("Click inside bounded areas to press or pull", Accept::POINT),
            Some(a) => height_prompt(s, "Specify extrusion height", Some(a)),
        }
    }
    fn input(&mut self, s: &mut Session, i: Input) -> Result<Step> {
        match (self.at, &i) {
            (_, Input::Cancel) => Ok(Step::Cancel),
            (None, Input::Point(p)) => {
                self.at = Some(*p);
                Ok(Step::Continue)
            }
            (None, Input::Enter) => Ok(Step::Done),
            (Some(a), _) => match height(s, &i, Some(a))? {
                Some(h) => {
                    let r = super::run_presspull(s, &json!({ "point": [a.x, a.y], "height": h }));
                    finish(s, r)
                }
                None => Ok(Step::Continue),
            },
            _ => Ok(Step::Continue),
        }
    }
}
