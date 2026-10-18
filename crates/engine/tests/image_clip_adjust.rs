//! IMAGECLIP and IMAGEADJUST: boundaries kept in the image's pixel coordinates, cut to the image,
//! switched on and off and deleted; brightness, contrast and fade set; all of it typed, undone and
//! kept through DXF and DWG.

use cadaddy_engine::Session;
use cadaddy_engine::doc::{Drawing, EntityKind, Handle, Image, Space};
use cadaddy_engine::geom::{Vec2, Vec3};
use serde_json::json;

/// A 100 × 50 pixel image, one unit a pixel, lower-left corner at the origin.
fn session() -> (Session, Handle) {
    let mut s = Session::new();
    let img = Image {
        insert: Vec3::new(0.0, 0.0, 0.0),
        u: Vec3::new(1.0, 0.0, 0.0),
        v: Vec3::new(0.0, 1.0, 0.0),
        size: Vec2::new(100.0, 50.0),
        path: "clip-test.png".into(),
        ..Image::default()
    };
    let h = s.add_entity(EntityKind::Image(img)).unwrap();
    (s, h)
}

fn image(s: &Session, h: Handle) -> Image {
    match &s.doc().unwrap().entity(h).unwrap().kind {
        EntityKind::Image(i) => i.clone(),
        k => panic!("not an image: {k:?}"),
    }
}

fn near(a: &[Vec2], b: &[(f64, f64)]) -> bool {
    a.len() == b.len() && a.iter().zip(b).all(|(p, q)| p.near(Vec2::new(q.0, q.1), 1e-9))
}

fn typed(s: &mut Session, lines: &[&str]) {
    for t in lines {
        s.cmdline(t).unwrap();
    }
}

#[test]
fn json_rectangle_is_kept_in_pixels_and_cut_to_the_image() {
    let (mut s, h) = session();
    let r = s.execute("imageclip", &json!({ "handles": [h.hex()], "boundary": [[10, 10], [30, 40]] })).unwrap();
    assert_eq!(r["changed"], 1);
    let i = image(&s, h);
    // world y 10..40 is pixel row 39.5..9.5 (rows count down from the top)
    assert!(near(&i.clip, &[(9.5, 9.5), (29.5, 39.5)]), "{:?}", i.clip);
    assert!(i.clipping && i.display & 4 != 0);
    // past the image's edge: cut to it
    s.execute("imageclip", &json!({ "handles": [h.hex()], "boundary": [[-10, -10], [20, 20]] })).unwrap();
    assert!(near(&image(&s, h).clip, &[(-0.5, 29.5), (19.5, 49.5)]), "{:?}", image(&s, h).clip);
}

#[test]
fn json_polygon_on_off_delete_and_undo() {
    let (mut s, h) = session();
    let hs = json!([h.hex()]);
    s.execute("imageclip", &json!({ "handles": hs, "boundary": [[10, 10], [90, 10], [50, 40]] })).unwrap();
    let i = image(&s, h);
    assert!(near(&i.clip, &[(9.5, 39.5), (89.5, 39.5), (49.5, 9.5)]), "{:?}", i.clip);
    // a polygon reaching past the image is cut to it
    s.execute("imageclip", &json!({ "handles": hs, "boundary": [[50, -50], [150, 25], [50, 100]] })).unwrap();
    let c = image(&s, h).clip;
    assert!(c.len() >= 3 && c.iter().all(|p| (-0.5..=99.5).contains(&p.x) && (-0.5..=49.5).contains(&p.y)), "{c:?}");
    s.execute("imageclip", &json!({ "handles": hs, "option": "off" })).unwrap();
    let i = image(&s, h);
    assert!(!i.clipping && i.display & 4 == 0 && i.clip.len() >= 3, "OFF keeps the boundary");
    s.execute("imageclip", &json!({ "handles": hs, "option": "ON" })).unwrap();
    assert!(image(&s, h).clipping && image(&s, h).display & 4 != 0);
    s.execute("imageclip", &json!({ "handles": hs, "option": "delete" })).unwrap();
    let i = image(&s, h);
    assert!(i.clip.is_empty() && !i.clipping && i.display & 4 == 0);
    s.execute("u", &json!({})).unwrap();
    assert!(image(&s, h).clipping, "undo brings the boundary back");
}

#[test]
fn turned_image_takes_a_world_rectangle_as_a_polygon() {
    let mut s = Session::new();
    let img = Image {
        u: Vec3::new(0.0, 1.0, 0.0),
        v: Vec3::new(-1.0, 0.0, 0.0),
        size: Vec2::new(100.0, 50.0),
        path: "turned.png".into(),
        ..Image::default()
    };
    let h = s.add_entity(EntityKind::Image(img)).unwrap();
    s.execute("imageclip", &json!({ "handles": [h.hex()], "boundary": [[-40, 10], [-10, 30]] })).unwrap();
    assert_eq!(image(&s, h).clip.len(), 4);
}

#[test]
fn bad_input_is_refused_never_a_panic() {
    let (mut s, h) = session();
    let hs = json!([h.hex()]);
    for p in [
        json!({ "handles": hs }),
        json!({ "handles": hs, "boundary": "x" }),
        json!({ "handles": hs, "boundary": [[1, 1], [1, 1]] }),
        json!({ "handles": hs, "boundary": [[200, 200], [300, 300]] }),
        json!({ "handles": hs, "boundary": [[0, 0], [1, 1], [2, 2]] }),
        json!({ "handles": hs, "boundary": [[0, 0], ["nan", 1]] }),
        json!({ "handles": hs, "boundary": [[0, 0], [1e308, 1e308], [-1e308, 1e308]] }),
        json!({ "handles": hs, "option": "zap" }),
    ] {
        assert!(s.execute("imageclip", &p).is_err(), "{p}");
    }
    let line = s.execute("line", &json!({ "points": [[0, 0], [1, 1]] })).unwrap();
    let lh = line["handle"].clone();
    assert!(s.execute("imageclip", &json!({ "handles": [lh], "boundary": [[0, 0], [5, 5]] })).is_err());
    assert!(s.execute("imageadjust", &json!({ "handles": [lh], "fade": 10 })).is_err());
    for p in [
        json!({ "handles": hs }),
        json!({ "handles": hs, "fade": 101 }),
        json!({ "handles": hs, "brightness": -1 }),
        json!({ "handles": hs, "contrast": "x" }),
    ] {
        assert!(s.execute("imageadjust", &p).is_err(), "{p}");
    }
    let i = image(&s, h);
    assert!(i.clip.is_empty() && (i.brightness, i.contrast, i.fade) == (50, 50, 0), "nothing changed");
    // A degenerate image is skipped, not divided by zero.
    let flat = s
        .add_entity(EntityKind::Image(Image {
            size: Vec2::new(10.0, 10.0),
            u: Vec3::new(1.0, 0.0, 0.0),
            v: Vec3::new(1.0, 0.0, 0.0),
            ..Image::default()
        }))
        .unwrap();
    assert!(s.execute("imageclip", &json!({ "handles": [flat.hex()], "boundary": [[0, 0], [5, 5]] })).is_err());
}

#[test]
fn typed_rectangular_and_polygonal_boundaries() {
    let (mut s, h) = session();
    s.set_selection(vec![h]);
    typed(&mut s, &["imageclip", "", "", "10,10", "30,40"]);
    assert!(near(&image(&s, h).clip, &[(9.5, 9.5), (29.5, 39.5)]), "{:?}", image(&s, h).clip);
    // A new boundary replaces the old one after the question; a polygon closed with C.
    s.set_selection(vec![h]);
    typed(&mut s, &["imageclip", "n", "y", "p", "10,10", "90,10", "u", "80,10", "50,40", "c"]);
    assert!(near(&image(&s, h).clip, &[(9.5, 39.5), (79.5, 39.5), (49.5, 9.5)]), "{:?}", image(&s, h).clip);
    // Picked at the prompt, then switched off.
    typed(&mut s, &["imageclip"]);
    s.input(cadaddy_engine::Input::Pick(vec![h])).unwrap();
    typed(&mut s, &["off"]);
    assert!(!image(&s, h).clipping);
    assert!(s.running.is_none(), "{}", s.prompt_text());
}

#[test]
fn adjust_json_and_typed() {
    let (mut s, h) = session();
    let r = s.execute("imageadjust", &json!({ "handles": [h.hex()], "brightness": 70, "fade": 20.4 })).unwrap();
    assert_eq!(r["changed"], 1);
    let i = image(&s, h);
    assert_eq!((i.brightness, i.contrast, i.fade), (70, 50, 20));
    s.set_selection(vec![h]);
    typed(&mut s, &["imageadjust", "c", "150", "80"]);
    assert_eq!(image(&s, h).contrast, 80, "out of range asked again, then taken");
    s.set_selection(vec![h]);
    typed(&mut s, &["-imageadjust", "", "35"]);
    assert_eq!(image(&s, h).brightness, 35, "Brightness is the default option");
    s.set_selection(vec![h]);
    typed(&mut s, &["imageadjust", "fade", ""]);
    assert_eq!(image(&s, h).fade, 20, "Enter keeps the value");
}

fn images(d: &Drawing) -> Vec<Image> {
    d.space(&Space::Model)
        .unwrap()
        .iter()
        .filter_map(|e| match &e.kind {
            EntityKind::Image(i) => Some(i.clone()),
            _ => None,
        })
        .collect()
}

#[test]
fn clip_and_adjustments_survive_dxf_and_dwg() {
    let (mut s, h) = session();
    let rect = s.add_entity(EntityKind::Image(image(&s, h))).unwrap();
    s.execute("imageclip", &json!({ "handles": [h.hex()], "boundary": [[10, 10], [90, 10], [50, 40]] })).unwrap();
    s.execute("imageclip", &json!({ "handles": [rect.hex()], "boundary": [[10, 10], [30, 40]] })).unwrap();
    s.execute("imageclip", &json!({ "handles": [rect.hex()], "option": "off" })).unwrap();
    s.execute("imageadjust", &json!({ "handles": [h.hex(), rect.hex()], "brightness": 65, "contrast": 30, "fade": 15 })).unwrap();
    let want = images(s.doc().unwrap());
    let exts: &[&str] = if cfg!(target_arch = "wasm32") { &["x.dxf"] } else { &["x.dxf", "x.dwg"] };
    for name in exts {
        let bytes = cadaddy_io::write(s.doc().unwrap(), name).unwrap();
        let got = images(&cadaddy_io::read(&bytes, name).unwrap());
        assert_eq!(got.len(), 2, "{name}");
        for (a, b) in got.iter().zip(&want) {
            assert!(near(&a.clip, &b.clip.iter().map(|p| (p.x, p.y)).collect::<Vec<_>>()), "{name}: {:?} vs {:?}", a.clip, b.clip);
            assert_eq!(
                (a.clipping, a.display & 4, a.brightness, a.contrast, a.fade),
                (b.clipping, b.display & 4, b.brightness, b.contrast, b.fade),
                "{name}"
            );
        }
    }
}
