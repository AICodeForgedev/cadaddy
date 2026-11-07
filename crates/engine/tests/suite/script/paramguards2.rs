//! Wrong-typed or out-of-range JSON parameters are refused, never read as "no filter" or
//! silently clamped: QSELECT `type`, FILLET/CHAMFER distances, image sizes, plot resolution
//! and render samples.

use cadaddy_engine::Session;
use serde_json::{Value, json};

fn drawing() -> Session {
    let mut s = Session::new();
    for l in ["LINE 0,0 12,0", "LINE 10,-2 10,12", "CIRCLE 3,3 1"] {
        s.cmdline(l).unwrap();
        s.cmdline("").unwrap();
    }
    s
}

fn handles(s: &Session) -> Vec<String> {
    s.doc().unwrap().model.iter().map(|e| e.handle.hex()).collect()
}

#[test]
fn qselect_refuses_a_wrong_typed_type() {
    for bad in [json!(12345), json!({"a": [1]}), json!([1]), json!(true)] {
        let mut s = drawing();
        let e = s.execute("qselect", &json!({ "type": bad })).unwrap_err().to_string();
        assert!(e.contains("`type` must be an object type name"), "{e}");
        assert!(s.selection().is_empty(), "nothing is selected");
    }
    for key in ["layer", "operator", "property", "mode"] {
        let mut s = drawing();
        assert!(s.execute("qselect", &json!({ key: 7 })).is_err(), "{key}");
        assert!(s.selection().is_empty());
    }
}

#[test]
fn qselect_empty_type_still_means_any_type() {
    let mut s = drawing();
    s.execute("qselect", &json!({ "type": "" })).unwrap();
    assert_eq!(s.selection().len(), 3);
    s.execute("qselect", &json!({ "type": "Circle" })).unwrap();
    assert_eq!(s.selection().len(), 1);
    s.execute("qselect", &json!({})).unwrap();
    assert_eq!(s.selection().len(), 3);
}

#[test]
fn json_chamfer_and_fillet_refuse_negative_distances() {
    for (cmd, params) in [
        ("chamfer", json!({"d1": -2, "d2": -2})),
        ("chamfer", json!({"d1": 2, "d2": -1})),
        ("chamfer", json!({"d1": -2})),
        ("fillet", json!({"radius": -1})),
    ] {
        let mut s = drawing();
        let hs = handles(&s);
        let before = s.doc().unwrap().model.len();
        let mut p = params.clone();
        p["h1"] = Value::String(hs[0].clone());
        p["h2"] = Value::String(hs[1].clone());
        let e = s.execute(cmd, &p).unwrap_err().to_string();
        assert!(e.contains("non-negative distance"), "{cmd} {params}: {e}");
        assert_eq!(s.doc().unwrap().model.len(), before, "{cmd} {params} changed the drawing");
    }
}

#[test]
fn json_chamfer_and_fillet_take_valid_distances() {
    let mut s = drawing();
    let hs = handles(&s);
    s.execute("chamfer", &json!({"d1": 1, "d2": 1, "h1": hs[0], "h2": hs[1]})).unwrap();
    let mut s = drawing();
    let hs = handles(&s);
    s.execute("fillet", &json!({"radius": 0, "h1": hs[0], "h2": hs[1]})).unwrap();
}

#[test]
fn plot_resolution_out_of_range_is_refused() {
    for bad in [json!(100000), json!(49), json!(-5), json!("300"), json!({"a": 1})] {
        let mut s = drawing();
        let e = s.execute("plot", &json!({ "output": "png", "resolution": bad })).unwrap_err().to_string();
        assert!(e.contains("`resolution` must be 50-2400 dpi"), "{bad}: {e}");
    }
}

#[test]
fn model_render_sizes_are_refused_out_of_range() {
    for (k, v) in [("width", 100000), ("width", 0), ("width", 1), ("height", 15), ("height", 8193)] {
        let mut s = drawing();
        let e = s.execute("model.render", &json!({ k: v })).unwrap_err().to_string();
        assert!(e.contains(&format!("`{k}` is 16 to 4096 pixels")), "{k}={v}: {e}");
    }
    let mut s = drawing();
    for bad in [json!(-5), json!("big"), json!(12.5)] {
        assert!(s.execute("model.render", &json!({ "width": bad })).is_err(), "{bad}");
    }
}

#[test]
fn render_samples_have_an_upper_bound() {
    let mut s = drawing();
    let e = s.execute("render", &json!({ "samples": 12345 })).unwrap_err().to_string();
    assert!(e.contains("`samples` is 1 to 4096"), "{e}");
}
