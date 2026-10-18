//! CADaddy's own format (.cdy) in the app: new drawings are saved as .cdy by default, the Start
//! page shows a .cdy file's preview, and Options ▸ Open and Save keeps the DWG copy choice.

use std::cell::RefCell;
use std::rc::Rc;

use cadaddy_engine::Session;
use cadaddy_engine::cmd::file::{IoHooks, set_io};
use cadaddy_engine::doc::Drawing;
use cadaddy_ui_egui::{CadApp, SaveFormat, Services};
use egui::{Shape, vec2};
use egui_kittest::Harness;
use serde_json::{Value, json};

fn hooks() {
    set_io(IoHooks { read: |_, _| Ok(Drawing::new_metric()), write: |_, name| Ok(name.as_bytes().to_vec()), plot: None, read_pdf: None });
}

#[test]
fn new_drawings_are_saved_as_cdy_by_default() {
    hooks();
    let dir = std::env::temp_dir().join(format!("cadaddy-ui-cdy-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let asked = Rc::new(RefCell::new(String::new()));
    let a = asked.clone();
    let d = dir.clone();
    let services = Services {
        pick_save: Some(Box::new(move |name: &str| {
            *a.borrow_mut() = name.to_string();
            Some(d.join(name).to_string_lossy().into_owned())
        })),
        ..Services::default()
    };
    let mut app = CadApp::new(Session::new(), services);
    assert_eq!(app.ui.save_format, SaveFormat::Cdy);
    let title = app.session.state().unwrap().title.clone();
    // QSAVE of a drawing with no file asks where, suggesting a .cdy name, and saves there.
    app.run("qsave", Value::Null).unwrap();
    assert_eq!(*asked.borrow(), format!("{title}.cdy"));
    let st = app.session.state().unwrap();
    assert_eq!(st.title, format!("{title}.cdy"));
    assert!(!st.is_dirty());
    // Choosing DWG as the default suggests .dwg for the next new drawing.
    app.run("ui.saveformat", json!({"format": "dwg"})).unwrap();
    app.run("new", json!({})).unwrap();
    // (Saving it needs the app's version-aware writer, not installed here: only the name matters.)
    let _ = app.run("ui.saveas", Value::Null);
    assert!(asked.borrow().ends_with(".dwg"), "{}", asked.borrow());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn dwg_copy_is_an_option_kept_in_the_profile() {
    hooks();
    let mut app = CadApp::new(Session::new(), Services::default());
    let r = app.run("options", json!({"DWGCOPY": 1})).unwrap();
    assert_eq!(r["DWGCOPY"], json!(1), "{r}");
    assert!(cadaddy_engine::cmd::file::dwg_copy());
    let profile = app.profile_json();
    // All or nothing: a bad value elsewhere leaves it as it was.
    assert!(app.run("options", json!({"DWGCOPY": 0, "PICKBOX": "x"})).is_err());
    assert!(cadaddy_engine::cmd::file::dwg_copy());
    app.run("options", json!({"reset": true})).unwrap();
    assert!(!cadaddy_engine::cmd::file::dwg_copy(), "reset turns it off");
    // Restored with the profile at the next start.
    let mut b = CadApp::new(Session::new(), Services::default());
    b.load_profile(&profile);
    assert!(cadaddy_engine::cmd::file::dwg_copy());
    cadaddy_engine::cmd::file::set_dwg_copy(false);
}

/// A tiny PNG.
fn png() -> Vec<u8> {
    let img = image::RgbaImage::from_pixel(8, 6, image::Rgba([200, 30, 30, 255]));
    let mut out = std::io::Cursor::new(Vec::new());
    img.write_to(&mut out, image::ImageFormat::Png).unwrap();
    out.into_inner()
}

fn textured(s: &Shape, out: &mut usize) {
    match s {
        Shape::Mesh(m) if m.texture_id != egui::TextureId::default() => *out += 1,
        Shape::Vec(v) => v.iter().for_each(|s| textured(s, out)),
        _ => {}
    }
}

#[test]
fn the_start_page_shows_a_cdy_files_preview() {
    hooks();
    let asked = Rc::new(RefCell::new(Vec::<String>::new()));
    let a = asked.clone();
    let services = Services {
        thumbnail: Some(Box::new(move |path: &str| {
            a.borrow_mut().push(path.to_string());
            path.ends_with(".cdy").then(png)
        })),
        ..Services::default()
    };
    let mut app = CadApp::new(Session::empty(), services);
    app.ui.recent_files = vec!["/drawings/plan.cdy".into(), "/drawings/old.dxf".into()];
    let mut h = Harness::builder().with_size(vec2(1200.0, 760.0)).build_ui_state(
        |ui, app: &mut CadApp| {
            app.logic(&ui.ctx().clone());
            app.ui(ui);
        },
        app,
    );
    h.run_steps(3);
    let mut images = 0;
    for s in &h.output().shapes {
        textured(&s.shape, &mut images);
    }
    assert_eq!(images, 1, "one preview: the .cdy file's");
    // Each file is asked once, then kept.
    assert_eq!(*asked.borrow(), ["/drawings/plan.cdy", "/drawings/old.dxf"]);
}
