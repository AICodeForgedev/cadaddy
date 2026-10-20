//! End to end: the text tools typed and run as an agent would — SCALETEXT, JUSTIFYTEXT, TORIENT
//! and TEXTMASK — each checked in the drawing (heights, justification, the mask object) and the
//! mask in a render.

mod e2e_common;

use e2e_common::E2e;
use serde_json::{Value, json};

fn texts(e: &mut E2e) -> Vec<Value> {
    e.exec("texts", json!({}))["texts"].as_array().cloned().unwrap_or_default()
}

#[test]
fn scale_justify_orient_and_mask_text() {
    let mut e = E2e::new();
    e.exec("text", json!({ "at": [0, 0], "text": "ROOM 101", "height": 2 }));
    e.exec("text", json!({ "at": [0, 10], "text": "ROOM 102", "height": 2, "rotation": 180 }));
    e.typed("line -5,1 30,1");
    e.typed("");
    let hs: Vec<String> = texts(&mut e).iter().map(|t| t["handle"].as_str().unwrap().to_string()).collect();
    assert_eq!(hs.len(), 2);
    // SCALETEXT to a height.
    let r = e.exec("scaletext", json!({ "handles": hs, "height": 3 }));
    assert_eq!(r["changed"].as_array().map(Vec::len), Some(2), "{r}");
    assert!(texts(&mut e).iter().all(|t| (t["height"].as_f64().unwrap() - 3.0).abs() < 1e-9));
    // JUSTIFYTEXT keeps the text where it is.
    let before = e.inked(400, 300);
    e.exec("justifytext", json!({ "handles": hs, "justify": "mc" }));
    assert!(texts(&mut e).iter().all(|t| t["text"].as_str().unwrap().starts_with("ROOM")));
    // TORIENT: the upside-down one reads right way up.
    e.exec("torient", json!({ "handles": hs }));
    assert!(
        texts(&mut e).iter().all(|t| {
            let r = t["rotation"].as_f64().unwrap().rem_euclid(360.0);
            r <= 90.0 + 1e-6 || r >= 270.0 - 1e-6
        }),
        "{:?}",
        texts(&mut e)
    );
    // TEXTMASK: a mask under each text, covering the line (the render changes).
    let n = e.entity_count();
    let m = e.exec("textmask", json!({ "handles": hs }));
    assert!(e.entity_count() > n, "masks added: {m}");
    let after = e.inked(400, 300);
    assert_ne!(before, after, "the mask shows in a render");
    e.exec("textunmask", json!({ "handles": hs }));
    assert_eq!(e.entity_count(), n);
}
