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
            .params("{preset?: draft|medium|high, samples?, bounces?, width?, height? (pixels; the view's size), denoise? (true), output?: path (.png or .jpg), crop?: [[x0,y0],[x1,y1]] (pixels)} → {width, height, samples, seconds, secondsPerSample, output?}")
            .noundo(),
        CommandSpec::new("rendercrop", "Render Region", run_rendercrop)
            .menu(&["View", "Render", "Render Region"])
            .params("{corner1: [x,y], corner2: [x,y] (on the view), preset?, samples?, output?}")
            .noundo()
            .interactive(|_| Ok(Box::new(CropM::default()))),
        // Saved from the Render window (its Save button); scripts and agents give the path.
        CommandSpec::new("saveimg", "Save Image", run_saveimg).params("{path} (.png or .jpg: the last render)").noundo(),
    ]
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
    let lit = sc.lighting.clone().unwrap_or_else(|| cadaddy_render::light::Lighting::of(d, &Space::Model));
    let mut r = Render::new(sc, view, lit, opts);
    for _ in 0..samples {
        r.pass();
    }
    let image = Arc::new(r.image());
    let out = json!({ "width": w, "height": h, "samples": r.samples, "seconds": r.seconds, "secondsPerSample": r.seconds_per_sample() });
    let serial = s.last_render.as_ref().map_or(1, |l| l.serial + 1);
    s.last_render = Some(RenderResult { image, samples: r.samples, seconds: r.seconds, seconds_per_sample: r.seconds_per_sample(), serial });
    let mut out = out;
    if let Some(path) = str_param(p, "output") {
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
