//! End to end (0.7.8): Select Color. A hex value typed on the True Color tab gives the RGB the
//! dialog holds and an HSL that converts back to it (worked out here, not with the app's code);
//! applied, it becomes the current colour. A Color Books chip applies its colour too, and picking
//! an index colour sets the current colour to it.

mod e2e_common;

use e2e_common::E2e;
use egui::{Event, Key, Modifiers};
use serde_json::{Value, json};

fn key(e: &mut E2e, k: Key, modifiers: Modifiers) {
    e.events.push(Event::Key { key: k, physical_key: None, pressed: true, repeat: false, modifiers });
    e.events.push(Event::Key { key: k, physical_key: None, pressed: false, repeat: false, modifiers });
    e.frame();
}

fn click_el(e: &mut E2e, el: &Value) {
    let r: Vec<f64> = el["rect"].as_array().into_iter().flatten().filter_map(Value::as_f64).collect();
    e.ok("ui.click", json!({ "x": r[0] + r[2] / 2.0, "y": r[1] + r[3] / 2.0, "space": "screen" }));
    e.pump();
}

/// HSL (H 0–360, S and L 0–100) to RGB, from the textbook formula.
fn hsl_to_rgb(h: f64, s: f64, l: f64) -> [u8; 3] {
    let (s, l) = (s / 100.0, l / 100.0);
    let c = (1.0 - (2.0 * l - 1.0).abs()) * s;
    let hp = (h.rem_euclid(360.0)) / 60.0;
    let x = c * (1.0 - (hp % 2.0 - 1.0).abs());
    let (r, g, b) = match hp as u32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    let m = l - c / 2.0;
    [r, g, b].map(|v| ((v + m) * 255.0).round() as u8)
}

/// The current true colour (CECOLOR_RGB, 0xRRGGBB).
fn current_rgb(e: &mut E2e) -> [u8; 3] {
    let v = e.exec("getvar", json!({ "name": "CECOLOR_RGB" }));
    let n = v["value"].as_u64().or_else(|| v["value"].as_str().and_then(|s| s.parse().ok())).unwrap_or_else(|| panic!("{v}"));
    [(n >> 16) as u8, (n >> 8) as u8, n as u8]
}

fn state<T: Clone + Send + Sync + 'static>(e: &E2e, key: &str) -> Option<T> {
    let id = egui::Id::new("color_dialog_palette").with(key);
    e.ctx.data_mut(|d| d.get_temp::<T>(id))
}

#[test]
fn hex_rgb_and_hsl_agree_and_books_and_index_colours_apply() {
    let mut e = E2e::new();
    e.typed("color");
    assert_eq!(e.dialog().as_deref(), Some("color"));
    assert!(e.click("True Color"));
    e.pump();
    let hex =
        e.elements().into_iter().find(|x| x["role"] == "textedit" && x["label"].as_str().is_some_and(|l| l.starts_with('#'))).expect("the hex field");
    click_el(&mut e, &hex);
    key(&mut e, Key::A, Modifiers::COMMAND);
    e.events.push(Event::Text("#3a7bd5".into()));
    e.frame();
    e.pump();
    let rgb: [u8; 3] = state(&e, "rgb").expect("the dialog's RGB");
    assert_eq!(rgb, [0x3a, 0x7b, 0xd5], "hex → RGB");
    let [h, s, l]: [f64; 3] = state(&e, "hsl").expect("the dialog's HSL");
    let back = hsl_to_rgb(h, s, l);
    assert!(back.iter().zip(rgb).all(|(a, b)| (i16::from(*a) - i16::from(b)).abs() <= 1), "HSL {h:.1},{s:.1},{l:.1} → {back:?}, not {rgb:?}");
    assert!((h - 215.0).abs() < 1.0 && (s - 64.9).abs() < 1.0 && (l - 53.1).abs() < 1.0, "HSL of #3a7bd5: {h},{s},{l}");
    // The RGB model shows the same colour.
    assert!(e.click("RGB"));
    e.pump();
    assert_eq!(state::<[u8; 3]>(&e, "rgb"), Some(rgb), "switching the model keeps the colour");
    assert!(e.click("Apply true color"));
    e.pump();
    assert_eq!(e.dialog(), None, "applying closes the dialog");
    assert_eq!(current_rgb(&mut e), [58, 123, 213], "the current colour");

    // Color Books: a chip applies its colour.
    e.typed("color");
    assert!(e.click("Color Books"));
    e.pump();
    assert!(e.shows("CADaddy Basics"), "the default book");
    assert!(e.click("Brick"), "the Brick chip");
    e.pump();
    assert_eq!(current_rgb(&mut e), [158, 75, 50]);

    // An index colour.
    e.typed("color");
    assert!(e.click("Index Color"));
    e.pump();
    assert!(e.click("Color 5"));
    e.pump();
    let v = e.exec("getvar", json!({ "name": "CECOLOR" }));
    assert!(v.to_string().contains('5'), "CECOLOR is 5 (blue): {v}");

    // For objects: the colour goes to them, not to the current colour.
    let c = e.exec("circle", json!({ "center": [0, 0], "radius": 3 }))["handle"].clone();
    cadaddy_ui_egui::cmddlgs::open_color_dialog(&e.ctx, Some(vec![c.as_str().unwrap().to_string()]));
    e.pump();
    assert!(e.click("Color Books"));
    e.pump();
    assert!(e.click("Brick"));
    e.pump();
    let ent = e.exec("entities", json!({ "type": "Circle" }));
    assert!(ent.to_string().contains("158") && ent.to_string().contains("75"), "the circle is Brick: {ent}");
}
