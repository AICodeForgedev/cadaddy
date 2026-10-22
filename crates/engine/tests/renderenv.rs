//! RENDERENVIRONMENT (Q164): what it takes comes back in the environment it returns; what it
//! can't use is refused, saying what it accepts; RENDER shows the background and the fog; a
//! named view keeps the environment, in the drawing file too.

use cadaddy_engine::Session;
use cadaddy_engine::render::light::{Lighting, tonemap};
use serde_json::{Value, json};

fn dir(name: &str) -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("cadaddy-renderenv-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// A small Radiance file: flat RGBE pixels, exponent 129 (a mantissa of 128 is 1.0).
fn hdr_file(path: &std::path::Path) {
    let mut f = b"#?RADIANCE\nFORMAT=32-bit_rle_rgbe\n\n-Y 2 +X 4\n".to_vec();
    for _ in 0..8 {
        f.extend([128, 64, 32, 129]);
    }
    std::fs::write(path, f).unwrap();
}

fn env(s: &Session) -> Value {
    serde_json::to_value(s.state().unwrap().render_environment.clone()).unwrap()
}

#[test]
fn what_it_takes_comes_back() {
    let d = dir("roundtrip");
    let hdr = d.join("sky.hdr");
    hdr_file(&hdr);
    let mut s = Session::new();
    let r = s
        .execute(
            "renderenvironment",
            &json!({
                "background": {"kind": "solid", "color": [0.1, 0.2, 0.3]},
                "fog": {"enabled": true, "color": [0.5, 0.4, 0.3], "near": 20, "far": 60},
                "ibl": hdr.to_string_lossy(),
                "exposure": 1.5,
                "whiteBalance": {"temperature": 3000, "tint": 0.2}
            }),
        )
        .unwrap();
    let e = &r["environment"];
    assert_eq!(e["background"], json!({"kind": "solid", "color": [0.1, 0.2, 0.3]}), "{e}");
    assert_eq!(e["fog"], json!({"enabled": true, "color": [0.5, 0.4, 0.3], "near": 20.0, "far": 60.0}), "{e}");
    assert_eq!(e["ibl"], json!(hdr.to_string_lossy()), "{e}");
    assert_eq!(e["exposure"], json!(1.5));
    assert_eq!(e["whiteBalance"], json!({"temperature": 3000.0, "tint": 0.2}));
    assert_eq!(&env(&s), e, "kept as returned");
    // A later call changes only what it gives.
    let r = s.execute("renderenvironment", &json!({ "background": {"kind": "gradient", "bottom": [0, 0, 0], "top": [1, 1, 1]} })).unwrap();
    assert_eq!(r["environment"]["fog"]["near"], json!(20.0));
    assert_eq!(r["environment"]["background"]["kind"], json!("gradient"));
}

/// Each refusal names what it accepts, and changes nothing.
fn refused(s: &mut Session, p: Value, says: &[&str]) {
    let before = env(s);
    let e = s.execute("renderenvironment", &p).unwrap_err().to_string();
    for w in says {
        assert!(e.contains(w), "`{w}` in: {e}");
    }
    assert_eq!(env(s), before, "nothing changed by {p}");
}

#[test]
fn an_unknown_background_kind_is_refused() {
    let mut s = Session::new();
    refused(&mut s, json!({ "background": {"kind": "solidd", "color": [1, 0, 0]} }), &["solidd", "sky, solid, gradient or image"]);
}

#[test]
fn a_missing_lighting_file_is_refused() {
    let mut s = Session::new();
    let gone = dir("missing").join("nope.hdr");
    refused(&mut s, json!({ "ibl": gone.to_string_lossy() }), &["can't be read", "Radiance .hdr"]);
}

#[test]
fn a_lighting_file_that_isnt_hdr_is_refused() {
    let mut s = Session::new();
    let bad = dir("invalid").join("photo.hdr");
    std::fs::write(&bad, b"\x89PNG not a radiance file").unwrap();
    refused(&mut s, json!({ "ibl": bad.to_string_lossy() }), &["isn't a Radiance .hdr file", "#?RADIANCE"]);
}

#[test]
fn a_missing_background_image_is_refused() {
    let mut s = Session::new();
    let gone = dir("noimage").join("backdrop.png");
    refused(&mut s, json!({ "background": {"kind": "image", "path": gone.to_string_lossy()} }), &["can't be read", "PNG, JPEG"]);
}

/// Two boxes seen from above: one 10 high (its top is the model's front), one 1 high, 9 behind.
fn two_boxes() -> Session {
    let mut s = Session::new();
    s.execute("box", &json!({ "corner": [0, 0, 0], "length": 10, "width": 10, "height": 10 })).unwrap();
    s.execute("box", &json!({ "corner": [20, 0, 0], "length": 10, "width": 10, "height": 1 })).unwrap();
    s
}

fn render(s: &mut Session) -> std::sync::Arc<cadaddy_engine::render::images::Raster> {
    s.execute("render", &json!({ "preset": "draft", "samples": 2, "width": 120, "height": 60 })).unwrap();
    std::sync::Arc::clone(&s.last_render.as_ref().unwrap().image)
}

fn near(a: [u8; 4], b: [u8; 3], tol: i32) -> bool {
    (0..3).all(|c| (i32::from(a[c]) - i32::from(b[c])).abs() <= tol)
}

#[test]
fn render_shows_the_background_and_the_fog() {
    let mut s = two_boxes();
    let exposure = Lighting::of(&s.state().unwrap().doc, &cadaddy_engine::doc::Space::Model).exposure;
    let plain = render(&mut s);
    // The boxes' tops: the tall one left, the low one right (frame: 15.6 units high in 60 px).
    let (tall, low, corner) = ((22, 30), (98, 30), (2, 2));
    let px = |r: &cadaddy_engine::render::images::Raster, (x, y): (u32, u32)| r.pixel(x, y).unwrap();
    // A solid background: every pixel off the model is that colour (through the tone curve).
    s.execute("renderenvironment", &json!({ "background": {"kind": "solid", "color": [0.6, 0.1, 0.1]} })).unwrap();
    let solid = render(&mut s);
    let want = tonemap([0.6, 0.1, 0.1], exposure);
    assert!(near(px(&solid, corner), want, 2), "{:?} vs {want:?}", px(&solid, corner));
    assert!(!near(px(&plain, corner), want, 10), "the sky was different: {:?}", px(&plain, corner));
    assert!(near(px(&solid, tall), [px(&plain, tall)[0], px(&plain, tall)[1], px(&plain, tall)[2]], 2), "the model unchanged");
    // Fog from 2 to 5 units from the front: the tall box's top (0) is clear, the low one's (9)
    // is the fog colour.
    s.execute(
        "renderenvironment",
        &json!({ "background": {"kind": "sky"}, "fog": {"enabled": true, "color": [0.3, 0.5, 0.7], "near": 2, "far": 5} }),
    )
    .unwrap();
    let fogged = render(&mut s);
    let fog = tonemap([0.3, 0.5, 0.7], exposure);
    let p = px(&plain, tall);
    assert!(near(px(&fogged, tall), [p[0], p[1], p[2]], 2), "before near: {:?} vs {p:?}", px(&fogged, tall));
    assert!(near(px(&fogged, low), fog, 2), "past far: {:?} vs {fog:?}", px(&fogged, low));
    assert!(!near(px(&plain, low), fog, 10), "it wasn't that colour before: {:?}", px(&plain, low));
}

#[test]
fn a_named_view_keeps_the_environment_in_the_drawing_too() {
    let mut s = two_boxes();
    s.execute("renderenvironment", &json!({ "background": {"kind": "solid", "color": [0, 0, 1]}, "fog": {"enabled": true, "near": 3, "far": 9} }))
        .unwrap();
    let saved = env(&s);
    s.execute("view", &json!({ "option": "save", "name": "Dusk" })).unwrap();
    s.execute("renderenvironment", &json!({ "background": {"kind": "gradient"}, "fog": {"enabled": false}, "exposure": 2 })).unwrap();
    assert_ne!(env(&s), saved);
    s.execute("view", &json!({ "option": "restore", "name": "Dusk" })).unwrap();
    assert_eq!(env(&s), saved, "restored with the view");
    // Saved as a drawing and opened again: the view still brings it back.
    cadaddy_engine::cmd::file::set_io(cadaddy_engine::cmd::file::IoHooks {
        read: |b, name| cadaddy_io::read(b, name).map_err(|e| e.to_string()),
        write: |d, name| cadaddy_io::write(d, name).map_err(|e| e.to_string()),
        plot: None,
        read_pdf: None,
        read_dwf: None,
        write_dwfx: None,
    });
    s.execute("renderenvironment", &json!({ "background": {"kind": "sky"} })).unwrap();
    let path = dir("cdy").join("dusk.cdy");
    s.execute("saveas", &json!({ "path": path.to_string_lossy() })).unwrap();
    let mut t = Session::new();
    t.execute("open", &json!({ "path": path.to_string_lossy() })).unwrap();
    t.execute("view", &json!({ "option": "restore", "name": "Dusk" })).unwrap();
    assert_eq!(env(&t), saved, "after save and reopen");
}
