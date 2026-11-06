//! MLEADER sizes come from the multileader style and MLEADERSCALE, never DIMSCALE: 1:1 on a
//! layout's sheet with DIMSCALE 48.

use cadaddy_engine::Session;
use cadaddy_engine::doc::{EntityKind, Handle};
use serde_json::json;

fn mleader(s: &mut Session) -> Box<cadaddy_engine::doc::MLeader> {
    let r = s.execute("mleader", &json!({ "points": [[1, 1], [3, 2]], "text": "Check" })).unwrap();
    let h = Handle::parse_hex(r["handle"].as_str().unwrap()).unwrap();
    let EntityKind::MLeader(m) = &s.doc().unwrap().entity(h).unwrap().kind else { panic!("a multileader") };
    m.clone()
}

#[test]
fn mleader_ignores_dimscale_and_uses_mleaderscale() {
    let mut s = Session::new();
    s.execute("setvar", &json!({ "name": "DIMSCALE", "value": 48 })).unwrap();
    s.execute("layout", &json!({ "option": "set", "name": "Layout1" })).unwrap();
    let m = mleader(&mut s);
    assert!((m.arrow_size - 0.18).abs() < 1e-9, "the style's size in paper space: {}", m.arrow_size);
    assert!((m.text.as_ref().unwrap().height - 0.18).abs() < 1e-9);
    assert!((m.dogleg - 0.36).abs() < 1e-9);

    // Model space too: DIMSCALE is the dimensions'.
    s.execute("layout", &json!({ "option": "set", "name": "Model" })).unwrap();
    assert!((mleader(&mut s).arrow_size - 0.18).abs() < 1e-9);

    // MLEADERSCALE scales them; 0 (scale to the layout) is 1:1.
    s.execute("setvar", &json!({ "name": "MLEADERSCALE", "value": 24 })).unwrap();
    let m = mleader(&mut s);
    assert!((m.arrow_size - 0.18 * 24.0).abs() < 1e-9, "{}", m.arrow_size);
    assert!((m.text.as_ref().unwrap().height - 0.18 * 24.0).abs() < 1e-9);
    s.execute("setvar", &json!({ "name": "MLEADERSCALE", "value": 0 })).unwrap();
    assert!((mleader(&mut s).arrow_size - 0.18).abs() < 1e-9);

    // An annotative style's sizes are paper sizes whatever MLEADERSCALE says.
    s.execute("setvar", &json!({ "name": "MLEADERSCALE", "value": 24 })).unwrap();
    s.execute("mleaderstyle", &json!({ "name": "Notes", "annotative": true })).unwrap();
    assert!((mleader(&mut s).arrow_size - 0.18).abs() < 1e-9);
}
