//! Text and dimension masks over what lies beneath: an MTEXT background mask and a dimension's
//! DIMTFILL box hide a hatch drawn before them (render order), in the rendered image.

use cadaddy_engine::Session;
use serde_json::{Value, json};

fn h(r: &Value) -> String {
    r.get("handle").or_else(|| r["handles"].get(0)).and_then(Value::as_str).unwrap().to_string()
}

/// The drawing as an image (x, y in drawing units within 0..100 × 0..100), and a pixel getter.
fn render(s: &Session) -> (u32, u32, Vec<u8>) {
    let d = s.doc().unwrap();
    let w = cadaddy_geom::Bounds2 { min: cadaddy_geom::Vec2::new(0.0, 0.0), max: cadaddy_geom::Vec2::new(100.0, 100.0) };
    let png = cadaddy_io::png_framed(d, &cadaddy_doc::Space::Model, 500, 500, Some(w)).unwrap();
    let r = cadaddy_render::images::decode(&png).unwrap();
    (r.width, r.height, r.rgba)
}

/// Share of hatch-coloured (red) pixels in a drawing-unit rectangle.
fn red_share(img: &(u32, u32, Vec<u8>), x0: f64, y0: f64, x1: f64, y1: f64) -> f64 {
    let (w, hgt, px) = img;
    let to = |x: f64, y: f64| ((x / 100.0 * f64::from(*w)) as u32, ((1.0 - y / 100.0) * f64::from(*hgt)) as u32);
    let ((ax, ay), (bx, by)) = (to(x0, y1), to(x1, y0));
    let (mut red, mut n) = (0, 0);
    for y in ay..by {
        for x in ax..bx {
            let i = ((y * w + x) * 4) as usize;
            let p = &px[i..i + 4];
            n += 1;
            if p[0] > 150 && p[1] < 90 && p[2] < 90 {
                red += 1;
            }
        }
    }
    red as f64 / n.max(1) as f64
}

#[test]
fn masks_hide_the_hatch_beneath_in_render_order() {
    let mut s = Session::new();
    let b = h(&s.execute("rectang", &json!({"p1": [5, 5], "p2": [95, 95]})).unwrap());
    let hatch = h(&s.execute("hatch", &json!({"handles": [b], "pattern": "ANSI31", "scale": 0.5})).unwrap());
    s.execute("properties.set", &json!({"handles": [hatch], "color": 1})).unwrap();
    let img = render(&s);
    let bare = red_share(&img, 20.0, 60.0, 40.0, 70.0);
    assert!(bare > 0.05, "the hatch shows where nothing covers it: {bare}");
    // An MTEXT with a background mask over the hatch.
    let t = h(&s.execute("mtext", &json!({"at": [20, 70], "text": "MASKED", "height": 6})).unwrap());
    s.execute("properties.set", &json!({"handles": [t], "background": true})).unwrap();
    // A dimension with DIMTFILL 1 (the drawing's background) over the hatch.
    s.execute("setvar", &json!({"name": "DIMTFILL", "value": 1})).unwrap();
    s.execute("setvar", &json!({"name": "DIMSCALE", "value": 25})).unwrap();
    s.execute("dimlinear", &json!({"p1": [20, 30], "p2": [80, 30], "at": [50, 25]})).unwrap();
    let img = render(&s);
    // Inside the text's box, between and around the glyph strokes: the mask, not the hatch.
    let under_text = red_share(&img, 21.0, 65.5, 55.0, 69.0);
    assert!(under_text < 0.01, "the MTEXT mask hides the hatch: {under_text}");
    let under_dim = red_share(&img, 37.0, 22.0, 63.0, 25.0);
    assert!(under_dim < 0.01, "DIMTFILL hides the hatch under the dimension text: {under_dim}");
    // Brought to the front, the hatch covers the masks (draw order decides).
    s.execute("draworder", &json!({"handles": [hatch], "order": "front"})).unwrap();
    let img = render(&s);
    assert!(red_share(&img, 21.0, 65.5, 55.0, 69.0) > 0.9, "the hatch in front hides the text and its mask");
    // TEXTUNMASK takes the MTEXT mask away (the hatch shows behind the text again).
    s.execute("draworder", &json!({"handles": [hatch], "order": "back"})).unwrap();
    s.execute("textunmask", &json!({"handles": [t]})).unwrap();
    let img = render(&s);
    assert!(red_share(&img, 21.0, 65.5, 55.0, 69.0) > 0.5, "unmasked");
}

#[test]
fn a_mask_hides_an_image_beneath_it() {
    // A red image under a masked MTEXT.
    let path = format!("textmask-red-{}.png", std::process::id());
    cadaddy_render::images::insert(&path, cadaddy_render::images::Raster::new(4, 4, [220u8, 30, 30, 255].repeat(16)).unwrap());
    let mut s = Session::new();
    let img = cadaddy_doc::Image {
        insert: cadaddy_geom::Vec3::new(5.0, 5.0, 0.0),
        u: cadaddy_geom::Vec3::new(22.5, 0.0, 0.0),
        v: cadaddy_geom::Vec3::new(0.0, 22.5, 0.0),
        size: cadaddy_geom::Vec2::new(4.0, 4.0),
        path: path.clone(),
        ..cadaddy_doc::Image::default()
    };
    s.add_entity(cadaddy_doc::EntityKind::Image(img)).unwrap();
    let t = h(&s.execute("mtext", &json!({"at": [20, 70], "text": "MASKED", "height": 6})).unwrap());
    let before = red_share(&render(&s), 21.0, 65.5, 48.0, 69.0);
    assert!(before > 0.5, "the image shows behind plain text: {before}");
    s.execute("textmask", &json!({"handles": [t], "type": "background", "offset": 0.2})).unwrap();
    let after = red_share(&render(&s), 21.0, 65.5, 48.0, 69.0);
    assert!(after < 0.01, "the mask hides the image: {after}");
    cadaddy_render::images::forget(&path);
}
