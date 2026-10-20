//! The clipboard with other programs (Q153): copies become a picture as well, and what other
//! programs copy (an image, text) pastes with PASTECLIP and PASTESPEC.

use std::sync::Arc;

use cadaddy_engine::Session;
use cadaddy_engine::cmd::clipboard::{self, SystemContents};
use cadaddy_engine::doc::EntityKind;
use cadaddy_render::images::Raster;
use serde_json::json;

/// A drawing saved (in name) in a fresh folder, so pasted images have somewhere to go.
fn saved_session(tag: &str) -> (Session, std::path::PathBuf) {
    let dir = std::env::temp_dir().join(format!("cadaddy-clip2-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let mut s = Session::new();
    s.state_mut().unwrap().path = Some(dir.join("site plan.dxf").to_string_lossy().into_owned());
    (s, dir)
}

/// A 40 × 20 picture: red on the left half, blue on the right.
fn picture() -> Raster {
    let mut rgba = Vec::new();
    for _y in 0..20 {
        for x in 0..40 {
            rgba.extend(if x < 20 { [200, 30, 30, 255] } else { [30, 30, 200, 255] });
        }
    }
    Raster::new(40, 20, rgba).unwrap()
}

fn images(s: &Session) -> Vec<cadaddy_engine::doc::Image> {
    s.doc().unwrap().model.iter().filter_map(|e| if let EntityKind::Image(i) = &e.kind { Some(i.clone()) } else { None }).collect()
}

#[test]
fn copied_objects_become_a_picture_too() {
    let mut s = Session::new();
    s.execute("line", &json!({ "points": [[0, 0], [10, 0], [10, 5]] })).unwrap();
    s.execute("circle", &json!({ "center": [5, 2.5], "radius": 2 })).unwrap();
    s.execute("selectall", &json!({})).unwrap();
    assert!(clipboard::system_image(&s, 800).is_none(), "nothing copied yet");
    s.execute("copyclip", &json!({})).unwrap();
    let img = clipboard::system_image(&s, 800).expect("a picture of the copy");
    // Twice as wide as tall, the longer side as asked, drawn dark on white.
    assert_eq!(img.width, 800);
    assert!((395..=405).contains(&img.height), "{}", img.height);
    assert_eq!(img.pixel(0, 0), Some([255, 255, 255, 255]), "white background");
    let dark = img.rgba.chunks(4).filter(|p| p[0] < 128 && p[1] < 128 && p[2] < 128).count();
    assert!(dark > 200, "lines drawn: {dark} dark pixels");
    // Too big a request stays at the cap; the PNG decodes back to the same picture.
    assert_eq!(clipboard::system_image(&s, 100_000).unwrap().width, clipboard::IMAGE_SIDE);
    assert_eq!(cadaddy_render::images::decode(&img.encode_png().unwrap()).unwrap(), img);
}

#[test]
fn an_image_from_another_program_pastes_as_a_raster_image_beside_the_drawing() {
    let (mut s, dir) = saved_session("image");
    s.system_clipboard = SystemContents { image: Some(Arc::new(picture())), text: None };
    // PASTECLIP with no CADaddy objects: an image, not an error.
    let r = s.execute("pasteclip", &json!({ "at": [1, 2], "width": 8 })).unwrap();
    assert_eq!(r["as"], "image");
    let file = dir.join("site plan-pasted-1.png");
    let saved = cadaddy_render::images::decode(&std::fs::read(&file).unwrap()).unwrap();
    assert_eq!(saved, picture(), "the picture saved as it was copied");
    let img = images(&s);
    assert_eq!(img.len(), 1);
    let i = &img[0];
    assert_eq!(i.path, "site plan-pasted-1.png", "stored relative to the drawing");
    assert_eq!((i.insert.x, i.insert.y), (1.0, 2.0));
    assert!((i.u.x * 40.0 - 8.0).abs() < 1e-9 && (i.v.y * 20.0 - 4.0).abs() < 1e-9, "8 wide, 4 tall: {i:?}");
    // A second paste takes the next name, never overwriting the first.
    s.execute("pastespec", &json!({ "as": "image" })).unwrap();
    assert!(dir.join("site plan-pasted-2.png").exists());
    assert_eq!(images(&s).len(), 2);
    // Default placement: centred in the view, half the view tall.
    let view = s.state().unwrap().view();
    let last = images(&s).pop().unwrap();
    let (w, h) = (last.u.x * 40.0, last.v.y * 20.0);
    assert!((h - view.height * 0.5).abs() < 1e-9, "{h}");
    assert!((last.insert.x + w / 2.0 - view.center.x).abs() < 1e-9);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn an_unsaved_drawing_asks_to_be_saved_before_an_image_paste() {
    let mut s = Session::new();
    s.system_clipboard = SystemContents { image: Some(Arc::new(picture())), text: None };
    let e = s.execute("pasteclip", &json!({})).unwrap_err().to_string();
    assert!(e.contains("save the drawing first"), "{e}");
    assert!(images(&s).is_empty());
}

#[test]
fn text_from_another_program_pastes_as_multiline_text() {
    let mut s = Session::new();
    s.system_clipboard = SystemContents { image: None, text: Some("PANEL A {120V}\r\nC:\\specs\\notes\n".into()) };
    let r = s.execute("pasteclip", &json!({ "at": [3, 4] })).unwrap();
    assert_eq!(r["as"], "text");
    let m = s.doc().unwrap().model.iter().find_map(|e| if let EntityKind::MText(m) = &e.kind { Some(m.clone()) } else { None }).expect("an MTEXT");
    // Braces and backslashes kept as typed, lines as paragraphs; the trailing break dropped.
    assert_eq!(m.contents, "PANEL A \\{120V\\}\\PC:\\\\specs\\\\notes");
    assert_eq!(cadaddy_fonts::plain_mtext(&m.contents), "PANEL A {120V}\nC:\\specs\\notes");
    assert_eq!((m.insert.x, m.insert.y), (3.0, 4.0));
}

#[test]
fn pastespec_pastes_the_way_asked() {
    let (mut s, dir) = saved_session("spec");
    s.execute("line", &json!({ "points": [[0, 0], [4, 4]] })).unwrap();
    s.execute("selectall", &json!({})).unwrap();
    s.execute("copyclip", &json!({})).unwrap();
    s.system_clipboard = SystemContents { image: None, text: Some("NOTE".into()) };
    // Default: the CADaddy objects.
    assert_eq!(s.execute("pastespec", &json!({ "at": [10, 0] })).unwrap()["as"], "objects");
    // The copied objects drawn as a picture, when no picture came from elsewhere.
    let r = s.execute("pastespec", &json!({ "as": "image", "at": [20, 0], "width": 4 })).unwrap();
    assert_eq!(r["as"], "image");
    assert!(dir.join("site plan-pasted-1.png").exists());
    assert_eq!(s.execute("pastespec", &json!({ "as": "text" })).unwrap()["as"], "text");
    // Asked for what isn't there, or for nothing it knows.
    s.system_clipboard = SystemContents::default();
    assert!(s.execute("pastespec", &json!({ "as": "text" })).is_err());
    assert!(s.execute("pastespec", &json!({ "as": "ole" })).is_err());
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn pastespec_at_the_command_line_asks_how_then_where() {
    let (mut s, dir) = saved_session("cmdline");
    s.execute("line", &json!({ "points": [[0, 0], [4, 4]] })).unwrap();
    s.execute("selectall", &json!({})).unwrap();
    s.execute("copyclip", &json!({})).unwrap();
    s.system_clipboard = SystemContents { image: Some(Arc::new(picture())), text: None };
    s.start("pastespec").unwrap();
    let p = s.current_prompt().expect("asks how");
    assert_eq!(p.keywords, ["Objects", "Image"], "only the ways the clipboard can paste");
    s.input(cadaddy_engine::Input::Text("i".into())).unwrap();
    assert!(s.current_prompt().is_some_and(|p| p.accept.point));
    s.input(cadaddy_engine::Input::Point(cadaddy_engine::geom::Vec2::new(5.0, 5.0))).unwrap();
    assert!(!s.prompting());
    let img = images(&s);
    assert_eq!(img.len(), 1);
    assert_eq!((img[0].insert.x, img[0].insert.y), (5.0, 5.0));
    // Nothing on the clipboard: says so and ends.
    let mut empty = Session::new();
    empty.start("pastespec").ok();
    assert!(!empty.prompting());
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn pasteclip_at_the_command_line_places_another_programs_picture() {
    let (mut s, dir) = saved_session("pasteclip-cmdline");
    s.system_clipboard = SystemContents { image: Some(Arc::new(picture())), text: None };
    // Ctrl+V of a picture: PASTECLIP asks where; a point places it there.
    s.start("pasteclip").unwrap();
    assert!(s.current_prompt().is_some_and(|p| p.accept.point));
    s.input(cadaddy_engine::Input::Point(cadaddy_engine::geom::Vec2::new(2.0, 3.0))).unwrap();
    assert_eq!(images(&s).len(), 1);
    // Enter: centred in the view.
    s.start("pasteclip").unwrap();
    s.input(cadaddy_engine::Input::Enter).unwrap();
    assert!(!s.prompting());
    assert_eq!(images(&s).len(), 2);
    let _ = std::fs::remove_dir_all(dir);
}
