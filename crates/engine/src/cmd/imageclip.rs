//! IMAGECLIP and IMAGEADJUST: clip a raster image to a rectangular or polygonal boundary (ON /
//! OFF / Delete / New boundary), and set its brightness, contrast and fade. The boundary is kept
//! as the IMAGE keeps it in a drawing file, in pixel coordinates (origin at the image's top-left
//! corner, y down; two points a rectangle, more a polygon), cut to the image's extent, so it
//! moves, turns and scales with the image.

use cadaddy_doc::{EntityKind, Handle, Image, LwPolyline};
use cadaddy_geom::{Bounds2, PolyVertex, Vec2};
use serde_json::{Value, json};

use crate::cmd::machines::{SelOutcome, SelectPhase, number};
use crate::cmd::{CommandSpec, bad, point_value, str_param, targets};
use crate::{Accept, Input, Interactive, Prompt, Result, Session, Step};

/// Most boundary points a typed or JSON boundary may have.
const MAX_POINTS: usize = 100_000;
/// IMAGE display flag: use the clip boundary.
const USE_CLIP: u16 = 4;

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new("imageclip", "Image", run_clip)
            .menu(&["Modify", "Clip", "Image"])
            .alias(&["icl"])
            .params(
                "{handles?, option?: \"new\"|\"on\"|\"off\"|\"delete\" (default new), boundary?: [[x,y]…] (world; two points = rectangle, \
                 more = polygon)} → {changed}",
            )
            .interactive(|_| Ok(Box::new(ClipM::default()))),
        CommandSpec::new("imageadjust", "Adjust...", run_adjust)
            .menu(&["Modify", "Object", "Image", "Adjust..."])
            .alias(&["iad", "-imageadjust"])
            .params("{handles?, brightness?: 0..100, contrast?: 0..100, fade?: 0..100} → {changed}")
            .interactive(|_| Ok(Box::new(AdjustM::default()))),
    ]
}

/// The raster images among `hs`.
fn images(s: &Session, hs: &[Handle]) -> Result<Vec<(Handle, Image)>> {
    let d = s.doc()?;
    Ok(hs
        .iter()
        .filter_map(|h| match &d.entity(*h)?.kind {
            EntityKind::Image(i) => Some((*h, i.clone())),
            _ => None,
        })
        .collect())
}

fn modify_image(s: &mut Session, h: Handle, f: impl FnOnce(&mut Image)) -> Result<()> {
    s.doc_mut()?.modify_entity(h, |e| {
        if let EntityKind::Image(i) = &mut e.kind {
            f(i);
        }
    })?;
    Ok(())
}

/// A world point in the image's pixel coordinates (as its clip boundary is kept); `None` when
/// the image's pixel vectors are degenerate.
fn to_pixels(i: &Image, p: Vec2) -> Option<Vec2> {
    let (o, u, v) = (i.insert.xy(), i.u.xy(), i.v.xy());
    let det = u.x * v.y - u.y * v.x;
    if !det.is_finite() || det.abs() < 1e-300 {
        return None;
    }
    let r = p - o;
    let a = (r.x * v.y - r.y * v.x) / det;
    let b = (u.x * r.y - u.y * r.x) / det;
    let q = Vec2::new(a - 0.5, i.size.y - b - 0.5);
    q.is_finite().then_some(q)
}

/// The image's extent in pixel coordinates (pixel corners).
fn extent(i: &Image) -> Bounds2 {
    Bounds2::new(Vec2::new(-0.5, -0.5), Vec2::new(i.size.x - 0.5, i.size.y - 0.5))
}

/// The pixel boundary for world points `world` (two opposite corners, or a polygon), cut to the
/// image; `None` when nothing of the image is left inside it.
fn pixel_boundary(i: &Image, world: &[Vec2]) -> Option<Vec<Vec2>> {
    if !(i.size.x.is_finite() && i.size.y.is_finite() && i.size.x > 0.0 && i.size.y > 0.0) {
        return None;
    }
    let px: Option<Vec<Vec2>> = world.iter().map(|p| to_pixels(i, *p)).collect();
    let px = crate::cmd::distinct_points(&px?);
    let ext = extent(i);
    let area = |p: &[Vec2]| cadaddy_geom::shoelace(p).abs() > 1e-12;
    match px.as_slice() {
        // Square to the image: two corners, kept as a rectangle.
        [a, b] if world.len() == 2 && (i.u.y.abs() < 1e-12 && i.v.x.abs() < 1e-12) => {
            let lo = Vec2::new(a.x.min(b.x).max(ext.min.x), a.y.min(b.y).max(ext.min.y));
            let hi = Vec2::new(a.x.max(b.x).min(ext.max.x), a.y.max(b.y).min(ext.max.y));
            (hi.x - lo.x > 1e-9 && hi.y - lo.y > 1e-9).then(|| vec![lo, hi])
        }
        // A rectangle picked on a turned image is a polygon in its pixels.
        [_, _] if world.len() == 2 => {
            let (wa, wb) = (world.first()?, world.get(1)?);
            pixel_boundary(i, &[*wa, Vec2::new(wb.x, wa.y), *wb, Vec2::new(wa.x, wb.y)])
        }
        // Inside the image: kept as picked.
        p if p.len() >= 3 && area(p) && p.iter().all(|q| ext.contains(*q)) => Some(p.to_vec()),
        p if p.len() >= 3 && area(p) => {
            let cut = cadaddy_render::clip::clip_polygon(p, &ext);
            let cut = crate::cmd::distinct_points(&cut);
            (cut.len() >= 3 && area(&cut)).then_some(cut)
        }
        _ => None,
    }
}

fn clip_new(s: &mut Session, hs: &[Handle], world: &[Vec2]) -> Result<usize> {
    let mut n = 0;
    for (h, img) in images(s, hs)? {
        let Some(clip) = pixel_boundary(&img, world) else { continue };
        modify_image(s, h, |i| {
            i.clip = clip;
            i.clipping = true;
            i.display |= USE_CLIP;
        })?;
        n += 1;
    }
    Ok(n)
}

/// ON / OFF / Delete on the images among `hs` that have a boundary of their own.
fn clip_option(s: &mut Session, hs: &[Handle], option: &str) -> Result<usize> {
    let mut n = 0;
    for (h, img) in images(s, hs)? {
        if img.clip.len() < 2 {
            continue;
        }
        modify_image(s, h, |i| match option {
            "on" => {
                i.clipping = true;
                i.display |= USE_CLIP;
            }
            "off" => {
                i.clipping = false;
                i.display &= !USE_CLIP;
            }
            _ => {
                i.clip.clear();
                i.clipping = false;
                i.display &= !USE_CLIP;
            }
        })?;
        n += 1;
    }
    Ok(n)
}

fn run_clip(s: &mut Session, p: &Value) -> Result<Value> {
    let hs = targets(s, p)?;
    if images(s, &hs)?.is_empty() {
        return Err(bad("imageclip", "no images selected"));
    }
    let option = str_param(p, "option").unwrap_or("new").trim().to_ascii_lowercase();
    match option.as_str() {
        "new" => {
            let Some(Value::Array(a)) = p.get("boundary") else {
                return Err(bad("imageclip", "`boundary` is required for a new boundary"));
            };
            if a.len() > MAX_POINTS {
                return Err(bad("imageclip", format!("`boundary` has more than {MAX_POINTS} points")));
            }
            let pts: Option<Vec<Vec2>> = a.iter().map(point_value).collect();
            let pts = pts.ok_or_else(|| bad("imageclip", "`boundary` must be [[x, y], …]"))?;
            let n = clip_new(s, &hs, &pts)?;
            if n == 0 {
                return Err(bad("imageclip", "`boundary` needs two opposite corners or three or more points enclosing part of the image"));
            }
            Ok(json!({ "changed": n }))
        }
        "on" | "off" | "delete" => Ok(json!({ "changed": clip_option(s, &hs, &option)? })),
        o => Err(bad("imageclip", format!("`option` must be \"new\", \"on\", \"off\" or \"delete\", got \"{o}\""))),
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
enum Stage {
    #[default]
    Select,
    Option,
    /// Replace the existing boundary?
    Replace,
    Kind,
    Corner1,
    Corner2,
    Polygon,
}

/// IMAGECLIP typed: "Select image to clip:", "Enter image clipping option [ON/OFF/Delete/New
/// boundary] <New>:", then for New "Enter clipping type [Polygonal/Rectangular] <Rectangular>:"
/// and its points.
#[derive(Default)]
struct ClipM {
    stage: Stage,
    sel: SelectPhase,
    image: Option<Handle>,
    points: Vec<Vec2>,
}

impl ClipM {
    fn finish_new(&mut self, s: &mut Session, world: &[Vec2]) -> Result<Step> {
        let hs: Vec<Handle> = self.image.into_iter().collect();
        if clip_new(s, &hs, world)? == 0 {
            s.echo("The boundary encloses none of the image.");
        }
        s.set_selection(Vec::new());
        Ok(Step::Done)
    }

    /// The object is chosen: it must be an image.
    fn selected(&mut self, s: &mut Session, hs: &[Handle]) -> Result<Step> {
        match images(s, hs)?.first() {
            Some((h, _)) => {
                self.image = Some(*h);
                self.stage = Stage::Option;
                Ok(Step::Continue)
            }
            None => {
                s.echo("The object is not an image.");
                self.sel = SelectPhase::single();
                self.stage = Stage::Select;
                Ok(Step::Continue)
            }
        }
    }

    fn clipped(&self, s: &Session) -> bool {
        let hs: Vec<Handle> = self.image.into_iter().collect();
        images(s, &hs).is_ok_and(|v| v.iter().any(|(_, i)| i.clip.len() >= 2))
    }
}

impl Interactive for ClipM {
    fn name(&self) -> &'static str {
        "IMAGECLIP"
    }
    fn begin(&mut self, s: &mut Session) -> Result<Step> {
        let pre = s.selection();
        if s.settings.pickfirst
            && let [h] = pre.as_slice()
            && !images(s, &[*h])?.is_empty()
        {
            return self.selected(s, &[*h]);
        }
        self.sel = SelectPhase::single();
        Ok(Step::Continue)
    }
    fn prompt(&self, _s: &Session) -> Prompt {
        match self.stage {
            Stage::Select => Prompt::new("Select image to clip", Accept::SELECT),
            Stage::Option => Prompt::new("Enter image clipping option", Accept::TEXT).kw(&["ON", "OFF", "Delete", "New boundary"]).default("New"),
            Stage::Replace => Prompt::new("Delete old boundary?", Accept::TEXT).kw(&["Yes", "No"]).default("Yes"),
            Stage::Kind => Prompt::new("Enter clipping type", Accept::TEXT).kw(&["Polygonal", "Rectangular"]).default("Rectangular"),
            Stage::Corner1 => Prompt::new("Specify first corner point", Accept::POINT),
            Stage::Corner2 => Prompt::new("Specify opposite corner point", Accept::POINT),
            Stage::Polygon if self.points.is_empty() => Prompt::new("Specify first point", Accept::POINT),
            Stage::Polygon if self.points.len() < 3 => {
                Prompt::new("Specify next point or [Undo]", Accept::POINT).kw(&["Undo"]).base_opt(self.points.last().copied())
            }
            Stage::Polygon => {
                Prompt::new("Specify next point or [Close/Undo]", Accept::POINT).kw(&["Close", "Undo"]).base_opt(self.points.last().copied())
            }
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
                SelOutcome::Done(hs) => self.selected(s, &hs),
            },
            Stage::Option => {
                let option = match (word.as_deref(), &i) {
                    (_, Input::Enter) | (Some("n" | "new" | "new boundary"), _) => {
                        self.stage = if self.clipped(s) { Stage::Replace } else { Stage::Kind };
                        return Ok(Step::Continue);
                    }
                    (Some("on"), _) => "on",
                    (Some("off"), _) => "off",
                    (Some("d" | "delete"), _) => "delete",
                    _ => {
                        s.echo("Invalid option keyword.");
                        return Ok(Step::Continue);
                    }
                };
                let hs: Vec<Handle> = self.image.into_iter().collect();
                if clip_option(s, &hs, option)? == 0 {
                    s.echo("The image has no clipping boundary.");
                }
                s.set_selection(Vec::new());
                Ok(Step::Done)
            }
            Stage::Replace => match (word.as_deref(), &i) {
                (_, Input::Enter) | (Some("y" | "yes"), _) => {
                    self.stage = Stage::Kind;
                    Ok(Step::Continue)
                }
                (Some("n" | "no"), _) => Ok(Step::Done),
                _ => Ok(Step::Continue),
            },
            Stage::Kind => {
                self.stage = match (word.as_deref(), &i) {
                    (_, Input::Enter) | (Some("r" | "rectangular"), _) => Stage::Corner1,
                    (Some("p" | "polygonal"), _) => Stage::Polygon,
                    (_, Input::Point(p)) => {
                        // A point starts a rectangle, the default type.
                        self.points = vec![*p];
                        Stage::Corner2
                    }
                    _ => {
                        s.echo("Invalid option keyword.");
                        Stage::Kind
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
                (Some("c" | "close"), _) | (_, Input::Enter) if self.points.len() >= 3 => {
                    let pts = std::mem::take(&mut self.points);
                    self.finish_new(s, &pts)
                }
                (_, Input::Point(p)) if self.points.len() < MAX_POINTS => {
                    self.points.push(p);
                    Ok(Step::Continue)
                }
                (_, Input::Enter) => Ok(Step::Done),
                _ => Ok(Step::Continue),
            },
        }
    }
}

/// A 0..=100 setting from JSON: absent is `None`; anything but a number in range is refused.
fn percent(p: &Value, key: &str) -> Result<Option<u8>> {
    match p.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(v) => match v.as_f64().filter(|x| x.is_finite() && (0.0..=100.0).contains(x)) {
            Some(x) => Ok(Some(x.round() as u8)),
            None => Err(bad("imageadjust", format!("`{key}` must be a number from 0 to 100"))),
        },
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
enum Setting {
    #[default]
    Brightness,
    Contrast,
    Fade,
}

impl Setting {
    fn key(self) -> &'static str {
        match self {
            Setting::Brightness => "brightness",
            Setting::Contrast => "contrast",
            Setting::Fade => "fade",
        }
    }
    fn of(self, i: &Image) -> u8 {
        match self {
            Setting::Brightness => i.brightness,
            Setting::Contrast => i.contrast,
            Setting::Fade => i.fade,
        }
    }
}

fn adjust(s: &mut Session, hs: &[Handle], b: Option<u8>, c: Option<u8>, f: Option<u8>) -> Result<usize> {
    let imgs = images(s, hs)?;
    for (h, _) in &imgs {
        modify_image(s, *h, |i| {
            i.brightness = b.unwrap_or(i.brightness);
            i.contrast = c.unwrap_or(i.contrast);
            i.fade = f.unwrap_or(i.fade);
        })?;
    }
    Ok(imgs.len())
}

fn run_adjust(s: &mut Session, p: &Value) -> Result<Value> {
    let hs = targets(s, p)?;
    if images(s, &hs)?.is_empty() {
        return Err(bad("imageadjust", "no images selected"));
    }
    let (b, c, f) = (percent(p, "brightness")?, percent(p, "contrast")?, percent(p, "fade")?);
    if (b, c, f) == (None, None, None) {
        return Err(bad("imageadjust", "give `brightness`, `contrast` or `fade`"));
    }
    Ok(json!({ "changed": adjust(s, &hs, b, c, f)? }))
}

/// IMAGEADJUST typed (as -IMAGEADJUST): "Select image(s):", "Enter image adjustment option
/// [Contrast/Fade/Brightness] <Brightness>:", then "Enter brightness value (0-100) <50>:" with
/// the first image's value as the default.
#[derive(Default)]
struct AdjustM {
    sel: SelectPhase,
    imgs: Vec<Handle>,
    setting: Option<Setting>,
    current: Setting,
}

impl AdjustM {
    fn selected(&mut self, s: &mut Session, hs: &[Handle]) -> Result<Step> {
        self.imgs = images(s, hs)?.into_iter().map(|(h, _)| h).collect();
        if self.imgs.is_empty() {
            s.echo("No images selected.");
            return Ok(Step::Done);
        }
        Ok(Step::Continue)
    }
}

impl Interactive for AdjustM {
    fn name(&self) -> &'static str {
        "IMAGEADJUST"
    }
    fn begin(&mut self, s: &mut Session) -> Result<Step> {
        self.sel = SelectPhase::begin(s);
        if self.sel.done {
            let hs = self.sel.picked.clone();
            return self.selected(s, &hs);
        }
        Ok(Step::Continue)
    }
    fn prompt(&self, s: &Session) -> Prompt {
        if !self.sel.done {
            return Prompt::new(if self.sel.removing { "Remove objects" } else { "Select image(s)" }, Accept::SELECT);
        }
        match self.setting {
            None => Prompt::new("Enter image adjustment option", Accept::TEXT).kw(&["Contrast", "Fade", "Brightness"]).default(match self.current {
                Setting::Brightness => "Brightness",
                Setting::Contrast => "Contrast",
                Setting::Fade => "Fade",
            }),
            Some(k) => {
                let now = images(s, &self.imgs).ok().and_then(|v| v.first().map(|(_, i)| k.of(i))).unwrap_or(50);
                Prompt::new(format!("Enter {} value (0-100)", k.key()), Accept::NUMBER).default(now.to_string())
            }
        }
    }
    fn input(&mut self, s: &mut Session, i: Input) -> Result<Step> {
        if i == Input::Cancel {
            return Ok(Step::Cancel);
        }
        if !self.sel.done {
            return match self.sel.feed(s, &i)? {
                SelOutcome::More => Ok(Step::Continue),
                SelOutcome::Empty => Ok(Step::Done),
                SelOutcome::Done(hs) => self.selected(s, &hs),
            };
        }
        let word = match &i {
            Input::Keyword(k) | Input::Text(k) => Some(k.trim().to_ascii_lowercase()),
            _ => None,
        };
        match self.setting {
            None => {
                self.setting = match (word.as_deref(), &i) {
                    (_, Input::Enter) => Some(self.current),
                    (Some("b" | "brightness"), _) => Some(Setting::Brightness),
                    (Some("c" | "contrast"), _) => Some(Setting::Contrast),
                    (Some("f" | "fade"), _) => Some(Setting::Fade),
                    _ => {
                        s.echo("Invalid option keyword.");
                        None
                    }
                };
                Ok(Step::Continue)
            }
            Some(k) => {
                let v = match (&i, word.as_deref()) {
                    (Input::Enter, _) => {
                        s.set_selection(Vec::new());
                        return Ok(Step::Done);
                    }
                    (_, Some(t)) => number(t).filter(|v| v.is_finite() && (0.0..=100.0).contains(v)),
                    _ => None,
                };
                let Some(v) = v else {
                    s.echo("Requires an integer between 0 and 100.");
                    return Ok(Step::Continue);
                };
                let v = Some(v.round() as u8);
                match k {
                    Setting::Brightness => adjust(s, &self.imgs, v, None, None)?,
                    Setting::Contrast => adjust(s, &self.imgs, None, v, None)?,
                    Setting::Fade => adjust(s, &self.imgs, None, None, v)?,
                };
                s.set_selection(Vec::new());
                Ok(Step::Done)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cadaddy_geom::Vec3;

    /// A pixel point in world coordinates, as the renderer places it.
    fn to_world(i: &Image, q: Vec2) -> Vec2 {
        i.insert.xy() + i.u.xy() * (q.x + 0.5) + i.v.xy() * (i.size.y - (q.y + 0.5))
    }

    #[test]
    fn pixel_and_world_coordinates_invert() {
        let img = Image {
            insert: Vec3::new(10.0, 20.0, 0.0),
            u: Vec3::new(0.0, 2.0, 0.0),
            v: Vec3::new(-2.0, 0.0, 0.0),
            size: Vec2::new(40.0, 30.0),
            ..Image::default()
        };
        for q in [Vec2::new(-0.5, -0.5), Vec2::new(12.0, 7.25), Vec2::new(39.5, 29.5)] {
            let back = to_pixels(&img, to_world(&img, q)).unwrap();
            assert!(back.near(q, 1e-9), "{q:?} → {back:?}");
        }
        let flat = Image { u: Vec3::new(1.0, 0.0, 0.0), v: Vec3::new(2.0, 0.0, 0.0), ..img };
        assert!(to_pixels(&flat, Vec2::ZERO).is_none());
    }
}
