//! IMAGEATTACH: a raster image (PNG, JPEG) placed as an IMAGE object that refers to its file.
//! The file is named as given, so a path relative to the drawing's folder moves with the project.

use cadaddy_doc::{EntityKind, Image};
use cadaddy_geom::{Vec2, Vec3};
use serde_json::{Value, json};

use super::machines::{SelOutcome, SelectPhase};
use super::{CommandSpec, bad, f64_or, point_param, str_param};
use crate::{Accept, EngineError, Input, Interactive, Prompt, Result, Session, Step};

pub(super) fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new("imageattach", "Raster Image Reference...", run)
            .menu(&["Insert", "Raster Image Reference..."])
            .alias(&["iat"])
            .params("{path: PNG or JPEG file (a relative path is relative to this drawing's folder), at?: [x,y] (default 0,0), scale?: the image's width in drawing units (default 1), rotation? (degrees), pathType?: \"relative\" (default: a file in or below a saved drawing's folder is stored relative to it, so the project folder can move) | \"full\"} → {handle, name, path (as stored), pixels: [width, height]}")
            .interactive(|_| Ok(Box::new(ImageAttachM::default()))),
        CommandSpec::new("image", "Image", run_image_list)
            .params("{} → {images: [{name, path, found, references}]} (the app opens the External References palette)")
            .noundo(),
        CommandSpec::new("-image", "Image", run_dash_image)
            .params("{option: \"?\"|detach|path|reload|unload|attach, names?: \"A,B*\" (default *), path? (path), and IMAGEATTACH's parameters (attach)}")
            .interactive(|_| Ok(Box::new(DashImageM::default()))),
        CommandSpec::new("transparency", "Transparency", run_transparency)
            .menu(&["Modify", "Object", "Image", "Transparency"])
            .params("{handles? (default: the selection), on: bool} → {changed}: an image's transparent pixels show what's behind it (on) or the image is opaque (off)")
            .interactive(|_| Ok(Box::new(TransparencyM::default()))),
    ]
}

/// The drawing's images by name (an image's name, else its file name), with their references.
fn image_names(s: &Session) -> Result<Vec<(String, String, usize)>> {
    let d = s.doc()?;
    let mut out: Vec<(String, String, usize)> = Vec::new();
    let spaces = std::iter::once(&d.model).chain(d.layouts.iter().map(|l| &l.entities)).chain(d.blocks.values().map(|b| &b.entities));
    for e in spaces.flat_map(|st| st.iter()) {
        if let EntityKind::Image(i) = &e.kind {
            let name = if i.name.is_empty() { stem(&i.path) } else { i.name.clone() };
            match out.iter_mut().find(|(n, _, _)| n.eq_ignore_ascii_case(&name)) {
                Some(row) => row.2 += 1,
                None => out.push((name, i.path.clone(), 1)),
            }
        }
    }
    Ok(out)
}

fn run_image_list(s: &mut Session, _p: &Value) -> Result<Value> {
    let rows: Vec<Value> = image_names(s)?
        .into_iter()
        .map(|(name, path, n)| json!({ "name": name, "path": path, "found": cadaddy_render::images::get(&path).is_some(), "references": n }))
        .collect();
    Ok(json!({ "images": rows }))
}

/// Change every IMAGE whose name matches `pattern` (comma-separated wildcards) with `f`; or erase
/// them (`f` returns false). The number of images changed.
fn each_image(s: &mut Session, pattern: &str, mut f: impl FnMut(&mut Image) -> bool) -> Result<usize> {
    let pats: Vec<String> = pattern.split(',').map(|p| p.trim().to_string()).filter(|p| !p.is_empty()).collect();
    let matches = |i: &Image| {
        let name = if i.name.is_empty() { stem(&i.path) } else { i.name.clone() };
        pats.iter().any(|p| crate::cmd::qselect::wildcard(p, &name))
    };
    let d = s.doc_mut()?;
    let hs: Vec<cadaddy_doc::Handle> = std::iter::once(&d.model)
        .chain(d.layouts.iter().map(|l| &l.entities))
        .flat_map(|st| st.iter())
        .filter(|e| matches!(&e.kind, EntityKind::Image(i) if matches(i)))
        .map(|e| e.handle)
        .collect();
    let mut erase = Vec::new();
    for h in &hs {
        let mut keep = true;
        d.modify_entity(*h, |e| {
            if let EntityKind::Image(i) = &mut e.kind {
                keep = f(i);
            }
        })?;
        if !keep {
            erase.push(*h);
        }
    }
    for h in erase {
        d.remove_entity(h);
    }
    Ok(hs.len())
}

fn run_dash_image(s: &mut Session, p: &Value) -> Result<Value> {
    let names = str_param(p, "names").unwrap_or("*").to_string();
    let changed = match str_param(p, "option").unwrap_or("attach").to_ascii_lowercase().as_str() {
        "?" | "list" => return run_image_list(s, p),
        "attach" => return run(s, p),
        "detach" => each_image(s, &names, |_| false)?,
        "path" => {
            let path = str_param(p, "path").filter(|t| !t.trim().is_empty()).ok_or_else(|| bad("-image", "`path` is required"))?.to_string();
            each_image(s, &names, |i| {
                i.path = path.clone();
                true
            })?
        }
        "unload" => each_image(s, &names, |i| {
            i.display &= !1;
            true
        })?,
        "reload" => {
            cadaddy_render::images::revalidate();
            let paths: Vec<String> = image_names(s)?.into_iter().map(|(_, p, _)| p).collect();
            for p in &paths {
                cadaddy_render::images::forget(p);
            }
            each_image(s, &names, |i| {
                i.display |= 1;
                true
            })?
        }
        other => return Err(bad("-image", format!("unknown option `{other}`: ?, detach, path, reload, unload or attach"))),
    };
    if changed == 0 {
        return Err(bad("-image", format!("no image matches \"{names}\"")));
    }
    Ok(json!({ "changed": changed }))
}

fn run_transparency(s: &mut Session, p: &Value) -> Result<Value> {
    let on = p.get("on").and_then(Value::as_bool).ok_or_else(|| bad("transparency", "`on` (true or false) is required"))?;
    let hs = super::targets(s, p)?;
    let d = s.doc_mut()?;
    let mut changed = 0;
    for h in hs {
        if matches!(d.entity(h).map(|e| &e.kind), Some(EntityKind::Image(_))) {
            d.modify_entity(h, |e| {
                if let EntityKind::Image(i) = &mut e.kind {
                    i.display = if on { i.display | 8 } else { i.display & !8 };
                }
            })?;
            changed += 1;
        }
    }
    if changed == 0 {
        return Err(bad("transparency", "select one or more images"));
    }
    Ok(json!({ "changed": changed }))
}

/// TRANSPARENCY started: select the images, then ON or OFF.
#[derive(Default)]
struct TransparencyM {
    sel: SelectPhase,
    objs: Vec<cadaddy_doc::Handle>,
}

impl Interactive for TransparencyM {
    fn name(&self) -> &'static str {
        "TRANSPARENCY"
    }
    fn begin(&mut self, s: &mut Session) -> Result<Step> {
        self.sel = SelectPhase::begin(s);
        if self.sel.done {
            self.objs = self.sel.picked.clone();
        }
        Ok(Step::Continue)
    }
    fn prompt(&self, _s: &Session) -> Prompt {
        if !self.sel.done {
            return self.sel.prompt();
        }
        Prompt::new("Enter transparency mode", Accept::TEXT).kw(&["ON", "OFF"]).default("OFF")
    }
    fn input(&mut self, s: &mut Session, i: Input) -> Result<Step> {
        if !self.sel.done {
            return match self.sel.feed(s, &i)? {
                SelOutcome::More => Ok(Step::Continue),
                SelOutcome::Empty => Ok(Step::Done),
                SelOutcome::Done(hs) => {
                    self.objs = hs;
                    Ok(Step::Continue)
                }
            };
        }
        let on = match &i {
            Input::Cancel => return Ok(Step::Cancel),
            Input::Enter => false,
            Input::Text(t) | Input::Keyword(t) => match t.trim().to_ascii_lowercase().as_str() {
                "on" => true,
                "off" | "" => false,
                _ => return Err(EngineError::Other("Invalid option keyword.".into())),
            },
            _ => return Ok(Step::Continue),
        };
        let handles: Vec<u64> = self.objs.iter().map(|h| h.0).collect();
        run_transparency(s, &json!({ "handles": handles, "on": on }))?;
        Ok(Step::Done)
    }
}

/// -IMAGE typed: the option, then the image name(s), then (Path) the new path; Attach goes on as
/// IMAGEATTACH.
#[derive(Default)]
struct DashImageM {
    option: Option<String>,
    names: Option<String>,
    attach: Option<ImageAttachM>,
}

impl Interactive for DashImageM {
    fn name(&self) -> &'static str {
        "-IMAGE"
    }
    fn prompt(&self, s: &Session) -> Prompt {
        if let Some(m) = &self.attach {
            return m.prompt(s);
        }
        match (&self.option, &self.names) {
            (None, _) => Prompt::new("Enter image option", Accept::TEXT).kw(&["?", "Detach", "Path", "Reload", "Unload", "Attach"]).default("Attach"),
            (Some(_), None) => Prompt::new("Enter image(s) to list or change", Accept::TEXT).default("*"),
            (Some(_), Some(_)) => Prompt::new("Enter new path", Accept::TEXT),
        }
    }
    fn input(&mut self, s: &mut Session, i: Input) -> Result<Step> {
        if let Some(m) = &mut self.attach {
            return m.input(s, i);
        }
        if i == Input::Cancel {
            return Ok(Step::Cancel);
        }
        let text = match &i {
            Input::Text(t) | Input::Keyword(t) => t.trim().to_string(),
            Input::Enter => String::new(),
            _ => return Ok(Step::Continue),
        };
        match (&self.option, &self.names) {
            (None, _) => {
                let opt = if text.is_empty() { "attach".to_string() } else { text.to_ascii_lowercase() };
                if opt == "attach" || opt == "a" {
                    self.attach = Some(ImageAttachM::default());
                } else {
                    self.option = Some(opt);
                }
                Ok(Step::Continue)
            }
            (Some(opt), None) => {
                let names = if text.is_empty() { "*".to_string() } else { text };
                if opt == "path" || opt == "p" {
                    self.names = Some(names);
                    return Ok(Step::Continue);
                }
                let option = match opt.as_str() {
                    "?" => "?",
                    "d" | "detach" => "detach",
                    "r" | "reload" => "reload",
                    "u" | "unload" => "unload",
                    _ => return Err(EngineError::Other("Invalid option keyword.".into())),
                };
                let r = run_dash_image(s, &json!({ "option": option, "names": names }))?;
                if option == "?" {
                    for row in r["images"].as_array().into_iter().flatten() {
                        s.echo(format!(
                            "{}  {}{}",
                            row["name"].as_str().unwrap_or(""),
                            row["path"].as_str().unwrap_or(""),
                            if row["found"] == json!(true) { "" } else { "  (not found)" }
                        ));
                    }
                }
                Ok(Step::Done)
            }
            (Some(_), Some(names)) => {
                if text.is_empty() {
                    return Ok(Step::Cancel);
                }
                let names = names.clone();
                run_dash_image(s, &json!({ "option": "path", "names": names, "path": text.trim_matches('"') }))?;
                Ok(Step::Done)
            }
        }
    }
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
