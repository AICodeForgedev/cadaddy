//! PNGOUT, JPGOUT, BMPOUT and TIFOUT: the selected objects as an image, as they look in the
//! current view (its size in pixels and its background colour), nothing else drawn.

use cadaddy_doc::{EntityStore, Handle};
use cadaddy_render::encode::{Depth, Format};
use serde_json::{Value, json};

use super::machines::{SelOutcome, SelectPhase};
use super::*;
use crate::{Accept, Input, Interactive, Prompt, Result, Session, Step};

/// Each command's format.
fn format_of(id: &str) -> Format {
    match id {
        "jpgout" => Format::Jpg,
        "bmpout" => Format::Bmp,
        "tifout" => Format::Tif,
        _ => Format::Png,
    }
}

/// Largest side of the image, pixels.
const MAX_SIDE: f64 = 16384.0;

const PARAMS: &str = "{path? (else returns base64 `data`), handles? (default: the selection), background?: \"#rrggbb\" (default white), size?: [w, h] pixels (default: the drawing area's), colorDepth?: 24|8|1} → {path | data, width, height, objects}: the objects as they look in the current view";

fn machine(id: &'static str, ext: &'static str) -> Box<dyn Interactive> {
    Box::new(OutM { id, ext, path: None, sel: SelectPhase::default() })
}

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new("pngout", "PNG Out", |s, p| run(s, p, "pngout")).params(PARAMS).noundo().interactive(|_| Ok(machine("pngout", "png"))),
        CommandSpec::new("jpgout", "JPEG Out", |s, p| run(s, p, "jpgout")).params(PARAMS).noundo().interactive(|_| Ok(machine("jpgout", "jpg"))),
        CommandSpec::new("bmpout", "BMP Out", |s, p| run(s, p, "bmpout")).params(PARAMS).noundo().interactive(|_| Ok(machine("bmpout", "bmp"))),
        CommandSpec::new("tifout", "TIFF Out", |s, p| run(s, p, "tifout")).params(PARAMS).noundo().interactive(|_| Ok(machine("tifout", "tif"))),
    ]
}

fn run(s: &mut Session, p: &Value, id: &'static str) -> Result<Value> {
    let format = format_of(id);
    let hs = targets(s, p)?;
    let st = s.state()?;
    let space = st.space.clone();
    let d = &st.doc;
    // Only objects in this space that exist.
    let store = d.space(&space).ok_or_else(|| bad(id, "nothing to draw here"))?;
    let picked: Vec<Handle> = hs.into_iter().filter(|h| store.contains(*h)).collect();
    if picked.is_empty() {
        return Err(bad(id, "select the objects to export (none is selected)"));
    }
    let bg = match str_param(p, "background") {
        Some(h) => cadaddy_color::Rgb::parse_hex(h).ok_or_else(|| bad(id, "`background` must be \"#rrggbb\""))?,
        None => cadaddy_color::Rgb(255, 255, 255),
    };
    let (w, h) = match p.get("size").filter(|v| !v.is_null()) {
        Some(v) => {
            let a = v.as_array().filter(|a| a.len() == 2).ok_or_else(|| bad(id, "`size` must be [width, height] in pixels"))?;
            let n = |x: &Value| {
                x.as_f64()
                    .filter(|n| n.is_finite() && *n >= 1.0 && *n <= MAX_SIDE)
                    .ok_or_else(|| bad(id, format!("`size`: 1 to {MAX_SIDE} pixels a side")))
            };
            (n(&a[0])?, n(&a[1])?)
        }
        None => (s.viewport_px.0.clamp(1.0, MAX_SIDE), s.viewport_px.1.clamp(1.0, MAX_SIDE)),
    };
    let depth = match p.get("colorDepth").filter(|v| !v.is_null()) {
        Some(v) => match v.as_u64() {
            Some(b @ (24 | 8 | 1)) => Depth::from_bits(b),
            _ => return Err(bad(id, "`colorDepth` must be 24, 8 or 1")),
        },
        None => Depth::Color,
    };
    // A copy of the drawing holding only the chosen objects in this space (tables, blocks and
    // styles kept for drawing them).
    let mut only = (**d).clone();
    let mut kept = EntityStore::new();
    for h in &picked {
        if let Some(e) = store.get(*h) {
            kept.push((**e).clone());
        }
    }
    if let Some(sp) = only.space_mut(&space) {
        *sp = kept;
    }
    let view = st.view();
    let scale = h / view.height.max(1e-12);
    let list = cadaddy_render::build(&only, &space, &cadaddy_render::Options { tolerance: (0.25 / scale).max(1e-9), ..Default::default() });
    let rv = cadaddy_render::raster::View { center: view.center, scale, width: w.round() as u32, height: h.round() as u32 };
    let ro = cadaddy_render::raster::RasterOptions { background: bg, hairline: 1.0, px_per_mm: 0.0, antialias: true };
    let bytes =
        cadaddy_render::raster::render_image(&list, &rv, &ro, false, format, depth, 96.0).ok_or_else(|| bad(id, "the image could not be made"))?;
    let out = json!({ "width": rv.width, "height": rv.height, "objects": picked.len(), "bytes": bytes.len() });
    match str_param(p, "path") {
        Some(path) => {
            super::exchange::write_file(id, path, &bytes)?;
            let mut out = out;
            out["path"] = json!(path);
            out["message"] = json!(format!("{} object(s) written to {path} ({} × {} pixels).", picked.len(), rv.width, rv.height));
            Ok(out)
        }
        None => {
            let mut out = out;
            out["data"] = json!(super::file::base64_encode(&bytes));
            Ok(out)
        }
    }
}

/// Typed: the file name, then the objects.
struct OutM {
    id: &'static str,
    ext: &'static str,
    path: Option<String>,
    sel: SelectPhase,
}

impl OutM {
    /// The drawing's name with this format's extension, beside it (or just the name).
    fn default_path(&self, s: &Session) -> String {
        let (stem, dir) = match s.state().ok().and_then(|st| st.path.clone()) {
            Some(p) => {
                let p = std::path::PathBuf::from(p);
                (p.file_stem().map(|x| x.to_string_lossy().into_owned()).unwrap_or_else(|| "Drawing".into()), p.parent().map(|d| d.to_path_buf()))
            }
            None => (s.state().map(|st| st.title.clone()).unwrap_or_else(|_| "Drawing".into()), None),
        };
        let name = format!("{stem}.{}", self.ext);
        dir.map_or(name.clone(), |d| d.join(&name).to_string_lossy().into_owned())
    }
}

impl Interactive for OutM {
    fn name(&self) -> &'static str {
        match self.id {
            "pngout" => "PNGOUT",
            "jpgout" => "JPGOUT",
            "bmpout" => "BMPOUT",
            _ => "TIFOUT",
        }
    }
    fn prompt(&self, s: &Session) -> Prompt {
        match self.path {
            None => Prompt::new("Enter image file name", Accept::TEXT).default(self.default_path(s)),
            Some(_) => self.sel.prompt(),
        }
    }
    fn input(&mut self, s: &mut Session, i: Input) -> Result<Step> {
        if i == Input::Cancel {
            return Ok(Step::Cancel);
        }
        if self.path.is_none() {
            let typed = match i {
                Input::Text(t) => t.trim().trim_matches('"').to_string(),
                Input::Enter => String::new(),
                _ => return Ok(Step::Continue),
            };
            let mut path = if typed.is_empty() { self.default_path(s) } else { typed };
            if std::path::Path::new(&path).extension().is_none() {
                path = format!("{path}.{}", self.ext);
            }
            self.path = Some(path);
            return Ok(Step::Continue);
        }
        match self.sel.feed(s, &i)? {
            SelOutcome::More => Ok(Step::Continue),
            SelOutcome::Empty => {
                s.echo("Nothing selected: no image written.");
                Ok(Step::Done)
            }
            SelOutcome::Done(hs) => {
                let handles: Vec<String> = hs.iter().map(|h| h.hex()).collect();
                let r = run(s, &json!({ "path": self.path, "handles": handles }), self.id)?;
                if let Some(m) = r.get("message").and_then(Value::as_str) {
                    s.echo(m.to_string());
                }
                Ok(Step::Done)
            }
        }
    }
}
