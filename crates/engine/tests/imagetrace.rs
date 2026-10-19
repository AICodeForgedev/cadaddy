//! IMAGETRACE: a synthetic plan rendered to an image, placed in the drawing, traced back into
//! objects at the drawing's scale and position.

use cadaddy_doc::{EntityKind, Image};
use cadaddy_engine::Session;
use cadaddy_geom::{Vec2, Vec3};
use serde_json::json;

/// A white RGBA image with black strokes (y down).
struct Canvas {
    w: usize,
    h: usize,
    px: Vec<u8>,
}

impl Canvas {
    fn new(w: usize, h: usize) -> Canvas {
        Canvas { w, h, px: vec![255; w * h * 4] }
    }
    fn dot(&mut self, cx: f64, cy: f64, r: f64) {
        for y in (cy - r).floor() as i64..=(cy + r).ceil() as i64 {
            for x in (cx - r).floor() as i64..=(cx + r).ceil() as i64 {
                if x >= 0 && y >= 0 && (x as usize) < self.w && (y as usize) < self.h && (x as f64 + 0.5 - cx).hypot(y as f64 + 0.5 - cy) <= r {
                    let i = (y as usize * self.w + x as usize) * 4;
                    self.px[i..i + 3].copy_from_slice(&[0, 0, 0]);
                }
            }
        }
    }
    fn line(&mut self, a: (f64, f64), b: (f64, f64), width: f64) {
        let n = ((b.0 - a.0).hypot(b.1 - a.1) * 2.0).ceil().max(1.0) as usize;
        for i in 0..=n {
            let t = i as f64 / n as f64;
            self.dot(a.0 + (b.0 - a.0) * t, a.1 + (b.1 - a.1) * t, width / 2.0);
        }
    }
}

fn plan() -> Canvas {
    let mut c = Canvas::new(400, 300);
    // A room of thin lines, a thick (solid) wall across the top, a column (circle).
    c.line((40.0, 80.0), (360.0, 80.0), 2.0);
    c.line((360.0, 80.0), (360.0, 260.0), 2.0);
    c.line((360.0, 260.0), (40.0, 260.0), 2.0);
    c.line((40.0, 260.0), (40.0, 80.0), 2.0);
    c.line((40.0, 30.0), (360.0, 30.0), 12.0);
    let n = 400;
    for i in 0..=n {
        let a = std::f64::consts::TAU * i as f64 / n as f64;
        c.dot(200.0 + 30.0 * a.cos(), 170.0 + 30.0 * a.sin(), 1.0);
    }
    c
}

#[test]
fn a_placed_plan_image_traces_back_at_the_drawings_scale() {
    let c = plan();
    let raster = cadaddy_render::images::Raster::new(c.w as u32, c.h as u32, c.px.clone()).unwrap();
    let path = format!("imagetrace-test-{}.png", std::process::id());
    cadaddy_render::images::insert(&path, raster);
    let mut s = Session::new();
    // Placed at (1000, 2000), half a drawing unit per pixel.
    let img = Image {
        insert: Vec3::new(1000.0, 2000.0, 0.0),
        u: Vec3::new(0.5, 0.0, 0.0),
        v: Vec3::new(0.0, 0.5, 0.0),
        size: Vec2::new(400.0, 300.0),
        path: path.clone(),
        ..Image::default()
    };
    s.add_entity(EntityKind::Image(img)).unwrap();
    let r = s.execute("imagetrace", &json!({"text": false})).unwrap();
    assert_eq!((r["lines"].as_u64(), r["walls"].as_u64(), r["circles"].as_u64()), (Some(4), Some(1), Some(1)), "{r}");
    let d = s.doc().unwrap();
    // The room: y-down 80/260 → y up 220/40 pixels → 2110 / 2020 in the drawing.
    let lines: Vec<_> = d
        .model
        .iter()
        .filter(|e| e.common.layer == "TRACED")
        .filter_map(|e| if let EntityKind::Line(l) = &e.kind { Some(l.clone()) } else { None })
        .collect();
    for (x, y) in [(1020.0, 2110.0), (1180.0, 2110.0), (1180.0, 2020.0), (1020.0, 2020.0)] {
        assert!(lines.iter().any(|l| [l.a, l.b].iter().any(|p| (p.x - x).hypot(p.y - y) <= 1.0)), "corner ({x}, {y}) in {lines:?}");
    }
    assert!(lines.iter().all(|l| l.a.x == l.b.x || l.a.y == l.b.y), "orthogonal");
    // The column: centre (200, 130 y up) → (1100, 2065), radius 15.
    let circle = d.model.iter().find_map(|e| if let EntityKind::Circle(c) = &e.kind { Some(c.clone()) } else { None }).unwrap();
    assert!((circle.center.x - 1100.0).abs() <= 1.0 && (circle.center.y - 2065.0).abs() <= 1.0 && (circle.radius - 15.0).abs() <= 1.0, "{circle:?}");
    // The thick wall: its two faces on TRACED-WALLS, about 6 units apart around y = 2135.
    let faces: Vec<_> = d
        .model
        .iter()
        .filter(|e| e.common.layer == "TRACED-WALLS")
        .filter_map(|e| if let EntityKind::Line(l) = &e.kind { Some(l.a.y) } else { None })
        .collect();
    assert_eq!(faces.len(), 2);
    assert!(((faces[0] - faces[1]).abs() - 6.0).abs() <= 1.5 && ((faces[0] + faces[1]) / 2.0 - 2135.0).abs() <= 1.0, "{faces:?}");
    // What was traced is selected, and one undo takes it all away.
    assert_eq!(s.selection().len(), r["handles"].as_array().unwrap().len());
    s.execute("undo", &json!({})).unwrap();
    assert_eq!(s.doc().unwrap().model.len(), 1);
    // Typed as VECTORIZE too; with no image or a bad threshold: said so.
    assert!(s.execute("vectorize", &json!({"threshold": 300})).is_err());
    let mut empty = Session::new();
    assert!(empty.execute("imagetrace", &json!({})).is_err());
    cadaddy_render::images::forget(&path);
}
