//! Raster images go into PDF plots (an image XObject per picture, clipped and placed) and SVG
//! exports (an embedded PNG).

use cadaddy_doc::{Common, Drawing, EntityKind, Image, Space};
use cadaddy_geom::{Vec2, Vec3};
use cadaddy_io::{PdfOptions, pdf, svg};
use cadaddy_render::images::{Raster, insert};

fn drawing(path: &str) -> Drawing {
    // 2 × 1 pixels, the second half transparent
    insert(path, Raster::new(2, 1, vec![255, 0, 0, 255, 0, 0, 255, 0]).unwrap());
    let mut d = Drawing::new_imperial();
    let img = Image {
        insert: Vec3::new(1.0, 1.0, 0.0),
        u: Vec3::new(2.0, 0.0, 0.0),
        v: Vec3::new(0.0, 2.0, 0.0),
        size: Vec2::new(2.0, 1.0),
        path: path.into(),
        ..Image::default()
    };
    d.add(&Space::Model, Common::default(), EntityKind::Image(img)).unwrap();
    d
}

#[test]
fn pdf_plots_embed_the_image_with_its_alpha() {
    let d = drawing("mem://export-pdf");
    let bytes = pdf(&d, &Space::Model, &PdfOptions { compress: false, ..PdfOptions::default() }).unwrap();
    let text = String::from_utf8_lossy(&bytes);
    assert!(text.contains("/Subtype /Image /Width 2 /Height 1"), "an image XObject");
    assert!(text.contains("/SMask"), "with its transparency");
    assert!(text.contains("/Im0 Do"), "painted on the page");
    assert!(text.contains("/XObject << /Im0"), "listed in the page resources");
}

#[test]
fn svg_exports_embed_a_png() {
    let d = drawing("mem://export-svg");
    let s = svg::export(&d, &Space::Model);
    assert!(s.contains(r#"<image clip-path="url(#img0)" width="2" height="1""#), "{s}");
    assert!(s.contains("data:image/png;base64,iVBORw0KGgo"), "a PNG");
}
