//! The plot hook the apps install ([`plot`]): PDF, PNG or SVG output of a plotter configuration (`output`), on the
//! device's paper (`width`/`height`, `margins` in mm) at its `resolution`.
//!
//! PDF goes to [`crate::pdf`]. PNG draws the sheet on white paper at the device resolution
//! (capped at [`MAX_PNG_PIXELS`]), with the plot style table and lineweights as PDF uses them.
//! SVG shows the sheet of a layout or the model-space extents.

use std::sync::Arc;

use cadaddy_color::Rgb;
use cadaddy_doc::{Drawing, Space};
use cadaddy_geom::{Bounds2, Vec2};
use cadaddy_render::{Sheet, paper, raster};
use serde_json::Value;

use crate::{IoError, PdfOptions, Result, pdf};

/// Largest PNG plot (pixels); larger sheets are drawn at a lower resolution.
pub const MAX_PNG_PIXELS: f64 = 40.0e6;

/// Plot `space` with JSON options (the engine's `plot` hook): PDF unless `output` is `png` or
/// `svg`. `margins: [l, b, r, t]` (mm) bound the printable area of this plot (model space or a
/// layout).
pub fn plot(d: &Drawing, space: &Space, opts: &Value) -> Result<Vec<u8>> {
    pdf::validate_plot_offset_json(opts)?;
    if matches!(space, Space::Model)
        && opts.get("output").and_then(Value::as_str).is_some_and(|output| output.eq_ignore_ascii_case("svg"))
        && opts.get("center").and_then(Value::as_bool) == Some(false)
    {
        return Err(IoError::Format("model-space plot offsets are not supported for SVG; choose PDF/PNG or center the plot".into()));
    }
    let margins = pdf::margins(opts.get("margins"));
    let with_margins;
    let d = match (space, margins) {
        (Space::Paper(n), Some(m)) => {
            let mut c = d.clone();
            if let Some(l) = c.layouts.iter_mut().find(|l| l.name.eq_ignore_ascii_case(n)) {
                l.page.margins_mm = m;
            }
            with_margins = c;
            &with_margins
        }
        _ => d,
    };
    // Model space as displayed in a 3D view or a visual style: a shaded raster of the view.
    if matches!(space, Space::Model)
        && let Some(shade) = opts.get("shade").filter(|v| v.is_object())
    {
        return shaded(d, opts, shade);
    }
    match opts.get("output").and_then(Value::as_str).map(str::to_ascii_lowercase).as_deref() {
        None | Some("pdf") => pdf::plot(d, space, opts),
        Some("png") => png(d, space, opts),
        Some("svg") => svg(d, space, opts),
        Some(other) => Err(IoError::Unsupported(other.to_string())),
    }
}

/// The sheet a plot draws on (a model-space plot's printable area inside the device margins).
fn sheet(d: &Drawing, space: &Space, o: &PdfOptions) -> Result<(Sheet, cadaddy_doc::PageSetup)> {
    let page = pdf::page_for(d, space, o)?;
    Ok((Sheet::from_page(&page, paper::page_unit_mm(d, &page)), page))
}

/// Paper units per drawing unit fitting `b` into the printable area; with `fit: false`, the
/// `scale` option or 1:1, as PDF plots it.
fn fit_scale(b: &Bounds2, sheet: &Sheet, o: &PdfOptions) -> f64 {
    pdf::plot_scale(b, sheet, o.fit != Some(false), o.scale)
}

fn png(d: &Drawing, space: &Space, opts: &Value) -> Result<Vec<u8>> {
    let o = PdfOptions::from_json(opts);
    let (sheet, page) = sheet(d, space, &o)?;
    let dpi = opts.get("resolution").and_then(Value::as_f64).filter(|r| r.is_finite()).unwrap_or(300.0).clamp(50.0, 2400.0);
    // Pixels per paper unit, lowered so the image stays within the caps.
    let mut k = sheet.unit_mm / 25.4 * dpi;
    let (w, h) = (sheet.size.x * k, sheet.size.y * k);
    let shrink = (MAX_PNG_PIXELS / (w * h).max(1.0)).sqrt().min(16384.0 / w.max(h).max(1.0)).min(1.0);
    if !(shrink.is_finite() && shrink > 0.0) {
        return Err(IoError::Format("the sheet is too large to plot as an image".into()));
    }
    k *= shrink;
    let px = |v: f64| (v * k).round().clamp(1.0, 16384.0) as u32;
    let (width, height) = (px(sheet.size.x), px(sheet.size.y));
    let lineweights = o.lineweights.unwrap_or(if space.is_paper() { page.lineweights } else { true });
    let table = cadaddy_doc::plot_style_table(d, &page.plot_style_table).map(Arc::new);
    // Paper units per drawing unit: 1 for a layout at 1:1, the fitted scale for model space.
    let fit = matches!(space, Space::Model) || o.fit == Some(true);
    let window = o.window.filter(|w| matches!(space, Space::Model) && !w.is_empty());
    let est = if fit { fit_scale(&window.unwrap_or_else(|| d.extents(space)), &sheet, &o) } else { 1.0 };
    // Chord tolerance: a fifth of a pixel.
    let tolerance = (0.2 / (k * est)).max(1e-9);
    let transparency = o.plot_transparency.unwrap_or(page.plot_transparency);
    let ropts = cadaddy_render::Options { tolerance, lineweights, plot_style_table: table, transparency, ..cadaddy_render::Options::default() };
    let mut list = cadaddy_render::build_plot(d, space, &ropts);
    let half = Vec2::new(f64::from(width), f64::from(height)) / (2.0 * k);
    let b = window.unwrap_or(list.bounds);
    let s = if fit { fit_scale(&b, &sheet, &o) } else { 1.0 };
    // Plot style ends, joins and fill patterns at the plot scale, as PDF draws them (a layout's
    // are already drawn).
    cadaddy_render::pen::apply(&mut list, s * sheet.unit_mm);
    let view = if fit {
        let (from, to) = pdf::placement(space, &b, true, &sheet, &o);
        // The world point at the image centre, mapped through the same paper origin as PDF.
        let center = from + (half - to) / s;
        raster::View { center, scale: k * s, width, height }
    } else {
        raster::View { center: pdf::layout_origin(d, space) + half, scale: k, width, height }
    };
    let px_per_mm = if lineweights { (k / sheet.unit_mm) as f32 } else { 0.0 };
    let ro = raster::RasterOptions { background: Rgb(255, 255, 255), hairline: 1.0, px_per_mm, antialias: true };
    raster::render_png_turned(&list, &view, &ro, o.upside_down).ok_or_else(|| IoError::Format("render failed".into()))
}

fn svg(d: &Drawing, space: &Space, opts: &Value) -> Result<Vec<u8>> {
    let o = PdfOptions::from_json(opts);
    let Space::Model = space else {
        let (sheet, page) = sheet(d, space, &o)?;
        let transparency = o.plot_transparency.unwrap_or(page.plot_transparency);
        let sheet = sheet.at(pdf::layout_origin(d, space));
        return Ok(turn_svg(crate::svg::export_window_with(d, space, Some(sheet.bounds()), transparency), o.upside_down).into_bytes());
    };
    // Model space as plotted: the plot style table, lineweights, and the table's ends, joins and
    // fills at the scale that fits the window (or the extents) on the device's paper.
    let (sheet, page) = sheet(d, space, &o)?;
    let window = o.window.filter(|w| !w.is_empty());
    let table = cadaddy_doc::plot_style_table(d, &page.plot_style_table).map(Arc::new);
    let lineweights = o.lineweights.unwrap_or(true);
    let est = fit_scale(&window.unwrap_or_else(|| d.extents(space)), &sheet, &o);
    // Chord tolerance: about 0.05 mm on paper.
    let tolerance = (0.05 / (sheet.unit_mm * est)).max(1e-9);
    let tolerance = if tolerance.is_finite() { tolerance } else { cadaddy_render::Options::default().tolerance };
    let transparency = o.plot_transparency.unwrap_or(page.plot_transparency);
    let ropts = cadaddy_render::Options { tolerance, lineweights, plot_style_table: table, transparency, ..cadaddy_render::Options::default() };
    let mut list = cadaddy_render::build_plot(d, space, &ropts);
    let s = fit_scale(&window.unwrap_or(list.bounds), &sheet, &o);
    cadaddy_render::pen::apply(&mut list, s * sheet.unit_mm);
    Ok(turn_svg(crate::svg::export_list(&list, window), o.upside_down).into_bytes())
}

/// The SVG turned half a turn about its view box's centre when `upside_down`.
fn turn_svg(svg: String, upside_down: bool) -> String {
    if !upside_down {
        return svg;
    }
    let view_box = svg.split_once("viewBox=\"").and_then(|(_, rest)| rest.split_once('"')).map(|(v, _)| v);
    let nums: Vec<f64> = view_box.map(|v| v.split_ascii_whitespace().filter_map(|n| n.parse().ok()).collect()).unwrap_or_default();
    let head_end = svg.find("<svg").and_then(|i| Some(i + svg.get(i..)?.find('>')?));
    let (Some(head_end), Some(tail), &[x, y, w, h]) = (head_end, svg.rfind("</svg>"), nums.as_slice()) else { return svg };
    if tail <= head_end {
        return svg;
    }
    let (cx, cy) = (x + w / 2.0, y + h / 2.0);
    let (head, rest) = svg.split_at(head_end + 1);
    let (body, end) = rest.split_at(tail - head_end - 1);
    format!("{head}<g transform=\"rotate(180 {cx} {cy})\">{body}</g>{end}")
}

/// A shaded plot (`shade: {style, dir, twist}`, what the engine passes for a 3D view or a
/// visual style): model space seen along `dir` (`twist` turning a plan view), its extents fitted
/// into the printable area, drawn in `style` on white paper as a raster, a PNG or one PDF page
/// holding it (PDF at 200 dpi, PNG at `resolution`).
fn shaded(d: &Drawing, opts: &Value, shade: &Value) -> Result<Vec<u8>> {
    use cadaddy_geom::Vec3;
    use cadaddy_geom::view3::Projection;
    use cadaddy_render::shade::{self, VisualStyle};
    let style = shade.get("style").and_then(Value::as_str).and_then(VisualStyle::parse).unwrap_or(VisualStyle::Wireframe);
    let dir = shade
        .get("dir")
        .and_then(Value::as_array)
        .and_then(|a| match a.as_slice() {
            [x, y, z] => Some(Vec3::new(x.as_f64()?, y.as_f64()?, z.as_f64()?)),
            _ => None,
        })
        .filter(|v| v.is_finite() && v.len() > 1e-12)
        .unwrap_or(Vec3::Z);
    let twist = shade.get("twist").and_then(Value::as_f64).filter(|t| t.is_finite()).unwrap_or(0.0);
    let proj = if cadaddy_geom::view3::is_plan(dir) { Projection::plan_twist(twist) } else { Projection::new(dir) };
    let o = PdfOptions::from_json(opts);
    let (sheet, _) = sheet(d, &Space::Model, &o)?;
    let png = opts.get("output").and_then(Value::as_str).is_some_and(|x| x.eq_ignore_ascii_case("png"));
    let dpi = if png { opts.get("resolution").and_then(Value::as_f64).filter(|r| r.is_finite()).unwrap_or(300.0).clamp(50.0, 2400.0) } else { 200.0 };
    let area = if sheet.printable.is_empty() { sheet.bounds() } else { sheet.printable };
    let k = sheet.unit_mm / 25.4 * dpi;
    let (mut w, mut h) = (area.width() * k, area.height() * k);
    let shrink = (MAX_PNG_PIXELS / (w * h).max(1.0)).sqrt().min(f64::from(shade::MAX_SIDE) / w.max(h).max(1.0)).min(1.0);
    if !(shrink.is_finite() && shrink > 0.0) {
        return Err(IoError::Format("the sheet is too large to plot".into()));
    }
    w *= shrink;
    h *= shrink;
    let (wp, hp) = ((w.round() as u32).max(1), (h.round() as u32).max(1));
    let raster =
        shade::render_fitted(d, proj, style, wp, hp, Rgb(255, 255, 255)).ok_or_else(|| IoError::Format("model space has nothing to plot".into()))?;
    if png {
        return raster.encode_png().ok_or_else(|| IoError::Format("PNG encoding failed".into()));
    }
    let pt = sheet.unit_mm * 72.0 / 25.4;
    let media = (sheet.size.x * pt, sheet.size.y * pt);
    let place = ((area.min.x - sheet.origin.x) * pt, (area.min.y - sheet.origin.y) * pt, area.width() * pt, area.height() * pt);
    Ok(pdf::raster_page(&raster, media, place))
}

#[cfg(test)]
mod tests {
    use super::*;
    use cadaddy_doc::{Common, EntityKind, Line};
    use cadaddy_geom::{Bounds2, Vec2, Vec3};
    use serde_json::json;

    /// Width and height from a PNG's header.
    fn png_size(b: &[u8]) -> (u32, u32) {
        let be = |i: usize| u32::from_be_bytes([b[i], b[i + 1], b[i + 2], b[i + 3]]);
        (be(16), be(20))
    }

    fn drawing() -> Drawing {
        let mut d = Drawing::new_metric();
        d.add(&Space::Model, Common::default(), EntityKind::Line(Line { a: Vec3::ZERO, b: Vec3::new(100.0, 50.0, 0.0) })).unwrap();
        d
    }

    #[test]
    fn png_and_svg_devices() {
        let d = drawing();
        let a4 = json!({"output": "png", "resolution": 100, "width": 210, "height": 297, "landscape": true, "margins": [5, 5, 5, 5]});
        let png = plot(&d, &Space::Model, &a4).unwrap();
        assert_eq!(png_size(&png), (1169, 827), "A4 landscape at 100 dpi");
        // Huge sheets at a high resolution are capped.
        let big = json!({"output": "png", "resolution": 2400, "width": 841, "height": 1189});
        let (w, h) = png_size(&plot(&d, &Space::Model, &big).unwrap());
        assert!(f64::from(w) * f64::from(h) <= MAX_PNG_PIXELS * 1.01 && w <= 16384 && h <= 16384);
        let layout = Space::Paper(d.layouts[0].name.clone());
        let svg = String::from_utf8(plot(&d, &layout, &json!({"output": "svg"})).unwrap()).unwrap();
        assert!(svg.starts_with("<svg"));
        assert!(plot(&d, &layout, &json!({"output": "png", "resolution": f64::NAN, "margins": [1, 2, 3]})).is_ok());
        assert!(plot(&d, &Space::Model, &json!({"output": "dwf"})).is_err());
        assert!(plot(&d, &Space::Model, &json!({})).unwrap().starts_with(b"%PDF"));
    }

    #[test]
    fn upside_down_turns_every_output_half_a_turn() {
        let d = drawing();
        let opts = |output: &str, turn: bool| json!({"output": output, "resolution": 50, "upsideDown": turn});
        // PNG: the same pixels in reverse order.
        let pixels = |turn: bool| tiny_skia::Pixmap::decode_png(&plot(&d, &Space::Model, &opts("png", turn)).unwrap()).unwrap().data().to_vec();
        let (plain, turned) = (pixels(false), pixels(true));
        assert_ne!(plain, turned, "the line is not symmetric on the sheet");
        let reversed: Vec<u8> = plain.as_chunks::<4>().0.iter().rev().flatten().copied().collect();
        assert_eq!(turned, reversed);
        // PDF: the page turns.
        let has = |b: &[u8], what: &str| b.windows(what.len()).any(|w| w == what.as_bytes());
        assert!(has(&plot(&d, &Space::Model, &opts("pdf", true)).unwrap(), "/Rotate 180"));
        assert!(!has(&plot(&d, &Space::Model, &opts("pdf", false)).unwrap(), "/Rotate"));
        // SVG (model and layout): the drawing turns about the view box's centre.
        let layout = Space::Paper(d.layouts[0].name.clone());
        for space in [Space::Model, layout] {
            let svg = String::from_utf8(plot(&d, &space, &opts("svg", true)).unwrap()).unwrap();
            assert!(svg.contains("<g transform=\"rotate(180 ") && svg.trim_end().ends_with("</g></svg>"), "{svg}");
            let plain = String::from_utf8(plot(&d, &space, &opts("svg", false)).unwrap()).unwrap();
            assert!(!plain.contains("rotate(180"));
        }
        assert_eq!(turn_svg("not svg".into(), true), "not svg");
    }

    #[test]
    fn model_plot_placement_preserves_centering_and_applies_mm_offsets() {
        let model = Space::Model;
        let bounds = Bounds2::new(Vec2::new(10.0, 20.0), Vec2::new(110.0, 70.0));
        let metric = drawing();
        let metric_sheet = Sheet::from_page(&pdf::page_for(&metric, &model, &PdfOptions::default()).unwrap(), 1.0);
        let centered = PdfOptions::from_json(&json!({"center": true}));
        assert_eq!(pdf::placement(&model, &bounds, true, &metric_sheet, &PdfOptions::default()), (bounds.center(), metric_sheet.printable.center()));
        assert_eq!(pdf::placement(&model, &bounds, true, &metric_sheet, &centered), (bounds.center(), metric_sheet.printable.center()));
        let moved = PdfOptions::from_json(&json!({"center": false, "offset": [10.0, 20.0]}));
        assert_eq!(pdf::placement(&model, &bounds, true, &metric_sheet, &moved), (bounds.min, metric_sheet.printable.min + Vec2::new(10.0, 20.0)));
        let origin = PdfOptions::from_json(&json!({"center": false}));
        assert_eq!(pdf::placement(&model, &bounds, true, &metric_sheet, &origin), (bounds.min, metric_sheet.printable.min));
        let negative = PdfOptions::from_json(&json!({"center": false, "offset": [-10.0, -20.0]}));
        assert_eq!(pdf::placement(&model, &bounds, true, &metric_sheet, &negative), (bounds.min, metric_sheet.printable.min - Vec2::new(10.0, 20.0)));

        let imperial = Drawing::new_imperial();
        let imperial_sheet = Sheet::from_page(&pdf::page_for(&imperial, &model, &PdfOptions::default()).unwrap(), 25.4);
        assert_eq!(
            pdf::placement(&model, &bounds, true, &imperial_sheet, &moved),
            (bounds.min, imperial_sheet.printable.min + Vec2::new(10.0 / 25.4, 20.0 / 25.4))
        );
        assert_eq!(
            pdf::placement(&model, &bounds, true, &imperial_sheet, &negative),
            (bounds.min, imperial_sheet.printable.min - Vec2::new(10.0 / 25.4, 20.0 / 25.4))
        );
    }

    #[test]
    fn plot_offsets_keep_default_bytes_and_match_across_pdf_and_png() {
        let d = drawing();
        let model = Space::Model;
        let pdf_default = plot(&d, &model, &json!({})).unwrap();
        let pdf_centered = plot(&d, &model, &json!({"center": true})).unwrap();
        assert_eq!(pdf_default, pdf_centered, "explicit center preserves legacy PDF bytes");

        let png_options = json!({"output": "png", "resolution": 100, "width": 210, "height": 297, "landscape": true, "margins": [5, 5, 5, 5]});
        let png_default = plot(&d, &model, &png_options).unwrap();
        let mut centered_png = png_options.clone();
        centered_png["center"] = json!(true);
        assert_eq!(png_default, plot(&d, &model, &centered_png).unwrap(), "explicit center preserves legacy PNG bytes");

        let mut pdf_moved = json!({"center": false, "offset": [10.0, 20.0]});
        let moved_pdf = plot(&d, &model, &pdf_moved).unwrap();
        assert_ne!(pdf_default, moved_pdf);
        pdf_moved["output"] = json!("png");
        pdf_moved["resolution"] = json!(100);
        pdf_moved["width"] = json!(210);
        pdf_moved["height"] = json!(297);
        pdf_moved["landscape"] = json!(true);
        pdf_moved["margins"] = json!([5, 5, 5, 5]);
        let moved_png = plot(&d, &model, &pdf_moved).unwrap();
        assert_ne!(png_default, moved_png);

        let layout = Space::Paper(d.layouts[0].name.clone());
        assert_eq!(plot(&d, &layout, &json!({})).unwrap(), plot(&d, &layout, &json!({"center": false, "offset": [10.0, 20.0]})).unwrap());
    }

    #[test]
    fn invalid_and_unsupported_plot_offsets_are_reported() {
        let d = drawing();
        let model = Space::Model;
        for offset in [json!([1]), json!([1, 2, 3]), json!([1, "2"]), json!([f64::NAN, 2]), json!([100_001, 0])] {
            assert!(plot(&d, &model, &json!({"offset": offset})).is_err());
        }
        let err = plot(&d, &model, &json!({"output": "svg", "center": false, "offset": [0, 0]})).unwrap_err();
        assert!(err.to_string().contains("not supported for SVG"));
        assert!(plot(&d, &model, &json!({"output": "svg", "center": true})).unwrap().starts_with(b"<svg"));
    }
}
