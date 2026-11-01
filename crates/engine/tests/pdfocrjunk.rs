//! B7-011: a CADaddy plot read back in. The PDF carries each text twice: the stroked lettering
//! you see and the hidden text layer's string at the same place. The SHX OCR must not add its
//! own guesses there (`H`, `6`, `O` for `Hello`), nor read a circle as an `O`.

use cadaddy_engine::Session;
use cadaddy_engine::doc::{EntityKind, Space};
use serde_json::json;

/// A drawing like the soak's seed: room names, a word, two lines of multiline text, circles, an
/// arc and some linework.
fn seed() -> Session {
    let mut s = Session::new();
    for (at, t) in [([0.0, 0.0], "LIVING ROOM"), ([0.0, -40.0], "BEDROOM"), ([120.0, 60.0], "Hello")] {
        s.execute("text", &json!({"at": at, "text": t, "height": 6})).expect("text");
    }
    s.execute("mtext", &json!({"at": [120.0, 0.0], "text": "Multi\\Pline", "height": 6, "width": 80})).expect("mtext");
    for (c, r) in [([-60.0, 30.0], 8.0), ([200.0, -30.0], 5.0)] {
        s.execute("circle", &json!({"center": c, "radius": r})).expect("circle");
    }
    s.execute("arc", &json!({"center": [60.0, -60.0], "radius": 10, "start": 0, "end": 180})).expect("arc");
    for (a, b) in [([-80.0, -80.0], [240.0, -80.0]), ([240.0, -80.0], [240.0, 90.0]), ([240.0, 90.0], [-80.0, 90.0]), ([-80.0, 90.0], [-80.0, -80.0])]
    {
        s.execute("line", &json!({"points": [a, b]})).expect("line");
    }
    s
}

#[test]
fn a_plot_read_back_has_its_own_words_and_no_ocr_junk() {
    let s = seed();
    let opts = json!({"fit": true, "paper": "ANSI A", "landscape": true});
    let pdf = cadaddy_io::pdf::plot(s.doc().expect("drawing"), &Space::Model, &opts).expect("plot");
    let r = cadaddy_io::pdf_read::read(&pdf, &cadaddy_io::pdf_read::PdfOptions::default()).expect("read");
    let visible: Vec<String> = r
        .drawing
        .model
        .iter()
        .filter(|e| e.common.layer.as_str() != cadaddy_io::pdf_read::HIDDEN_TEXT_LAYER)
        .filter_map(|e| match &e.kind {
            EntityKind::Text(t) => Some(t.value.trim().to_string()),
            EntityKind::MText(t) => Some(t.contents.trim().to_string()),
            _ => None,
        })
        .filter(|v| !v.is_empty())
        .collect();
    let words = ["LIVING ROOM", "BEDROOM", "Hello", "Multi", "line", "Multi line", "Multi\\Pline"];
    for v in &visible {
        assert!(words.contains(&v.as_str()), "junk text {v:?} among {visible:?} ({:?})", r.ocr);
    }
    assert!(!visible.iter().any(|v| v == "O" || v == "0"), "a circle read as a letter: {visible:?}");
}
