//! XCLIP: clip block references and xrefs to a boundary. New boundary (Rectangular, Polygonal or a
//! selected closed polyline, optionally inverted), ON / OFF, Delete, and generate Polyline (the
//! boundary drawn as a polyline) and Clipdepth (front and back clipping planes, as distances in
//! front of and behind the boundary's plane; objects entirely beyond them are hidden). The boundary is kept in block coordinates
//! ([`cadaddy_doc::XClip`]), so it moves, turns and scales with the reference; XCLIPFRAME shows it.

use cadaddy_doc::{EntityKind, Handle, Insert, LwPolyline, XClip};
use cadaddy_geom::{PolyVertex, Vec2};
use serde_json::{Value, json};

use crate::cmd::machines::{SelOutcome, SelectPhase};
use crate::cmd::{CommandSpec, bad, bool_or, point_value, str_param, targets};
use crate::{Accept, EngineError, Input, Interactive, Prompt, Result, Session, Step};

/// Most boundary points a typed or JSON boundary may have.
const MAX_POINTS: usize = 100_000;

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new("xclip", "Xref", run)
            .menu(&["Modify", "Clip", "Xref"])
            .alias(&["xc"])
            .params(
                "{handles?, option?: \"new\"|\"on\"|\"off\"|\"delete\"|\"polyline\"|\"clipdepth\" (default new), boundary?: [[x,y]…] (world; two points = rectangle), \
                 polyline?: hex (a closed object to clip to), invert?: bool, front?: distance in front of the boundary (clipdepth), back?: distance behind it, remove?: bool (clipdepth: no planes)} → {changed, handles?}",
            )
            .interactive(|_| Ok(Box::new(XclipM::default()))),
    ]
}

/// The block references among `hs` in the current space, with their block's base point.
fn references(s: &Session, hs: &[Handle]) -> Result<Vec<(Handle, Insert, Vec2)>> {
    let d = s.doc()?;
    Ok(hs
        .iter()
        .filter_map(|h| {
            let e = d.entity(*h)?;
            let EntityKind::Insert(i) = &e.kind else { return None };
            let base = d.block(&i.block)?.base.xy();
            Some((*h, i.clone(), base))
        })
        .collect())
}

/// A world outline in a reference's block coordinates (`None` when it can't be inverted).
fn to_block(ins: &Insert, base: Vec2, world: &[Vec2]) -> Option<Vec<Vec2>> {
    let inv = ins.transform(base).inverse()?;
    let pts: Vec<Vec2> = world.iter().map(|p| inv.apply(*p)).collect();
    pts.iter().all(|p| p.is_finite()).then_some(pts)
}

/// A boundary from points: two are the corners of a rectangle, more a polygon.
fn outline(pts: &[Vec2]) -> Option<Vec<Vec2>> {
    let pts = crate::cmd::distinct_points(pts);
    match pts.as_slice() {
        [a, b] if (a.x - b.x).abs() > 1e-12 && (a.y - b.y).abs() > 1e-12 => Some(vec![*a, Vec2::new(b.x, a.y), *b, Vec2::new(a.x, b.y)]),
        p if p.len() >= 3 && cadaddy_geom::shoelace(p).abs() > 1e-18 => Some(pts),
        _ => None,
    }
}

fn set_clip(s: &mut Session, h: Handle, clip: Option<XClip>) -> Result<()> {
    s.doc_mut()?.modify_entity(h, |e| {
        if let EntityKind::Insert(i) = &mut e.kind {
            i.clip = clip;
        }
    })?;
    Ok(())
}

/// Clip every reference among `hs` to the world outline `world`.
fn clip_new(s: &mut Session, hs: &[Handle], world: &[Vec2], invert: bool) -> Result<usize> {
    let mut n = 0;
    for (h, ins, base) in references(s, hs)? {
        let Some(boundary) = to_block(&ins, base, world) else { continue };
        set_clip(s, h, Some(XClip { boundary, enabled: true, inverted: invert, front: None, back: None }))?;
        n += 1;
    }
    Ok(n)
}

/// ON / OFF / Delete / generate Polyline on every clipped reference among `hs`.
fn clip_option(s: &mut Session, hs: &[Handle], option: &str) -> Result<(usize, Vec<Handle>)> {
    let mut n = 0;
    let mut made = Vec::new();
    for (h, ins, base) in references(s, hs)? {
        let Some(c) = ins.clip.clone() else { continue };
        match option {
            "on" | "off" => set_clip(s, h, Some(XClip { enabled: option == "on", ..c }))?,
            "delete" => set_clip(s, h, None)?,
            _ => {
                let m = ins.transform(base);
                let vertices = c.boundary.iter().map(|p| PolyVertex { p: m.apply(*p), ..Default::default() }).collect();
                let pl = LwPolyline { vertices, closed: true, const_width: 0.0, elevation: 0.0, plinegen: false };
                made.push(s.add_entity(EntityKind::LwPolyline(pl))?);
            }
        }
        n += 1;
    }
    Ok((n, made))
}

/// XCLIP Clipdepth: set the front and back clipping planes (distances in front of and behind the
/// boundary; `None` leaves that plane off) of every clipped reference among `hs`.
fn clip_depth(s: &mut Session, hs: &[Handle], front: Option<f64>, back: Option<f64>) -> Result<usize> {
    if [front, back].iter().flatten().any(|v| !v.is_finite() || v.abs() > 1e12) {
        return Err(bad("xclip", "clip distances must be numbers"));
    }
    if let (Some(f), Some(b)) = (front, back)
        && f < -b
    {
        return Err(bad("xclip", "the front clipping plane must not be behind the back one"));
    }
    let mut n = 0;
    for (h, ins, _) in references(s, hs)? {
        let Some(c) = ins.clip.clone() else { continue };
        set_clip(s, h, Some(XClip { front, back: back.map(|b| -b), ..c }))?;
        n += 1;
    }
    Ok(n)
}

/// A clip distance parameter: absent or null = no plane.
fn distance(p: &Value, key: &str) -> Result<Option<f64>> {
    match p.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(v) => v.as_f64().map(Some).ok_or_else(|| bad("xclip", format!("`{key}` must be a distance"))),
    }
}

/// The outline of a closed object, as XCLIP's Select polyline takes it.
fn object_outline(s: &Session, h: Handle) -> Result<Vec<Vec2>> {
    let d = s.doc()?;
    let e = d.entity(h).ok_or_else(|| EngineError::Other(format!("no object {}", h.hex())))?;
    cadaddy_render::clip::boundary_polygon(&e.kind).ok_or_else(|| EngineError::Other("Select a closed polyline, circle, ellipse or spline.".into()))
}

fn run(s: &mut Session, p: &Value) -> Result<Value> {
    let hs = targets(s, p)?;
    if references(s, &hs)?.is_empty() {
        return Err(bad("xclip", "no block references selected"));
    }
    let option = str_param(p, "option").unwrap_or("new").trim().to_ascii_lowercase();
    match option.as_str() {
        "new" => {
            let world = match (p.get("boundary"), str_param(p, "polyline")) {
                (Some(Value::Array(a)), _) => {
                    if a.len() > MAX_POINTS {
                        return Err(bad("xclip", format!("`boundary` has more than {MAX_POINTS} points")));
                    }
                    let pts: Option<Vec<Vec2>> = a.iter().map(point_value).collect();
                    outline(&pts.ok_or_else(|| bad("xclip", "`boundary` must be [[x, y], …]"))?)
                        .ok_or_else(|| bad("xclip", "`boundary` needs two opposite corners or three or more points enclosing an area"))?
                }
                (_, Some(hex)) => object_outline(s, Handle::parse_hex(hex).ok_or_else(|| bad("xclip", "`polyline` must be an object handle"))?)?,
                _ => return Err(bad("xclip", "`boundary` or `polyline` is required for a new boundary")),
            };
            let n = clip_new(s, &hs, &world, bool_or(p, "invert", false))?;
            Ok(json!({ "changed": n }))
        }
        "clipdepth" | "depth" => {
            let (front, back) = if bool_or(p, "remove", false) { (None, None) } else { (distance(p, "front")?, distance(p, "back")?) };
            Ok(json!({ "changed": clip_depth(s, &hs, front, back)? }))
        }
        "on" | "off" | "delete" | "polyline" => {
            let (n, made) = clip_option(s, &hs, &option)?;
            let mut r = json!({ "changed": n });
            if !made.is_empty() {
                r["handles"] = json!(made.iter().map(|h| h.hex()).collect::<Vec<_>>());
            }
            Ok(r)
        }
        o => Err(bad("xclip", format!("`option` must be \"new\", \"on\", \"off\", \"clipdepth\", \"delete\" or \"polyline\", got \"{o}\""))),
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
enum Stage {
    #[default]
    Select,
    Option,
    /// Replace the existing boundary?
    Replace,
    /// Clipdepth: the front plane's distance, then the back plane's.
    Front,
    Back,
    Boundary,
    Corner1,
    Corner2,
    Polygon,
    Pick,
}

/// XCLIP typed: "Select objects:", "Enter clipping option [ON/OFF/Clipdepth/Delete/generate
/// Polyline/New boundary] <New>:", then for New "Specify clipping boundary or select invert
/// option: [Select polyline/Polygonal/Rectangular/Invert clip] <Rectangular>:" and its points.
#[derive(Default)]
struct XclipM {
    stage: Stage,
    sel: SelectPhase,
    objs: Vec<Handle>,
    invert: bool,
    points: Vec<Vec2>,
    front: Option<f64>,
}

impl XclipM {
    fn finish_new(&mut self, s: &mut Session, world: &[Vec2]) -> Result<Step> {
        let Some(world) = outline(world) else {
            s.echo("The boundary encloses no area.");
            return Ok(Step::Done);
        };
        clip_new(s, &self.objs, &world, self.invert)?;
        s.set_selection(Vec::new());
        Ok(Step::Done)
    }
}

impl Interactive for XclipM {
    fn name(&self) -> &'static str {
        "XCLIP"
    }
    fn begin(&mut self, s: &mut Session) -> Result<Step> {
        self.sel = SelectPhase::begin(s);
        if self.sel.done {
            return self.selected(s);
        }
        Ok(Step::Continue)
    }
    fn prompt(&self, _s: &Session) -> Prompt {
        match self.stage {
            Stage::Select => self.sel.prompt(),
            Stage::Option => Prompt::new("Enter clipping option", Accept::TEXT)
                .kw(&["ON", "OFF", "Clipdepth", "Delete", "generate Polyline", "New boundary"])
                .default("New"),
            Stage::Replace => Prompt::new("Delete old boundary(s)?", Accept::TEXT).kw(&["Yes", "No"]).default("Yes"),
            Stage::Front => Prompt::new("Specify front clip distance or [Remove]", Accept::NUMBER).kw(&["Remove"]),
            Stage::Back => Prompt::new("Specify back clip distance or [Remove]", Accept::NUMBER).kw(&["Remove"]),
            Stage::Boundary => Prompt::new(
                if self.invert {
                    "Inside mode - Objects inside boundary will be hidden.\nSpecify clipping boundary or select invert option"
                } else {
                    "Specify clipping boundary or select invert option"
                },
                Accept::TEXT,
            )
            .kw(&["Select polyline", "Polygonal", "Rectangular", "Invert clip"])
            .default("Rectangular"),
            Stage::Corner1 => Prompt::new("Specify first corner", Accept::POINT),
            Stage::Corner2 => Prompt::new("Specify opposite corner", Accept::POINT),
            Stage::Polygon if self.points.is_empty() => Prompt::new("Specify first point", Accept::POINT),
            Stage::Polygon => Prompt::new("Specify next point or [Undo]", Accept::POINT).kw(&["Undo"]).base_opt(self.points.last().copied()),
            Stage::Pick => Prompt::new("Select polyline", Accept::SELECT),
        }
    }
    fn preview(&self, _s: &Session, cursor: Vec2) -> Vec<EntityKind> {
        let pts: Vec<Vec2> = match (self.stage, self.points.first()) {
            (Stage::Corner2, Some(a)) => vec![*a, Vec2::new(cursor.x, a.y), cursor, Vec2::new(a.x, cursor.y)],
            (Stage::Polygon, Some(_)) => self.points.iter().copied().chain(std::iter::once(cursor)).collect(),
            _ => return Vec::new(),
        };
        let vertices = pts.into_iter().map(|p| PolyVertex { p, ..Default::default() }).collect();
        vec![EntityKind::LwPolyline(LwPolyline { vertices, closed: true, const_width: 0.0, elevation: 0.0, plinegen: false })]
    }
    fn input(&mut self, s: &mut Session, i: Input) -> Result<Step> {
        if i == Input::Cancel {
            return Ok(Step::Cancel);
        }
        let word = match &i {
            Input::Keyword(k) | Input::Text(k) => Some(k.trim().to_ascii_lowercase()),
            _ => None,
        };
        match self.stage {
            Stage::Select => match self.sel.feed(s, &i)? {
                SelOutcome::More => Ok(Step::Continue),
                SelOutcome::Empty => Ok(Step::Done),
                SelOutcome::Done(hs) => {
                    self.objs = hs;
                    self.selected(s)
                }
            },
            Stage::Option => {
                let option = match (word.as_deref(), &i) {
                    (_, Input::Enter) | (Some("n" | "new" | "new boundary"), _) => {
                        let clipped = references(s, &self.objs)?.iter().any(|(_, ins, _)| ins.clip.is_some());
                        self.stage = if clipped { Stage::Replace } else { Stage::Boundary };
                        return Ok(Step::Continue);
                    }
                    (Some("c" | "clipdepth"), _) => {
                        self.stage = Stage::Front;
                        return Ok(Step::Continue);
                    }
                    (Some("on"), _) => "on",
                    (Some("off"), _) => "off",
                    (Some("d" | "delete"), _) => "delete",
                    (Some("p" | "polyline" | "generate polyline"), _) => "polyline",
                    _ => {
                        s.echo("Invalid option keyword.");
                        return Ok(Step::Continue);
                    }
                };
                let (n, _) = clip_option(s, &self.objs, option)?;
                if n == 0 {
                    s.echo("No clipped references selected.");
                }
                s.set_selection(Vec::new());
                Ok(Step::Done)
            }
            Stage::Front | Stage::Back => {
                let value = match (word.as_deref(), &i) {
                    (Some("r" | "remove"), _) => {
                        clip_depth(s, &self.objs, None, None)?;
                        s.set_selection(Vec::new());
                        return Ok(Step::Done);
                    }
                    (_, Input::Enter) => None,
                    (Some(w), _) => match w.parse::<f64>().ok().filter(|v| v.is_finite()) {
                        Some(v) => Some(v),
                        None => {
                            s.echo("Requires a distance or the Remove option.");
                            return Ok(Step::Continue);
                        }
                    },
                    _ => return Ok(Step::Continue),
                };
                if self.stage == Stage::Front {
                    self.front = value;
                    self.stage = Stage::Back;
                    return Ok(Step::Continue);
                }
                if clip_depth(s, &self.objs, self.front, value)? == 0 {
                    s.echo("No clipped references selected.");
                }
                s.set_selection(Vec::new());
                Ok(Step::Done)
            }
            Stage::Replace => match (word.as_deref(), &i) {
                (_, Input::Enter) | (Some("y" | "yes"), _) => {
                    self.stage = Stage::Boundary;
                    Ok(Step::Continue)
                }
                (Some("n" | "no"), _) => Ok(Step::Done),
                _ => Ok(Step::Continue),
            },
            Stage::Boundary => {
                self.stage = match (word.as_deref(), &i) {
                    (_, Input::Enter) | (Some("r" | "rectangular"), _) => Stage::Corner1,
                    (Some("p" | "polygonal"), _) => Stage::Polygon,
                    (Some("s" | "select polyline"), _) => Stage::Pick,
                    (Some("i" | "invert clip"), _) => {
                        self.invert = !self.invert;
                        Stage::Boundary
                    }
                    (_, Input::Point(p)) => {
                        // A point starts a rectangle, as the default option.
                        self.points = vec![*p];
                        Stage::Corner2
                    }
                    _ => {
                        s.echo("Invalid option keyword.");
                        Stage::Boundary
                    }
                };
                Ok(Step::Continue)
            }
            Stage::Corner1 => {
                if let Input::Point(p) = i {
                    self.points = vec![p];
                    self.stage = Stage::Corner2;
                }
                Ok(Step::Continue)
            }
            Stage::Corner2 => match i {
                Input::Point(p) => {
                    let a = self.points.first().copied().unwrap_or(p);
                    self.finish_new(s, &[a, p])
                }
                _ => Ok(Step::Continue),
            },
            Stage::Polygon => match (word.as_deref(), i) {
                (Some("u" | "undo"), _) => {
                    self.points.pop();
                    Ok(Step::Continue)
                }
                (_, Input::Point(p)) if self.points.len() < MAX_POINTS => {
                    self.points.push(p);
                    Ok(Step::Continue)
                }
                (_, Input::Enter) if self.points.len() >= 3 => {
                    let pts = std::mem::take(&mut self.points);
                    self.finish_new(s, &pts)
                }
                (_, Input::Enter) => Ok(Step::Done),
                _ => Ok(Step::Continue),
            },
            Stage::Pick => match i {
                Input::Pick(hs) => {
                    let Some(h) = hs.first().copied() else { return Ok(Step::Continue) };
                    match object_outline(s, h) {
                        Ok(world) => self.finish_new(s, &world),
                        Err(e) => {
                            s.echo(e.to_string());
                            Ok(Step::Continue)
                        }
                    }
                }
                Input::Enter => Ok(Step::Done),
                _ => Ok(Step::Continue),
            },
        }
    }
}

impl XclipM {
    /// The objects are chosen: keep the block references and ask for the option.
    fn selected(&mut self, s: &mut Session) -> Result<Step> {
        if self.objs.is_empty() {
            self.objs = self.sel.picked.clone();
        }
        let refs: Vec<Handle> = references(s, &self.objs)?.into_iter().map(|r| r.0).collect();
        if refs.is_empty() {
            s.echo("No block references or xrefs selected.");
            return Ok(Step::Done);
        }
        self.objs = refs;
        self.stage = Stage::Option;
        Ok(Step::Continue)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cadaddy_doc::Common;

    fn session() -> (Session, Handle) {
        let mut s = Session::new();
        let d = s.doc_mut().unwrap();
        let mut b = cadaddy_doc::Block::new("GRID");
        b.entities.push(cadaddy_doc::Entity {
            handle: Handle(0x900),
            common: Common::default(),
            kind: EntityKind::Line(cadaddy_doc::Line { a: cadaddy_geom::Vec3::new(0.0, 0.0, 0.0), b: cadaddy_geom::Vec3::new(100.0, 100.0, 0.0) }),
        });
        d.blocks.insert("GRID".into(), std::sync::Arc::new(b));
        let ins = Insert {
            block: "GRID".into(),
            insert: cadaddy_geom::Vec3::new(200.0, 0.0, 0.0),
            scale: cadaddy_geom::Vec3::new(2.0, 2.0, 1.0),
            rotation: 0.0,
            attribs: Vec::new(),
            cols: 1,
            rows: 1,
            col_spacing: 0.0,
            row_spacing: 0.0,
            clip: None,
        };
        let h = s.add_entity(EntityKind::Insert(ins)).unwrap();
        (s, h)
    }

    fn clip(s: &Session, h: Handle) -> Option<XClip> {
        match &s.doc().unwrap().entity(h).unwrap().kind {
            EntityKind::Insert(i) => i.clip.clone(),
            _ => None,
        }
    }

    #[test]
    fn json_new_boundary_is_kept_in_block_coordinates_then_on_off_delete() {
        let (mut s, h) = session();
        // World 220..260 × 20..60 on a reference at 200,0 scaled 2: block 10..30 × 10..30.
        let r = s.execute("xclip", &json!({ "handles": [h.hex()], "boundary": [[220, 20], [260, 60]] })).unwrap();
        assert_eq!(r["changed"], 1);
        let c = clip(&s, h).unwrap();
        let b = cadaddy_geom::Bounds2::from_points(c.boundary.iter().copied());
        assert!((b.min.x - 10.0).abs() < 1e-9 && (b.max.y - 30.0).abs() < 1e-9, "{b:?}");
        s.execute("xclip", &json!({ "handles": [h.hex()], "option": "off" })).unwrap();
        assert!(!clip(&s, h).unwrap().enabled);
        let r = s.execute("xclip", &json!({ "handles": [h.hex()], "option": "polyline" })).unwrap();
        let ph = Handle::parse_hex(r["handles"][0].as_str().unwrap()).unwrap();
        assert!(
            matches!(&s.doc().unwrap().entity(ph).unwrap().kind, EntityKind::LwPolyline(p) if p.vertices.len() == 4 && (p.vertices[0].p.x - 220.0).abs() < 1e-9)
        );
        s.execute("xclip", &json!({ "handles": [h.hex()], "option": "delete" })).unwrap();
        assert!(clip(&s, h).is_none());
        // Bad input is refused, never a panic.
        assert!(s.execute("xclip", &json!({ "handles": [h.hex()], "boundary": [[1, 1], [1, 1]] })).is_err());
        assert!(s.execute("xclip", &json!({ "handles": [h.hex()], "boundary": "x" })).is_err());
        assert!(s.execute("xclip", &json!({ "handles": [h.hex()], "option": "zap", "boundary": [[0, 0], [1, 1]] })).is_err());
    }

    #[test]
    fn typed_rectangular_polygonal_and_polyline_boundaries() {
        let (mut s, h) = session();
        s.set_selection(vec![h]);
        s.cmdline("xclip").unwrap();
        s.cmdline("").unwrap();
        s.cmdline("r").unwrap();
        s.cmdline("220,20").unwrap();
        s.cmdline("260,60").unwrap();
        assert_eq!(clip(&s, h).map(|c| c.boundary.len()), Some(4));
        // A new boundary replaces it (after the question), inverted this time, as a polygon.
        s.set_selection(vec![h]);
        for t in ["xclip", "n", "y", "i", "p", "220,20", "260,20", "240,60", ""] {
            s.cmdline(t).unwrap();
        }
        let c = clip(&s, h).unwrap();
        assert!(c.inverted && c.boundary.len() == 3, "{c:?}");
        // Select polyline: a closed polyline's outline.
        let pl = s.execute("pline", &json!({ "points": [[210, 10], [290, 10], [290, 90], [210, 90]], "closed": true })).unwrap();
        let ph = pl["handle"].as_str().map(str::to_string).unwrap_or_default();
        s.execute("xclip", &json!({ "handles": [h.hex()], "polyline": ph })).unwrap();
        let c = clip(&s, h).unwrap();
        let b = cadaddy_geom::Bounds2::from_points(c.boundary.iter().copied());
        assert!((b.min.x - 5.0).abs() < 1e-9 && (b.max.x - 45.0).abs() < 1e-9 && !c.inverted, "{b:?}");
    }
}
