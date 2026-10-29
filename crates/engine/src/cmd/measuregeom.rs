//! MEASUREGEOM at the command line: pick what to measure (Distance by default), then the points or
//! the object it needs; each result is echoed and the option prompt comes back, as in AutoCAD.
//! Quick shows the distances to the nearest geometry left, right, below and above the cursor (and
//! the area of the closed object around it) as the cursor moves; a click echoes them. Distance,
//! Angle and Area label their running result on the drawing while you pick (MEASURELABELS).

use cadaddy_doc::{Drawing, EntityKind, Space};
use cadaddy_geom::{Bounds2, Vec2};
use serde_json::{Value, json};

use super::inquiry::live;
use crate::{Accept, Input, Interactive, LiveLabel, Prompt, Result, Session, Step};

#[derive(Clone, Copy, Debug, Default, PartialEq)]
enum Mode {
    #[default]
    Choose,
    Distance,
    Radius,
    Angle,
    Area,
    /// Volume: the Area points, then a height (a number, or two points whose distance is it).
    Volume,
    /// Volume's height prompt, the base polygon taken.
    VolumeHeight,
    Quick,
}

#[derive(Default)]
pub(super) struct MeasureM {
    mode: Mode,
    points: Vec<Vec2>,
    /// One measurement and done (the Angle tool: MEASUREGEOM's Angle on its own).
    once: bool,
    /// Volume: the polygon taken, and the first point of a height given by two points.
    base: Vec<Vec2>,
    height_from: Option<Vec2>,
}

const OPTIONS: &[&str] = &["Distance", "Radius", "Angle", "ARea", "Volume", "Quick", "eXit"];

impl MeasureM {
    /// The Angle tool: the angle at a vertex between two points, then done.
    pub(super) fn angle() -> Self {
        Self { mode: Mode::Angle, once: true, ..Default::default() }
    }
    /// The Volume tool: the volume of a polygon times a height, then done.
    pub(super) fn volume() -> Self {
        Self { mode: Mode::Volume, once: true, ..Default::default() }
    }
    /// Run the measurement, echo its result and keep its labels (MEASURELABELS 2); back to the
    /// option prompt.
    fn show(&mut self, s: &mut Session, cmd: &str, p: Value, labels: &[LiveLabel]) -> Result<Step> {
        let r = s.execute(cmd, &p);
        self.mode = Mode::Choose;
        self.points.clear();
        let r = r?;
        match r.get("message").and_then(Value::as_str) {
            Some(m) => m.lines().for_each(|l| s.echo(l.to_string())),
            None => s.echo(r.to_string()),
        }
        live::keep(s, labels);
        Ok(if self.once { Step::Done } else { Step::Continue })
    }
    fn pts(&self) -> Vec<[f64; 2]> {
        self.points.iter().map(|p| [p.x, p.y]).collect()
    }
}

impl Interactive for MeasureM {
    fn name(&self) -> &'static str {
        "MEASUREGEOM"
    }

    fn prompt(&self, _s: &Session) -> Prompt {
        let n = self.points.len();
        let last = self.points.last().copied();
        match self.mode {
            Mode::Choose => Prompt::new("Enter an option", Accept::TEXT).kw(OPTIONS).default("Distance"),
            Mode::Distance if n == 0 => Prompt::new("Specify first point", Accept::POINT),
            Mode::Distance => Prompt::new("Specify second point", Accept::POINT).base_opt(last),
            Mode::Radius => Prompt::new("Select arc or circle", Accept::SELECT),
            Mode::Angle => Prompt::new(
                ["Specify angle vertex", "Specify first angle endpoint", "Specify second angle endpoint"].get(n).copied().unwrap_or(""),
                Accept::POINT,
            ),
            Mode::Area if n == 0 => Prompt::new("Specify first corner point", Accept::POINT),
            Mode::Area => Prompt::new("Specify next point or press Enter to total", Accept::POINT).base_opt(last),
            Mode::Volume if n == 0 => Prompt::new("Specify first corner point", Accept::POINT),
            Mode::Volume => Prompt::new("Specify next point or press Enter to total", Accept::POINT).base_opt(last),
            Mode::VolumeHeight if self.height_from.is_none() => Prompt::new("Specify height", Accept::POINT_OR_NUMBER),
            Mode::VolumeHeight => Prompt::new("Specify second point of the height", Accept::POINT).base_opt(self.height_from),
            Mode::Quick => Prompt::new("Move cursor or", Accept::POINT).kw(&["Distance", "Radius", "Angle", "ARea", "Volume", "eXit"]).default("eXit"),
        }
    }

    fn input(&mut self, s: &mut Session, i: Input) -> Result<Step> {
        if i == Input::Cancel {
            return Ok(Step::Cancel);
        }
        if self.mode == Mode::Choose {
            let k = match &i {
                Input::Enter => "distance".to_string(),
                Input::Keyword(k) | Input::Text(k) => k.trim().to_ascii_lowercase(),
                _ => return Ok(Step::Continue),
            };
            self.mode = match k.as_str() {
                "d" | "distance" => Mode::Distance,
                "r" | "radius" => Mode::Radius,
                "a" | "angle" => Mode::Angle,
                "ar" | "area" => Mode::Area,
                "v" | "volume" => Mode::Volume,
                "q" | "quick" => Mode::Quick,
                "x" | "exit" => return Ok(Step::Done),
                _ => {
                    s.echo("Invalid option keyword.");
                    Mode::Choose
                }
            };
            return Ok(Step::Continue);
        }
        if self.mode == Mode::VolumeHeight {
            let height = match &i {
                Input::Enter => {
                    s.echo("A height is required.");
                    return Ok(Step::Continue);
                }
                Input::Point(p) => match self.height_from.take() {
                    None => {
                        self.height_from = Some(*p);
                        return Ok(Step::Continue);
                    }
                    Some(a) => a.dist(*p),
                },
                Input::Text(_) | Input::Keyword(_) if self.height_from.is_some() => {
                    s.echo("The second point of the height needs a point.");
                    return Ok(Step::Continue);
                }
                Input::Text(t) | Input::Keyword(t) => match super::machines::number(t).filter(|v| v.is_finite()) {
                    Some(h) => h,
                    None => {
                        s.echo("Requires a number or a point.");
                        return Ok(Step::Continue);
                    }
                },
                _ => return Ok(Step::Continue),
            };
            let pts: Vec<[f64; 2]> = self.base.iter().map(|p| [p.x, p.y]).collect();
            self.height_from = None;
            let r = self.show(s, "volume.measure", json!({ "points": pts, "height": height }), &[]);
            self.base.clear();
            return r;
        }
        if self.mode == Mode::Quick {
            return Ok(match i {
                Input::Point(p) => {
                    let msg = quick_at(s, p).map(|q| q.text(s)).unwrap_or_default();
                    msg.lines().for_each(|l| s.echo(l.to_string()));
                    Step::Continue
                }
                Input::Enter => Step::Done,
                Input::Keyword(k) | Input::Text(k) => {
                    // The other measurements from Quick's own prompt.
                    self.mode = Mode::Choose;
                    return self.input(s, Input::Keyword(k));
                }
                _ => Step::Continue,
            });
        }
        match (self.mode, i) {
            (Mode::Radius, Input::Pick(hs)) => match hs.first() {
                Some(h) => self.show(s, "radius.measure", json!({ "handle": h.hex() }), &[]),
                None => Ok(Step::Continue),
            },
            (_, Input::Point(p)) => {
                self.points.push(p);
                match (self.mode, self.points.as_slice()) {
                    (Mode::Distance, &[a, b]) => {
                        let labels = [live::distance(s, a, b, 1.0)];
                        self.show(s, "dist", json!({ "p1": [a.x, a.y], "p2": [b.x, b.y] }), &labels)
                    }
                    (Mode::Angle, &[v, a, b]) => {
                        let labels = live::angle(s, v, a, b);
                        self.show(s, "angle.measure", json!({ "vertex": [v.x, v.y], "p1": [a.x, a.y], "p2": [b.x, b.y] }), &labels)
                    }
                    _ => Ok(Step::Continue),
                }
            }
            (Mode::Volume, Input::Enter) if self.points.len() >= 3 => {
                self.base = std::mem::take(&mut self.points);
                self.mode = Mode::VolumeHeight;
                Ok(Step::Continue)
            }
            (Mode::Area, Input::Enter) if self.points.len() >= 3 => {
                let labels = live::area(s, &self.points, &[], None);
                let pts = self.pts();
                self.show(s, "area", json!({ "points": pts }), &labels)
            }
            (_, Input::Enter) if self.once => Ok(Step::Done),
            (_, Input::Enter) => {
                self.mode = Mode::Choose;
                self.points.clear();
                Ok(Step::Continue)
            }
            (_, Input::Text(_) | Input::Keyword(_)) => {
                s.echo("Point or option keyword required.");
                Ok(Step::Continue)
            }
            _ => Ok(Step::Continue),
        }
    }

    fn preview(&self, s: &Session, cursor: Vec2) -> Vec<EntityKind> {
        let line = super::helpers::line;
        match (self.mode, self.points.as_slice()) {
            (Mode::Quick, _) => {
                let (Ok(st), Some(q)) = (s.state(), quick_at(s, cursor)) else { return Vec::new() };
                q.preview(st.view().height / 60.0)
            }
            (Mode::Distance | Mode::Angle, &[a]) => vec![line(a, cursor)],
            (Mode::Angle, &[v, a]) => vec![line(v, a), line(v, cursor)],
            (Mode::Area | Mode::Volume, [_, ..]) => {
                let vertices = self.points.iter().chain([&cursor]).map(|p| cadaddy_geom::PolyVertex::new(*p)).collect();
                vec![super::helpers::lwpoly(vertices, true)]
            }
            _ => Vec::new(),
        }
    }

    fn labels(&self, s: &Session, cursor: Vec2) -> Vec<LiveLabel> {
        match (self.mode, self.points.as_slice()) {
            (Mode::Quick, _) => {
                let (Ok(st), Some(q)) = (s.state(), quick_at(s, cursor)) else { return Vec::new() };
                q.labels(s, st.view().height / 60.0)
            }
            (Mode::Distance, &[a]) => vec![live::distance(s, a, cursor, 1.0)],
            (Mode::Angle, &[v, a]) => live::angle(s, v, a, cursor),
            (Mode::Area | Mode::Volume, [_, ..]) => {
                let pts: Vec<Vec2> = self.points.iter().copied().chain([cursor]).collect();
                live::area(s, &pts, &[], None)
            }
            _ => Vec::new(),
        }
    }
}

/// What Quick measures around a point: the nearest geometry in each direction along the X and Y
/// axes, and the smallest closed object (circle, closed polyline, ellipse, hatch, region) the point
/// is inside.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Quick {
    pub at: Vec2,
    pub left: Option<Vec2>,
    pub right: Option<Vec2>,
    pub down: Option<Vec2>,
    pub up: Option<Vec2>,
    /// Area and perimeter of the closed object around the point.
    pub area: Option<(f64, f64)>,
}

/// Objects Quick ignores: what has no edges to measure to.
fn skipped(k: &EntityKind) -> bool {
    matches!(
        k,
        EntityKind::Text(_)
            | EntityKind::MText(_)
            | EntityKind::Insert(_)
            | EntityKind::Dimension(_)
            | EntityKind::AttDef(_)
            | EntityKind::Table(_)
            | EntityKind::Tolerance(_)
            | EntityKind::MLeader(_)
            | EntityKind::Image(_)
            | EntityKind::Point(_)
    )
}

/// Quick measurements at `at` among the visible objects of `space` within `reach` of it.
pub(crate) fn quick(d: &Drawing, space: &Space, at: Vec2, reach: f64) -> Quick {
    let mut q = Quick { at, ..Default::default() };
    if !(at.is_finite() && reach.is_finite() && reach > 0.0) {
        return q;
    }
    let ix = crate::spatial::index(d, space);
    let probe = Bounds2::new(at, at).expand(reach);
    let Some(cands) = crate::select::candidates(d, space, &ix, &probe, false, false) else { return q };
    let tol = reach / 2000.0;
    let eps = reach * 1e-9;
    let mut best = [f64::INFINITY; 4];
    for (e, known) in cands {
        if skipped(&e.kind) || !d.is_visible(e) || !crate::select::bounds_of(d, e, known).intersects(&probe) {
            continue;
        }
        let polys = crate::select::hit_polylines(d, e, tol);
        for w in polys.iter().flat_map(|p| p.windows(2)) {
            let [a, b] = [w[0], w[1]];
            // Crossing the horizontal line through the point (left and right).
            if (a.y - at.y) * (b.y - at.y) <= 0.0 && (a.y - b.y).abs() > f64::EPSILON {
                let x = a.x + (at.y - a.y) / (b.y - a.y) * (b.x - a.x);
                let dx = x - at.x;
                if dx > eps && dx < best[1] {
                    best[1] = dx;
                } else if dx < -eps && -dx < best[0] {
                    best[0] = -dx;
                }
            }
            // Crossing the vertical line (below and above).
            if (a.x - at.x) * (b.x - at.x) <= 0.0 && (a.x - b.x).abs() > f64::EPSILON {
                let y = a.y + (at.x - a.x) / (b.x - a.x) * (b.y - a.y);
                let dy = y - at.y;
                if dy > eps && dy < best[3] {
                    best[3] = dy;
                } else if dy < -eps && -dy < best[2] {
                    best[2] = -dy;
                }
            }
        }
        if let Ok((area, per)) = super::inquiry::kind_area(&e.kind)
            && q.area.is_none_or(|(a, _)| area < a)
            && encloses(&e.kind, &polys, at, tol)
        {
            q.area = Some((area, per));
        }
    }
    let hit = |d: f64, dir: Vec2| (d <= reach).then(|| at + dir * d);
    q.left = hit(best[0], Vec2::new(-1.0, 0.0));
    q.right = hit(best[1], Vec2::new(1.0, 0.0));
    q.down = hit(best[2], Vec2::new(0.0, -1.0));
    q.up = hit(best[3], Vec2::new(0.0, 1.0));
    q
}

/// Whether the closed object `k` (drawn as `polys`) has `at` inside it. A polyline's edges come
/// as separate pieces, so it is tested as one ring; otherwise any loop holding the point counts.
fn encloses(k: &EntityKind, polys: &[Vec<Vec2>], at: Vec2, tol: f64) -> bool {
    match k {
        EntityKind::Circle(c) => c.center.xy().dist(at) < c.radius,
        EntityKind::LwPolyline(pl) => {
            let ring = cadaddy_geom::Polyline { vertices: pl.vertices.clone(), closed: true }.tessellate(tol);
            cadaddy_geom::point_in_polygon(&ring, at)
        }
        _ => polys.iter().any(|p| cadaddy_geom::point_in_polygon(p, at)),
    }
}

/// Quick at `at` in the current space, reaching as far as the view is tall.
fn quick_at(s: &Session, at: Vec2) -> Option<Quick> {
    let st = s.state().ok()?;
    let reach = st.view().height.max(1e-9);
    Some(quick(&st.doc, &st.space, at, reach))
}

impl Quick {
    fn span(a: Option<Vec2>, b: Option<Vec2>) -> Option<f64> {
        Some(a?.dist(b?))
    }
    /// Distances from the point to each hit, and across the two axes when both sides hit.
    pub(crate) fn json(&self) -> Value {
        let d = |p: Option<Vec2>| p.map(|p| json!(self.at.dist(p)));
        json!({
            "left": d(self.left), "right": d(self.right), "down": d(self.down), "up": d(self.up),
            "horizontal": Self::span(self.left, self.right), "vertical": Self::span(self.down, self.up),
            "area": self.area.map(|a| a.0), "perimeter": self.area.map(|a| a.1),
        })
    }
    pub(crate) fn text(&self, s: &Session) -> String {
        let f = |v: f64| super::inquiry::fmt_d(s, v);
        let side = |p: Option<Vec2>| p.map_or("-".to_string(), |p| f(self.at.dist(p)));
        let mut t = format!("Left = {},  Right = {},  Down = {},  Up = {}", side(self.left), side(self.right), side(self.down), side(self.up));
        if let Some(h) = Self::span(self.left, self.right) {
            t.push_str(&format!("\nHorizontal = {}", f(h)));
        }
        if let Some(v) = Self::span(self.down, self.up) {
            t.push_str(&format!("\nVertical = {}", f(v)));
        }
        if let Some((a, per)) = self.area {
            t.push_str(&format!("\n{}", super::inquiry::area_text(s, a, per)));
        }
        t
    }
    /// Dimension-like lines to each hit, with ticks `h` long across their ends.
    fn preview(&self, h: f64) -> Vec<EntityKind> {
        let mut v = Vec::new();
        for (hit, across) in [(self.left, Vec2::Y), (self.right, Vec2::Y), (self.down, Vec2::X), (self.up, Vec2::X)] {
            let Some(p) = hit else { continue };
            v.push(super::helpers::line(self.at, p));
            // A tick across the end, as a dimension's.
            v.push(super::helpers::line(p - across * h * 0.5, p + across * h * 0.5));
        }
        v
    }
    /// The distance to each hit at the middle of its line, and the area and perimeter around the
    /// point below it (`h`: a text height in drawing units, to keep the two apart).
    fn labels(&self, s: &Session, h: f64) -> Vec<LiveLabel> {
        let mut v: Vec<LiveLabel> = [self.left, self.right, self.down, self.up]
            .into_iter()
            .flatten()
            .map(|p| LiveLabel::new((self.at + p) * 0.5).line("", super::inquiry::fmt_d(s, self.at.dist(p))))
            .collect();
        if let Some((a, per)) = self.area {
            let below = self.at + Vec2::new(h * 4.0, -h * 3.0);
            v.push(LiveLabel::new(below).line("Area", super::inquiry::fmt_area(s, a)).line("Perimeter", super::inquiry::fmt_d(s, per)));
        }
        v
    }
}
