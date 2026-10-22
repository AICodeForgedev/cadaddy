//! QVDRAWING and QVLAYOUT: quick views of the open drawings and of the current drawing's
//! layouts. The app shows them as a strip of thumbnails to switch with; the commands list them
//! (with thumbnails as PNG images in base64 when asked) and switch to one (`open`).

use cadaddy_doc::{Drawing, Space};
use cadaddy_geom::Vec2;
use cadaddy_render::raster::{self, RasterOptions};
use serde_json::{Value, json};

use super::*;
use crate::{Result, Session};

/// The largest thumbnail side, in pixels.
const MAX_SIDE: u32 = 1024;
/// At most this many thumbnails in one reply.
const MAX_THUMBS: usize = 64;

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new("qvdrawing", "Quick View Drawings", run_drawings)
            .params("{open?: index or title (switch to that drawing), thumbnails?: bool | {width, height}} → {drawings: [{index, title, path, current, layouts, thumbnail?}]}")
            .noundo(),
        CommandSpec::new("qvlayout", "Quick View Layouts", run_layouts)
            .params("{open?: layout name or \"Model\" (switch to it), thumbnails?: bool | {width, height}} → {layouts: [{name, current, thumbnail?}]}")
            .noundo(),
        CommandSpec::new("qvdrawingclose", "Close Quick View Drawings", |_, _| Ok(json!({}))).params("{} (the app hides the quick views)").noundo().transparent(),
        CommandSpec::new("qvlayoutclose", "Close Quick View Layouts", |_, _| Ok(json!({}))).params("{} (the app hides the quick views)").noundo().transparent(),
    ]
}

/// What a quick view shows of `space`: a layout's sheet, model space's extents.
fn frame(d: &Drawing, space: &Space) -> Option<cadaddy_geom::Bounds2> {
    let b = match space {
        Space::Paper(name) => cadaddy_render::sheet(d, name).map(|s| s.bounds())?,
        Space::Model => d.extents(&Space::Model),
    };
    (!b.is_empty() && b.width().is_finite() && b.height().is_finite()).then_some(b)
}

fn view(d: &Drawing, space: &Space, w: u32, h: u32) -> raster::View {
    let (center, scale) = match frame(d, space) {
        Some(b) => {
            let k = (f64::from(w) / b.width().max(1e-9)).min(f64::from(h) / b.height().max(1e-9)) * 0.92;
            (b.center(), if k.is_finite() && k > 0.0 { k } else { 1.0 })
        }
        None => (Vec2::ZERO, 1.0),
    };
    raster::View { center, scale, width: w, height: h }
}

/// A `w` × `h` picture of `space`: premultiplied RGBA pixels, row by row.
pub fn thumbnail_rgba(d: &Drawing, space: &Space, w: u32, h: u32) -> Option<Vec<u8>> {
    let (w, h) = (w.clamp(1, MAX_SIDE), h.clamp(1, MAX_SIDE));
    let list = cadaddy_render::build(d, space, &Default::default());
    raster::render(&list, &view(d, space, w, h), &RasterOptions::default()).map(|pm| pm.data().to_vec())
}

fn thumbnail_png(d: &Drawing, space: &Space, w: u32, h: u32) -> Option<String> {
    let list = cadaddy_render::build(d, space, &Default::default());
    raster::render_png(&list, &view(d, space, w, h), &RasterOptions::default()).map(|png| crate::cmd::file::base64_encode(&png))
}

/// The thumbnail size asked for: `true` is 160 × 120.
fn size(p: &Value) -> Option<(u32, u32)> {
    match p.get("thumbnails")? {
        Value::Bool(true) => Some((160, 120)),
        Value::Object(o) => {
            let n = |k: &str, d: u64| o.get(k).and_then(Value::as_u64).unwrap_or(d).clamp(16, u64::from(MAX_SIDE)) as u32;
            Some((n("width", 160), n("height", 120)))
        }
        _ => None,
    }
}

/// Model first, then the layouts in tab order.
fn spaces(d: &Drawing) -> Vec<Space> {
    let mut l: Vec<_> = d.layouts.iter().collect();
    l.sort_by_key(|l| l.tab_order);
    std::iter::once(Space::Model).chain(l.into_iter().map(|l| Space::Paper(l.name.clone()))).collect()
}

fn space_name(sp: &Space) -> String {
    match sp {
        Space::Model => "Model".into(),
        Space::Paper(n) => n.clone(),
    }
}

fn run_layouts(s: &mut Session, p: &Value) -> Result<Value> {
    if let Some(name) = str_param(p, "open").map(str::trim).filter(|n| !n.is_empty()) {
        super::run_layout_set(s, &json!({ "name": name }))?;
    }
    let thumbs = size(p);
    let current = s.layout_space();
    let d = s.doc()?;
    let list: Vec<Value> = spaces(d)
        .iter()
        .enumerate()
        .map(|(i, sp)| {
            let mut v = json!({ "name": space_name(sp), "current": *sp == current });
            if let Some((w, h)) = thumbs.filter(|_| i < MAX_THUMBS) {
                v["thumbnail"] = json!(thumbnail_png(d, sp, w, h));
            }
            v
        })
        .collect();
    Ok(json!({ "layouts": list }))
}

fn run_drawings(s: &mut Session, p: &Value) -> Result<Value> {
    match p.get("open") {
        None | Some(Value::Null) => {}
        Some(Value::Number(n)) => {
            crate::cmd::file::run_switch(s, &json!({ "index": n }))?;
        }
        Some(Value::String(t)) => {
            crate::cmd::file::run_switch(s, &json!({ "title": t }))?;
        }
        Some(_) => return Err(bad("qvdrawing", "`open` is a drawing's index or title")),
    }
    let thumbs = size(p);
    let list: Vec<Value> = s
        .docs
        .iter()
        .enumerate()
        .map(|(i, st)| {
            let d = st.drawing();
            let mut v = json!({
                "index": i,
                "title": st.title,
                "path": st.path,
                "current": i == s.active,
                "layouts": spaces(d).iter().map(space_name).collect::<Vec<_>>(),
            });
            if let Some((w, h)) = thumbs.filter(|_| i < MAX_THUMBS) {
                v["thumbnail"] = json!(thumbnail_png(d, &st.space, w, h));
            }
            v
        })
        .collect();
    Ok(json!({ "drawings": list }))
}
