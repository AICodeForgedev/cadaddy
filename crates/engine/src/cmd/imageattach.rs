//! IMAGEATTACH: a raster image (PNG, JPEG) placed as an IMAGE object that refers to its file.
//! The file is named as given, so a path relative to the drawing's folder moves with the project.

use cadaddy_doc::{EntityKind, Image};
use cadaddy_geom::{Vec2, Vec3};
use serde_json::{Value, json};

use super::{CommandSpec, bad, f64_or, point_param, str_param};
use crate::{Accept, EngineError, Input, Interactive, Prompt, Result, Session, Step};

pub(super) fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new("imageattach", "Raster Image Reference...", run)
            .menu(&["Insert", "Raster Image Reference..."])
            .alias(&["iat"])
            .params("{path: PNG or JPEG file (a relative path is relative to this drawing's folder), at?: [x,y] (default 0,0), scale?: the image's width in drawing units (default 1), rotation? (degrees), pathType?: \"relative\" (default: a file in or below a saved drawing's folder is stored relative to it, so the project folder can move) | \"full\"} → {handle, name, path (as stored), pixels: [width, height]}")
            .interactive(|_| Ok(Box::new(ImageAttachM::default()))),
    ]
}

/// The file name without folders and extension, as the image's name.
fn stem(path: &str) -> String {
    let file = path.rsplit(['/', '\\']).next().unwrap_or(path);
    match file.rsplit_once('.') {
        Some((stem, _)) if !stem.is_empty() => stem.to_string(),
        _ => file.to_string(),
    }
}

/// The pixel size of the image file `path` (looked up as drawing images are).
fn pixels(path: &str) -> Result<(u32, u32)> {
    cadaddy_render::images::get(path)
        .map(|r| (r.width, r.height))
        .ok_or_else(|| bad("imageattach", format!("{path} can't be read as an image (PNG or JPEG, found as given or beside the saved drawing)")))
}

/// The path an image is stored by: relative to the saved drawing's folder when the file is in or
/// below it (AutoCAD's default Relative path type), else as given.
fn stored_path(s: &Session, path: &str, full: bool) -> String {
    let host = s.state().ok().and_then(|st| st.path.clone());
    let dir = host.as_deref().and_then(|h| std::path::Path::new(h).parent()).and_then(|d| std::path::absolute(d).ok());
    let abs = std::path::Path::new(path);
    match (full, dir) {
        (false, Some(d)) if abs.is_absolute() => match abs.strip_prefix(&d) {
            Ok(rel) if !rel.as_os_str().is_empty() => rel.to_string_lossy().into_owned(),
            _ => path.to_string(),
        },
        _ => path.to_string(),
    }
}

/// Place the image `path` with its lower-left corner at `at`, `width` drawing units wide (square
/// pixels), turned `rotation` radians.
pub(crate) fn attach(s: &mut Session, path: &str, at: Vec2, width: f64, rotation: f64) -> Result<Value> {
    attach_as(s, path, at, width, rotation, false)
}

fn attach_as(s: &mut Session, path: &str, at: Vec2, width: f64, rotation: f64, full: bool) -> Result<Value> {
    if !(width.is_finite() && width > 0.0) {
        return Err(bad("imageattach", "`scale` must be a positive number"));
    }
    if !rotation.is_finite() {
        return Err(bad("imageattach", "`rotation` must be a number"));
    }
    let (w, h) = pixels(path)?;
    let px = width / f64::from(w);
    let (sin, cos) = rotation.sin_cos();
    let name = stem(path);
    let stored = stored_path(s, path, full);
    let image = Image {
        insert: Vec3::new(at.x, at.y, 0.0),
        u: Vec3::new(cos * px, sin * px, 0.0),
        v: Vec3::new(-sin * px, cos * px, 0.0),
        size: Vec2::new(f64::from(w), f64::from(h)),
        path: stored.clone(),
        name: name.clone(),
        ..Default::default()
    };
    let handle = s.add_entity(EntityKind::Image(image))?;
    Ok(json!({ "handle": handle.hex(), "name": name, "path": stored, "pixels": [w, h] }))
}

fn run(s: &mut Session, p: &Value) -> Result<Value> {
    let path = str_param(p, "path").filter(|p| !p.trim().is_empty()).ok_or_else(|| bad("imageattach", "`path` is required"))?;
    let at = point_param(p, "at").unwrap_or(Vec2::ZERO);
    let full = match str_param(p, "pathType").map(str::to_ascii_lowercase).as_deref() {
        None | Some("relative") => false,
        Some("full") => true,
        Some(other) => return Err(bad("imageattach", format!("`pathType` must be \"relative\" or \"full\", not \"{other}\""))),
    };
    attach_as(s, path, at, f64_or(p, "scale", 1.0), f64_or(p, "rotation", 0.0).to_radians(), full)
}

#[derive(Default)]
enum Stage {
    #[default]
    File,
    At,
    Scale,
    Rotation,
}

#[derive(Default)]
struct ImageAttachM {
    stage: Stage,
    path: String,
    at: Vec2,
    scale: f64,
}

impl Interactive for ImageAttachM {
    fn name(&self) -> &'static str {
        "IMAGEATTACH"
    }
    fn prompt(&self, _s: &Session) -> Prompt {
        match self.stage {
            Stage::File => Prompt::new("Enter name of image file to attach", Accept::TEXT),
            Stage::At => Prompt::new("Specify insertion point", Accept::POINT),
            Stage::Scale => Prompt::new("Specify scale factor", Accept::POINT_OR_NUMBER).default("1"),
            Stage::Rotation => Prompt::new("Specify rotation angle", Accept::POINT_OR_NUMBER).default("0"),
        }
    }
    fn input(&mut self, s: &mut Session, i: Input) -> Result<Step> {
        match (&self.stage, i) {
            (_, Input::Cancel) => Ok(Step::Cancel),
            (Stage::File, Input::Text(t)) => {
                let t = t.trim().trim_matches('"');
                if t.is_empty() {
                    return Ok(Step::Continue);
                }
                // Read it now, so a wrong name is asked again before the point.
                if let Err(e) = pixels(t) {
                    s.echo(match e {
                        EngineError::BadParams { msg, .. } => msg,
                        other => other.to_string(),
                    });
                    return Ok(Step::Continue);
                }
                self.path = t.to_string();
                self.stage = Stage::At;
                Ok(Step::Continue)
            }
            (Stage::File, Input::Enter) => Ok(Step::Cancel),
            (Stage::At, Input::Point(p)) => {
                self.at = p;
                self.stage = Stage::Scale;
                Ok(Step::Continue)
            }
            (Stage::Scale, i) => {
                let v = match i {
                    Input::Enter => Some(1.0),
                    Input::Point(p) => Some(self.at.dist(p)),
                    Input::Text(t) => crate::units::parse_distance(&t),
                    _ => None,
                };
                match v.filter(|v| v.is_finite() && *v > 0.0) {
                    Some(v) => {
                        self.scale = v;
                        self.stage = Stage::Rotation;
                    }
                    None => s.echo("Requires a positive number."),
                }
                Ok(Step::Continue)
            }
            (Stage::Rotation, i) => {
                let a = match i {
                    Input::Enter => Some(0.0),
                    Input::Point(p) => Some((p.y - self.at.y).atan2(p.x - self.at.x)),
                    Input::Text(t) => s.angle_settings().direction(&t),
                    _ => None,
                };
                let Some(a) = a else {
                    s.echo("Requires a valid angle.");
                    return Ok(Step::Continue);
                };
                let path = std::mem::take(&mut self.path);
                attach(s, &path, self.at, self.scale, a)?;
                Ok(Step::Done)
            }
            _ => Ok(Step::Continue),
        }
    }
}
