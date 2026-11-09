//! End to end: LIGHTBAKE on a small model (a slab and a box) — the bake reports what it baked,
//! the Realistic picture of the model changes with the baked light, and clearing drops the
//! lightmap (the picture is the unbaked one again, and the next bake starts over).

use crate::e2e_common::E2e;
use serde_json::json;

/// The Realistic view exported as a PNG (`export`, as an agent saves a picture of it), decoded.
fn realistic(e: &mut E2e) -> image::RgbaImage {
    let r = e.exec("export", json!({ "format": "png" }));
    let png = cadaddy_engine::cmd::file::base64_decode(r["data"].as_str().unwrap_or_default()).expect("the export's PNG");
    image::load_from_memory(&png).expect("a PNG").to_rgba8()
}

/// Pixels whose colour differs by more than 8 (of 255) in a channel.
fn changed(a: &image::RgbaImage, b: &image::RgbaImage) -> usize {
    assert_eq!(a.dimensions(), b.dimensions());
    a.pixels().zip(b.pixels()).filter(|(p, q)| p.0.iter().zip(q.0).any(|(x, y)| x.abs_diff(y) > 8)).count()
}

/// Of the pixels the model covers, at least this share must change with the bake (9 % do with
/// this model and bake; none did when the export left the baked light out).
const BAKE_SHOWS: f64 = 0.03;

#[test]
fn bake_and_clear() {
    let mut e = E2e::new();
    e.exec("box", json!({ "corner": [0, 0, -1], "length": 200, "width": 200, "height": 1 }));
    e.exec("box", json!({ "corner": [80, 80, 0], "length": 40, "width": 40, "height": 40 }));
    e.typed("vpoint 1,-1,1");
    e.typed("vscurrent realistic");
    let before = realistic(&mut e);
    let model = crate::e2e_common::inked(&before);
    assert!(model > 10_000, "the model shows: {model} pixels");

    let r = e.exec("lightbake", json!({ "quality": "draft", "texel": 20 }));
    assert!(r["baked"].as_u64().unwrap_or(0) >= 1 && r["texels"].as_u64().unwrap_or(0) > 0, "{r}");
    assert!(e.inked(400, 300) > 500, "the model renders");
    let baked = realistic(&mut e);
    let lit = changed(&before, &baked);
    assert!(lit as f64 > BAKE_SHOWS * model as f64, "the baked light shows: {lit} of {model} model pixels changed");

    let again = e.exec("lightbake", json!({ "quality": "draft", "texel": 20 }));
    assert!(again["kept"].as_u64().unwrap_or(0) >= 1, "nothing changed: kept {again}");

    let c = e.exec("lightbake", json!({ "clear": true }));
    assert_eq!(c["cleared"], json!(true), "{c}");
    let cleared = realistic(&mut e);
    assert_eq!(changed(&before, &cleared), 0, "cleared: the unbaked picture again");
    let fresh = e.exec("lightbake", json!({ "quality": "draft", "texel": 20 }));
    assert!(fresh["kept"] == json!(0) && fresh["baked"].as_u64().unwrap_or(0) >= 1, "cleared: nothing kept from before: {fresh}");
}
