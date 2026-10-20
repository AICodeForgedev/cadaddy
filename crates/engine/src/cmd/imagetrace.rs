//! IMAGETRACE (VECTORIZE): a scanned or rendered plan (an IMAGE object) traced into lines, arcs
//! and circles on a layer, thick bands (solid or hatched walls) as wall pairs (their two faces),
//! and text as boxes (outlines where the text is; no reading of it). The tracing itself is
//! [`cadaddy_geom::vectorize`] (published methods, cited there).

use cadaddy_doc::{Arc, Circle, EntityKind, Line, LwPolyline};
use cadaddy_geom::vectorize::{self, Options, Shape};
use cadaddy_geom::{PolyVertex, Vec2, Vec3};
use serde_json::{Value, json};

use super::*;
use crate::{Result, Session};

/// Most objects one trace adds.
const MAX_ADDED: usize = 200_000;
/// Segments for an arc where the image is skewed or stretched (it isn't an arc any more there).
const ARC_SEGMENTS: usize = 24;

pub fn specs() -> Vec<CommandSpec> {
    vec![CommandSpec::new("imagetrace", "Trace Image to Vectors", run)
        .menu(&["Insert", "Trace Raster Image to Vectors"])
        .alias(&["vectorize"])
        .params("{handle?: the IMAGE (default the selected one, else the only one), layer?: \"TRACED\" (walls go on <layer>-WALLS, text boxes on <layer>-TEXT), threshold?: 0..255 (ink is at most this grey; default Otsu's), tolerance?: pixels (1.5), ortho?: degrees (3; 0 off) from the plan's own main directions (a slightly turned scan stays square), walls?: true, wallMin?: pixels (default three times the typical stroke), text?: true, textMax?: pixels (default 2.5% of the image's shorter side, up to 6% for letters with another beside them), minLength?: pixels (4)} → {lines, arcs, circles, walls, textBoxes, threshold, stroke (pixels), handles}")]
}

/// The IMAGE to trace.
fn target(s: &Session, p: &Value) -> Result<cadaddy_doc::Image> {
    let cmd = "imagetrace";
    let d = s.doc()?;
    let image_of = |h: cadaddy_doc::Handle| d.entity(h).and_then(|e| if let EntityKind::Image(i) = &e.kind { Some(i.clone()) } else { None });
    if let Some(h) = str_param(p, "handle") {
        let h = cadaddy_doc::Handle::parse_hex(h).ok_or_else(|| bad(cmd, format!("`{h}` is not a handle")))?;
        return image_of(h).ok_or_else(|| bad(cmd, "that object is not a raster image"));
    }
    let selected: Vec<_> = s.selection().into_iter().filter_map(image_of).collect();
    if let [one] = selected.as_slice() {
        return Ok(one.clone());
    }
    let all: Vec<_> = d.model.iter().filter_map(|e| if let EntityKind::Image(i) = &e.kind { Some(i.clone()) } else { None }).take(2).collect();
    match all.as_slice() {
        [one] => Ok(one.clone()),
        [] => Err(crate::EngineError::Other("There is no raster image to trace (attach one with IMAGEATTACH).".into())),
        _ => Err(crate::EngineError::Other("Select the raster image to trace (there are several).".into())),
    }
}

fn run(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "imagetrace";
    let img = target(s, p)?;
    let raster =
        cadaddy_render::images::get(&img.path).ok_or_else(|| crate::EngineError::Other(format!("The image file {} can't be read.", img.path)))?;
    let (w, h) = (raster.width as usize, raster.height as usize);
    // Grey from the pixels; see-through counts as paper.
    let gray: Vec<u8> = raster
        .rgba
        .as_chunks::<4>()
        .0
        .iter()
        .map(|px| if px[3] < 128 { 255 } else { ((u32::from(px[0]) * 299 + u32::from(px[1]) * 587 + u32::from(px[2]) * 114) / 1000) as u8 })
        .collect();
    let num = |k: &str| p.get(k).and_then(Value::as_f64).filter(|v| v.is_finite());
    let threshold = match p.get("threshold") {
        None | Some(Value::Null) => None,
        Some(v) => Some(v.as_u64().and_then(|t| u8::try_from(t).ok()).ok_or_else(|| bad(cmd, "`threshold` is a grey level from 0 to 255"))?),
    };
    let o = Options {
        threshold,
        tolerance: num("tolerance").unwrap_or(1.5),
        ortho_deg: num("ortho").unwrap_or(3.0),
        walls: bool_or(p, "walls", true),
        wall_min_px: num("wallMin"),
        text: bool_or(p, "text", true),
        text_max_px: num("textMax"),
        min_length: num("minLength").unwrap_or(4.0),
    };
    let traced = vectorize::trace(&gray, w, h, &o).map_err(|e| bad(cmd, e))?;
    // Image units (pixels, y up from the lower-left corner) → the drawing, as the image is placed.
    let (sx, sy) = (if w > 0 { img.size.x / w as f64 } else { 1.0 }, if h > 0 { img.size.y / h as f64 } else { 1.0 });
    let (u, v, o3) = (img.u * sx, img.v * sy, img.insert);
    let to = |q: [f64; 2]| -> Vec3 { o3 + u * q[0] + v * q[1] };
    let to2 = |q: [f64; 2]| -> Vec2 { to(q).xy() };
    // Arcs stay arcs where the image is placed without skew or stretch.
    let (ul, vl) = (u.xy().len(), v.xy().len());
    let conformal = ul > 0.0 && (ul - vl).abs() <= 1e-9 * ul.max(vl) && u.xy().dot(v.xy()).abs() <= 1e-9 * ul * vl;
    let mirrored = u.xy().cross(v.xy()) < 0.0;
    let rot = u.xy().angle();
    let layer = str_param(p, "layer").map(str::trim).filter(|l| !l.is_empty()).unwrap_or("TRACED").to_string();
    let (wall_layer, text_layer) = (format!("{layer}-WALLS"), format!("{layer}-TEXT"));
    let space = s.space();
    let base = s.current_common();
    {
        let d = s.doc_mut()?;
        for l in [&layer, &wall_layer, &text_layer] {
            d.ensure_layer(l);
        }
    }
    let mut added = Vec::new();
    let (mut lines, mut arcs, mut circles, mut walls, mut boxes) = (0usize, 0usize, 0usize, 0usize, 0usize);
    let add = |s: &mut Session, on: &str, kind: EntityKind, added: &mut Vec<String>| -> Result<()> {
        if added.len() >= MAX_ADDED {
            return Ok(());
        }
        let mut c = base.clone();
        c.layer = on.to_string();
        let hnd = s.doc_mut()?.add(&space, c, kind)?;
        added.push(hnd.hex());
        Ok(())
    };
    let line = |a: [f64; 2], b: [f64; 2]| EntityKind::Line(Line { a: to(a), b: to(b) });
    let poly = |pts: Vec<Vec2>, closed: bool| {
        EntityKind::LwPolyline(LwPolyline {
            vertices: pts.into_iter().map(|p| PolyVertex { p, ..Default::default() }).collect(),
            closed,
            const_width: 0.0,
            elevation: o3.z,
            plinegen: false,
        })
    };
    for shape in &traced.shapes {
        match shape {
            Shape::Line { a, b, .. } => {
                add(s, &layer, line(*a, *b), &mut added)?;
                lines += 1;
            }
            Shape::Circle { center, radius, .. } => {
                if conformal {
                    add(s, &layer, EntityKind::Circle(Circle { center: to(*center), radius: radius * ul }), &mut added)?;
                } else {
                    let pts = (0..ARC_SEGMENTS).map(|i| {
                        let a = std::f64::consts::TAU * i as f64 / ARC_SEGMENTS as f64;
                        to2([center[0] + radius * a.cos(), center[1] + radius * a.sin()])
                    });
                    add(s, &layer, poly(pts.collect(), true), &mut added)?;
                }
                circles += 1;
            }
            Shape::Arc { center, radius, start, end, .. } => {
                if conformal {
                    // A mirrored placement reverses the turning: the arc runs the other way round.
                    let (a0, a1) = if mirrored { (rot - end, rot - start) } else { (rot + start, rot + end) };
                    let k = EntityKind::Arc(Arc {
                        center: to(*center),
                        radius: radius * ul,
                        start: cadaddy_geom::norm_angle(a0),
                        end: cadaddy_geom::norm_angle(a1),
                    });
                    add(s, &layer, k, &mut added)?;
                } else {
                    let sweep = (end - start).rem_euclid(std::f64::consts::TAU);
                    let pts = (0..=ARC_SEGMENTS).map(|i| {
                        let a = start + sweep * i as f64 / ARC_SEGMENTS as f64;
                        to2([center[0] + radius * a.cos(), center[1] + radius * a.sin()])
                    });
                    add(s, &layer, poly(pts.collect(), false), &mut added)?;
                }
                arcs += 1;
            }
            Shape::Wall { a, b, thickness } => {
                // The band's two faces, half its thickness each side of its centre line.
                let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
                let len = dx.hypot(dy);
                if len > 0.0 {
                    let (nx, ny) = (-dy / len * thickness / 2.0, dx / len * thickness / 2.0);
                    for k in [1.0, -1.0] {
                        add(s, &wall_layer, line([a[0] + nx * k, a[1] + ny * k], [b[0] + nx * k, b[1] + ny * k]), &mut added)?;
                    }
                    walls += 1;
                }
            }
            Shape::TextBox { min, max } => {
                let pts = vec![to2(*min), to2([max[0], min[1]]), to2(*max), to2([min[0], max[1]])];
                add(s, &text_layer, poly(pts, true), &mut added)?;
                boxes += 1;
            }
        }
    }
    let n = added.len();
    s.echo(format!("Traced {n} objects: {lines} lines, {arcs} arcs, {circles} circles, {walls} wall pairs, {boxes} text boxes."));
    if let Ok(st) = s.state_mut() {
        st.selection = added.iter().filter_map(|h| cadaddy_doc::Handle::parse_hex(h)).collect();
    }
    Ok(json!({
        "lines": lines, "arcs": arcs, "circles": circles, "walls": walls, "textBoxes": boxes,
        "threshold": traced.threshold, "stroke": traced.stroke, "handles": added,
    }))
}
