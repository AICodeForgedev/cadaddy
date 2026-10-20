//! The clipboard with other programs (Q153), through the platform layer's test double: each
//! copy is written as CADaddy objects, PNG and bitmap; pictures and text other programs copied
//! paste as a raster image and multiline text; Ctrl+V with no text on the clipboard still pastes.

use std::cell::RefCell;
use std::rc::Rc;

use cadaddy_engine::Session;
use cadaddy_engine::cmd::clipboard;
use cadaddy_engine::cmd::file::{IoHooks, set_io};
use cadaddy_render::images::Raster;
use cadaddy_ui_egui::clipboard::{Format, MemoryClipboard, SystemClipboard, before_paste, dib_encode, mirror, shortcut_commands};
use cadaddy_ui_egui::{CadApp, Services};
use serde_json::json;

/// A 3 × 2 picture with every pixel different.
fn picture() -> Raster {
    let rgba = (0..6u8).flat_map(|i| [i * 40, 255 - i * 40, i * 7, 255]).collect();
    Raster::new(3, 2, rgba).unwrap()
}

/// The in-memory clipboard, shared so a test can look at what the app put there.
#[derive(Clone, Default)]
struct Shared(Rc<RefCell<MemoryClipboard>>);

impl SystemClipboard for Shared {
    fn set(&mut self, formats: &[(Format, Vec<u8>)]) -> bool {
        self.0.borrow_mut().set(formats)
    }
    fn get(&mut self, format: Format) -> Option<Vec<u8>> {
        self.0.borrow_mut().get(format)
    }
}

/// The app with the real file formats (a copy travels as DXF) and the test double clipboard.
fn app_with(clip: &Shared) -> CadApp {
    set_io(IoHooks {
        read: |b, name| cadaddy_io::read(b, name).map_err(|e| e.to_string()),
        write: |d, name| cadaddy_io::write(d, name).map_err(|e| e.to_string()),
        plot: None,
        read_pdf: None,
    });
    let services = Services { system_clipboard: Some(Box::new(clip.clone())), ..Services::default() };
    CadApp::new(Session::new(), services)
}

#[test]
fn the_app_puts_each_copy_on_the_system_clipboard_with_its_picture() {
    let clip = Shared::default();
    let mut app = app_with(&clip);
    let ctx = egui::Context::default();
    app.run("line", json!({ "points": [[0, 0], [10, 0], [10, 5]] })).unwrap();
    app.run("selectall", json!({})).unwrap();
    app.run("copyclip", json!({})).unwrap();
    mirror(&mut app, &ctx);
    let order: Vec<Format> = clip.0.borrow().formats.iter().map(|(f, _)| *f).collect();
    assert_eq!(order, [Format::Drawing, Format::Png, Format::Dib]);
    let png = cadaddy_render::images::decode(&clip.0.borrow_mut().get(Format::Png).unwrap()).unwrap();
    assert_eq!((png.width, png.pixel(0, 0)), (clipboard::IMAGE_SIDE, Some([255, 255, 255, 255])));
    // Once per copy.
    clip.0.borrow_mut().formats.clear();
    mirror(&mut app, &ctx);
    assert!(clip.0.borrow().formats.is_empty());
    // Our own copy read back before a paste stays this drawing's (PASTEORIG still refuses it).
    app.run("selectall", json!({})).unwrap();
    app.run("copyclip", json!({})).unwrap();
    mirror(&mut app, &ctx);
    before_paste(&mut app, "pasteorig");
    assert!(clipboard::from_active(&app.session));
    assert!(app.run("pasteorig", json!({})).is_err());
    let r = app.run("pasteclip", json!({ "at": [20, 0] })).unwrap();
    assert_eq!(r["handles"].as_array().map(Vec::len), Some(2));
}

#[test]
fn pasting_what_other_programs_copied() {
    let clip = Shared::default();
    let mut app = app_with(&clip);
    let dir = std::env::temp_dir().join(format!("cadaddy-uiclip-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    app.session.state_mut().unwrap().path = Some(dir.join("job.dxf").to_string_lossy().into_owned());
    // CADaddy objects copied earlier, then a picture copied in an image editor: the picture is
    // newer and pastes as a raster image saved beside the drawing.
    app.run("line", json!({ "points": [[0, 0], [1, 1]] })).unwrap();
    app.run("selectall", json!({})).unwrap();
    app.run("copyclip", json!({})).unwrap();
    clip.0.borrow_mut().formats = vec![(Format::Dib, dib_encode(&picture()).unwrap())];
    let r = app.run("pasteclip", json!({ "at": [5, 5], "width": 3 })).unwrap();
    assert_eq!(r["as"], "image", "{r}");
    assert_eq!(cadaddy_render::images::decode(&std::fs::read(dir.join("job-pasted-1.png")).unwrap()), Some(picture()));
    // Text from a word processor: multiline text.
    clip.0.borrow_mut().formats = vec![(Format::Text, "Note 1\nNote 2".as_bytes().to_vec())];
    assert_eq!(app.run("pastespec", json!({ "at": [0, -5] })).unwrap()["as"], "text");
    // Empty or unreadable: what the app had stays (the text).
    clip.0.borrow_mut().formats.clear();
    assert_eq!(app.run("pasteclip", json!({})).unwrap()["as"], "text");
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn ctrl_v_with_no_text_on_the_clipboard_still_pastes() {
    let clip = Shared::default();
    let mut app = app_with(&clip);
    let ctx = egui::Context::default();
    let cmd = egui::Modifiers { ctrl: true, command: true, ..Default::default() };
    let release = egui::Event::Key { key: egui::Key::V, physical_key: None, pressed: false, repeat: false, modifiers: cmd };
    let run = |app: &mut CadApp, events: Vec<egui::Event>| {
        let mut fired = Vec::new();
        let raw = egui::RawInput { events, ..Default::default() };
        let mut out = ctx.run_ui(raw, |ui| fired = shortcut_commands(app, ui.ctx()));
        out.textures_delta.clear();
        fired
    };
    // egui sends no Paste for a picture: the release is the paste.
    assert_eq!(run(&mut app, vec![release.clone()]), ["pasteclip"]);
    // With text, the Paste is the paste and its release adds nothing.
    assert_eq!(run(&mut app, vec![egui::Event::Paste("x".into())]), ["pasteclip"]);
    assert!(run(&mut app, vec![release.clone()]).is_empty());
    // A plain V release isn't a paste; without a host clipboard nothing is read this way.
    let plain = egui::Event::Key { key: egui::Key::V, physical_key: None, pressed: false, repeat: false, modifiers: egui::Modifiers::NONE };
    assert!(run(&mut app, vec![plain]).is_empty());
    let mut bare = CadApp::new(Session::new(), Services::default());
    assert!(run(&mut bare, vec![release]).is_empty());
}
