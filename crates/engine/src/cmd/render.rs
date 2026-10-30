//! RENDER, RENDERCROP and SAVEIMG: photoreal images of the current view by path tracing
//! ([`cadaddy_render::trace`]), lit by the drawing's sun, sky and lights through its materials.

use cadaddy_doc::Space;
use cadaddy_geom::Vec2;
use cadaddy_render::shade::{ShadeView, fit, scene};
use cadaddy_render::trace::{MAX_BOUNCES, MAX_SAMPLES, Preset, Render, TraceOptions, encode};
use serde_json::{Value, json};
use std::sync::Arc;

use super::{CommandSpec, bad, point_param, str_param};
use crate::{Accept, Input, Interactive, Prompt, RenderResult, Result, Session, Step};

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new("render", "Render", run_render)
            .menu(&["View", "Render", "Render"])
            .alias(&["rr"])
            .params("{preset?: draft|medium|high, samples? (1 to 4096), bounces?, width?, height? (pixels; the view's size), denoise? (true), output?: path (.png or .jpg), path?: same as output, crop?: [[x0,y0],[x1,y1]] (pixels)} → {width, height, samples, seconds, secondsPerSample, output?}")
            .noundo(),
        CommandSpec::new("rendercrop", "Render Region", run_rendercrop)
            .menu(&["View", "Render", "Render Region"])
            .params("{corner1: [x,y], corner2: [x,y] (on the view), preset?, samples?, output?, path?: same as output}")
            .noundo()
            .interactive(|_| Ok(Box::new(CropM::default()))),
        // Saved from the Render window (its Save button); scripts and agents give the path.
        CommandSpec::new("saveimg", "Save Image", run_saveimg).params("{path} (.png or .jpg: the last render)").noundo(),
        CommandSpec::new("renderenvironment", "Render Environment", run_renderenv)
            .menu(&["View", "Render", "Render Environment"])
            .alias(&["renderenv"])
            .params("{background?: {kind: sky|solid|gradient|image, color?, bottom?, top?, path? (image: PNG, JPEG, BMP, TIFF)}, fog?: {enabled, color?: [r,g,b] 0-1, near?, far? (distances in drawing units from the front of the model, along the view)}, ibl?: path (a Radiance .hdr file) | null, exposure?: stops, whiteBalance?: {temperature?: kelvin, tint?: -1 to 1}} → {environment}: the live view and RENDER show it"),
    ]
}

static CANCEL: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Ask the running RENDER (if any) to stop at its next sample pass. Safe to call from any thread;
/// the MCP server calls it when a `notifications/cancelled` or the `cancel` tool arrives.
pub fn request_cancel() {
    CANCEL.store(true, std::sync::atomic::Ordering::Relaxed);
}

/// Whether a cancel has been asked for and no render has taken it yet.
pub fn cancel_pending() -> bool {
    CANCEL.load(std::sync::atomic::Ordering::Relaxed)
}

/// Whether a cancel was requested; clears the request.
fn take_cancel() -> bool {
    CANCEL.swap(false, std::sync::atomic::Ordering::Relaxed)
}

fn other(m: impl Into<String>) -> crate::EngineError {
    crate::EngineError::Other(m.into())
}

/// The view a render sees: the current view direction and region of model space, at
/// `w` × `h` pixels (the view's own size unless given).
fn view_of(s: &Session, w: Option<u32>, h: Option<u32>) -> Result<(ShadeView, u32, u32)> {
    let st = s.state()?;
    let (vw, vh) = s.viewport_px;
    let aspect = if vw > 0.0 && vh > 0.0 { vw / vh } else { 1.5 };
    let (w, h) = match (w, h) {
        (Some(w), Some(h)) => (w, h),
        (Some(w), None) => (w, ((f64::from(w) / aspect).round() as u32).max(1)),
        (None, Some(h)) => (((f64::from(h) * aspect).round() as u32).max(1), h),
        (None, None) => ((vw.round() as u32).clamp(16, 1920), (vh.round() as u32).clamp(16, 1200)),
    };
    let (w, h) = (w.clamp(1, 8192), h.clamp(1, 8192));
    let proj = st.projection();
    let v = st.view();
    Ok((ShadeView::new(proj, v.center, v.height, w, h), w, h))
}

/// Read a sample count, bounce count and preset.
fn options(p: &Value) -> Result<(u32, TraceOptions)> {
    let preset = match str_param(p, "preset") {
        Some(n) => Preset::parse(n).ok_or_else(|| bad("render", "`preset` is draft, medium or high"))?,
        None => Preset::Medium,
    };
    let mut opts = TraceOptions::preset(preset);
    let samples = match p.get("samples").and_then(Value::as_u64) {
        Some(n) if (1..=u64::from(MAX_SAMPLES)).contains(&n) => n as u32,
        Some(_) => return Err(bad("render", format!("`samples` is 1 to {MAX_SAMPLES}"))),
        None if p.get("samples").is_some() => return Err(bad("render", "`samples` must be a whole number")),
        None => preset.samples(),
    };
    if let Some(b) = p.get("bounces") {
        opts.bounces =
            b.as_u64().filter(|b| *b <= u64::from(MAX_BOUNCES)).ok_or_else(|| bad("render", format!("`bounces` is 0 to {MAX_BOUNCES}")))? as u32;
    }
    if let Some(d) = p.get("denoise").and_then(Value::as_bool) {
        opts.denoise = d;
    }
    Ok((samples, opts))
}

fn size(p: &Value, key: &str) -> Result<Option<u32>> {
    match p.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(v) => {
            v.as_u64().filter(|n| (1..=8192).contains(n)).map(|n| Some(n as u32)).ok_or_else(|| bad("render", format!("`{key}` is 1 to 8192 pixels")))
        }
    }
}

/// Render the current view, keep the image (and write it to `output`).
fn render(s: &mut Session, p: &Value, crop: Option<(u32, u32, u32, u32)>) -> Result<Value> {
    take_cancel();
    let (samples, mut opts) = options(p)?;
    opts.crop = crop;
    let (view, w, h) = view_of(s, size(p, "width")?, size(p, "height")?)?;
    if let Some((x0, y0, x1, y1)) = crop
        && (x0 >= x1.min(w) || y0 >= y1.min(h))
    {
        return Err(bad("render", "the region is outside the view"));
    }
    let d = &s.state()?.doc;
    // Curves to about a pixel.
    let tol = view.height / f64::from(h) * 0.5;
    let sc = scene(d, &Space::Model, tol, cadaddy_color::Rgb(255, 255, 255));
    if sc.tris.is_empty() {
        return Err(other("There is nothing to render: no surfaces in the view."));
    }
    // A plan view with nothing set: frame the model.
    let view = if s.state()?.projection().is_none() { fit(&sc, None, w, h).unwrap_or(view) } else { view };
    let mut lit = sc.lighting.clone().unwrap_or_else(|| cadaddy_render::light::Lighting::of(d, &Space::Model));
    apply_environment(s, &mut lit)?;
    let mut r = Render::new(sc, view, lit, opts);
    for done in 0..samples {
        // Between sample passes: an MCP cancel stops the render within one pass.
        if take_cancel() {
            return Err(other(format!("Render cancelled after {done} of {samples} samples.")));
        }
        r.pass();
    }
    let image = Arc::new(r.image());
    let out = json!({ "width": w, "height": h, "samples": r.samples, "seconds": r.seconds, "secondsPerSample": r.seconds_per_sample() });
    let serial = s.last_render.as_ref().map_or(1, |l| l.serial + 1);
    s.last_render = Some(RenderResult { image, samples: r.samples, seconds: r.seconds, seconds_per_sample: r.seconds_per_sample(), serial });
    let mut out = out;
    if let Some(path) = str_param(p, "output").or_else(|| str_param(p, "path")) {
        save(s, path)?;
        if let Some(o) = out.as_object_mut() {
            o.insert("output".into(), json!(path));
        }
    }
    s.echo(format!("Rendered {w} × {h}, {samples} samples in {:.2} s ({:.1} ms a sample).", r.seconds, r.seconds_per_sample() * 1000.0));
    Ok(out)
}

fn run_render(s: &mut Session, p: &Value) -> Result<Value> {
    let crop = match p.get("crop").and_then(Value::as_array) {
        Some(a) => {
            let (Some(c1), Some(c2)) = (a.first().and_then(super::point_value), a.get(1).and_then(super::point_value)) else {
                return Err(bad("render", "`crop` is [[x0, y0], [x1, y1]] in pixels"));
            };
            let (lo, hi) = (Vec2::new(c1.x.min(c2.x), c1.y.min(c2.y)), Vec2::new(c1.x.max(c2.x), c1.y.max(c2.y)));
            if lo.x < 0.0 || lo.y < 0.0 || hi.x > 8192.0 || hi.y > 8192.0 {
                return Err(bad("render", "`crop` must be inside the image"));
            }
            Some((lo.x as u32, lo.y as u32, hi.x.ceil() as u32, hi.y.ceil() as u32))
        }
        None => None,
    };
    render(s, p, crop)
}

/// RENDERCROP: the region between two points of the current view, in its pixels.
fn run_rendercrop(s: &mut Session, p: &Value) -> Result<Value> {
    let (a, b) = (point_param(p, "corner1"), point_param(p, "corner2"));
    let (Some(a), Some(b)) = (a, b) else { return Err(bad("rendercrop", "`corner1` and `corner2` are required")) };
    let (view, w, h) = view_of(s, size(p, "width")?, size(p, "height")?)?;
    let px = view.height / f64::from(h);
    let to_px = |q: Vec2| Vec2::new((q.x - view.center.x) / px + f64::from(w) / 2.0, f64::from(h) / 2.0 - (q.y - view.center.y) / px);
    let (qa, qb) = (to_px(a), to_px(b));
    let clamp = |v: f64, m: u32| v.clamp(0.0, f64::from(m)) as u32;
    let crop = (clamp(qa.x.min(qb.x), w), clamp(qa.y.min(qb.y), h), clamp(qa.x.max(qb.x).ceil(), w), clamp(qa.y.max(qb.y).ceil(), h));
    render(s, p, Some(crop))
}

fn save(s: &Session, path: &str) -> Result<()> {
    let r = s.last_render.as_ref().ok_or_else(|| other("Nothing has been rendered yet."))?;
    let lower = path.to_ascii_lowercase();
    let jpeg = lower.ends_with(".jpg") || lower.ends_with(".jpeg");
    if !jpeg && !lower.ends_with(".png") {
        return Err(bad("saveimg", "the file must be .png or .jpg"));
    }
    let bytes = encode(&r.image, jpeg).ok_or_else(|| other("The image couldn't be encoded."))?;
    write(path, &bytes)
}

#[cfg(not(target_arch = "wasm32"))]
fn write(path: &str, bytes: &[u8]) -> Result<()> {
    crate::writefile::write(path, bytes).map_err(|e| bad("saveimg", format!("{path}: {e}")))
}

#[cfg(target_arch = "wasm32")]
fn write(_path: &str, _bytes: &[u8]) -> Result<()> {
    Err(other("Saving images isn't available in the browser."))
}

fn run_saveimg(s: &mut Session, p: &Value) -> Result<Value> {
    let path = str_param(p, "path").ok_or_else(|| bad("saveimg", "`path` is required"))?;
    save(s, path)?;
    Ok(json!({ "path": path }))
}

/// RENDERENVIRONMENT: set the background, fog, IBL, exposure and white balance.
fn run_renderenv(s: &mut Session, p: &Value) -> Result<Value> {
    use cadaddy_doc::lighting::{BackgroundKind, Fog, WhiteBalance};

    let mut env = s.state()?.render_environment.clone().unwrap_or_default();

    // Background.
    if let Some(bg) = p.get("background") {
        let kind = bg.get("kind").and_then(Value::as_str).unwrap_or("sky");
        env.background = match kind {
            "sky" => BackgroundKind::Sky,
            "solid" => {
                let c = parse_color3(bg, "color").unwrap_or([1.0, 1.0, 1.0]);
                BackgroundKind::Solid { color: c }
            }
            "gradient" => {
                let bottom = parse_color3(bg, "bottom").unwrap_or([0.2, 0.2, 0.3]);
                let top = parse_color3(bg, "top").unwrap_or([0.6, 0.7, 0.9]);
                BackgroundKind::Gradient { bottom, top }
            }
            "image" => {
                let path = bg.get("path").and_then(Value::as_str).unwrap_or("");
                if path.is_empty() {
                    return Err(bad("renderenvironment", "background.path is required for image backgrounds"));
                }
                if cadaddy_render::images::get(path).is_none() {
                    return Err(bad(
                        "renderenvironment",
                        format!("the background image `{path}` can't be read: give an image file that exists (PNG, JPEG, BMP or TIFF)"),
                    ));
                }
                BackgroundKind::Image { path: path.to_string() }
            }
            other => {
                return Err(bad("renderenvironment", format!("`background.kind` is sky, solid, gradient or image, not `{other}`")));
            }
        };
    }

    // Fog.
    if let Some(fg) = p.get("fog") {
        let enabled = fg.get("enabled").and_then(Value::as_bool).unwrap_or(true);
        let color = parse_color3(fg, "color").unwrap_or([1.0, 1.0, 1.0]);
        let near = fg.get("near").and_then(Value::as_f64).unwrap_or(env.fog.near);
        let far = fg.get("far").and_then(Value::as_f64).unwrap_or(env.fog.far);
        env.fog = Fog { enabled, color, near, far }.sane();
    }

    // IBL.
    if let Some(ibl) = p.get("ibl") {
        match ibl {
            Value::Null => {
                env.ibl = None;
            }
            Value::String(path) if path.is_empty() => {
                env.ibl = None;
            }
            Value::String(path) => {
                // Read now: a file that can't be used is said, not ignored at render time.
                hdr_map(path)?;
                env.ibl = Some(path.clone());
            }
            _ => return Err(bad("renderenvironment", "`ibl` is the path of a Radiance .hdr file, or null for the sky")),
        }
    }

    // Exposure.
    if let Some(exp) = p.get("exposure").and_then(Value::as_f64) {
        env.exposure = exp;
    }

    // White balance.
    if let Some(wb) = p.get("whiteBalance") {
        let temperature = wb.get("temperature").and_then(Value::as_f64).unwrap_or(env.white_balance.temperature);
        let tint = wb.get("tint").and_then(Value::as_f64).unwrap_or(env.white_balance.tint);
        env.white_balance = WhiteBalance { temperature, tint }.sane();
    }

    let env = env.sane();
    let out = serde_json::to_value(&env).unwrap_or(json!({}));
    s.state_mut()?.render_environment = Some(env);
    s.echo("Render environment updated.".to_string());
    Ok(json!({ "environment": out }))
}

/// Parse a [r, g, b] array (0–1) from a JSON value at a key.
fn parse_color3(p: &Value, key: &str) -> Option<[f64; 3]> {
    let a = p.get(key).and_then(Value::as_array)?;
    if a.len() < 3 {
        return None;
    }
    let r = a.first()?.as_f64()?;
    let g = a.get(1)?.as_f64()?;
    let b = a.get(2)?.as_f64()?;
    Some([r.clamp(0.0, 1.0), g.clamp(0.0, 1.0), b.clamp(0.0, 1.0)])
}

/// The last environment map read, by path (the live view applies the environment every frame).
static HDR: std::sync::Mutex<Option<(String, Arc<cadaddy_render::hdr::HdrImage>)>> = std::sync::Mutex::new(None);

/// The Radiance .hdr environment map at `path` (kept once read).
fn hdr_map(path: &str) -> Result<Arc<cadaddy_render::hdr::HdrImage>> {
    if let Some((p, m)) = HDR.lock().ok().and_then(|g| g.clone())
        && p == path
    {
        return Ok(m);
    }
    let bytes = read_hdr(path)?;
    let map = Arc::new(cadaddy_render::hdr::decode(&bytes).ok_or_else(|| {
        bad("renderenvironment", format!("`{path}` isn't a Radiance .hdr file (it starts \"#?RADIANCE\" and holds RGBE pixels): give one of those"))
    })?);
    if let Ok(mut g) = HDR.lock() {
        *g = Some((path.to_string(), Arc::clone(&map)));
    }
    Ok(map)
}

#[cfg(not(target_arch = "wasm32"))]
fn read_hdr(path: &str) -> Result<Vec<u8>> {
    std::fs::read(path).map_err(|e| bad("renderenvironment", format!("the lighting file `{path}` can't be read ({e}): give a Radiance .hdr file")))
}

#[cfg(target_arch = "wasm32")]
fn read_hdr(_path: &str) -> Result<Vec<u8>> {
    Err(bad("renderenvironment", "lighting files can't be read in the browser"))
}

/// Apply the current session's render environment to a [`Lighting`]: RENDER's, and the live
/// view's (GPU and CPU) every frame.
pub fn apply_environment(s: &Session, lit: &mut cadaddy_render::light::Lighting) -> Result<()> {
    let env = match &s.state()?.render_environment {
        Some(e) => e.clone().sane(),
        None => return Ok(()),
    };

    // Exposure: add the environment's offset to the drawing's.
    if env.exposure.abs() > 0.001 {
        lit.exposure *= 2f32.powf(env.exposure as f32);
    }

    // White balance.
    if !env.white_balance.is_neutral() {
        lit.white_balance = cadaddy_render::light::white_balance_rgb(env.white_balance.temperature, env.white_balance.tint);
    }

    // Fog.
    if env.fog.enabled {
        lit.fog = Some(cadaddy_render::light::FogParams {
            color: [env.fog.color[0] as f32, env.fog.color[1] as f32, env.fog.color[2] as f32],
            near: env.fog.near as f32,
            far: env.fog.far as f32,
        });
    }

    // Background.
    use cadaddy_doc::lighting::BackgroundKind;
    match &env.background {
        BackgroundKind::Sky => {}
        BackgroundKind::Solid { color } => {
            lit.background = cadaddy_render::light::Background::Solid([color[0] as f32, color[1] as f32, color[2] as f32]);
        }
        BackgroundKind::Gradient { bottom, top } => {
            lit.background = cadaddy_render::light::Background::Gradient {
                bottom: [bottom[0] as f32, bottom[1] as f32, bottom[2] as f32],
                top: [top[0] as f32, top[1] as f32, top[2] as f32],
            };
        }
        BackgroundKind::Image { path } => {
            if let Some(raster) = cadaddy_render::images::get(path) {
                lit.background = cadaddy_render::light::Background::Image(raster);
            }
        }
    }

    // IBL (image-based lighting from HDR): the path tracer's environment, and the ambient
    // term's sky. RENDERENVIRONMENT checked the file; one gone since is said here.
    if let Some(ibl_path) = &env.ibl {
        lit.set_env_map(hdr_map(ibl_path)?);
    }

    Ok(())
}

/// RENDERCROP at the prompts: two corners of the region.
#[derive(Default)]
struct CropM {
    first: Option<Vec2>,
}

impl Interactive for CropM {
    fn name(&self) -> &'static str {
        "RENDERCROP"
    }
    fn prompt(&self, _s: &Session) -> Prompt {
        match self.first {
            None => Prompt::new("Pick crop window to render", Accept::POINT),
            Some(f) => Prompt::new("Specify opposite corner", Accept::POINT).corner(f),
        }
    }
    fn input(&mut self, s: &mut Session, i: Input) -> Result<Step> {
        match (self.first, i) {
            (_, Input::Cancel) => Ok(Step::Cancel),
            (None, Input::Point(p)) => {
                self.first = Some(p);
                Ok(Step::Continue)
            }
            (Some(a), Input::Point(b)) if !a.near(b, 1e-12) => {
                // Nothing to render, or a region off the view: said, and the command ends.
                if let Err(e) = run_rendercrop(s, &json!({ "corner1": [a.x, a.y], "corner2": [b.x, b.y], "preset": "draft" })) {
                    return super::machines::end_with(s, &e);
                }
                Ok(Step::Done)
            }
            (_, Input::Enter) => Ok(Step::Done),
            _ => Ok(Step::Continue),
        }
    }
}
