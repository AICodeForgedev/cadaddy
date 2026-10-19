//! The Publish dialog's batch plot: "Plot a Folder of Drawings..." plots every drawing in the
//! picked folder to its own PDF in a PDF folder inside it.

use cadaddy_engine::Session;
use cadaddy_engine::cmd::file::{IoHooks, set_io};
use cadaddy_ui_egui::{CadApp, Services};
use egui::vec2;
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;

fn folder() -> std::path::PathBuf {
    std::env::temp_dir().join(format!("cadaddy-publish-folder-{}", std::process::id()))
}

#[test]
fn a_folder_is_plotted_from_the_publish_dialog() {
    let dir = folder();
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    // Two drawings the fake reader below "reads"; the fake plotter writes the options it got.
    std::fs::write(dir.join("a.dxf"), b"a").unwrap();
    std::fs::write(dir.join("b.dwg"), b"b").unwrap();
    set_io(IoHooks {
        read: |_, _| {
            let mut d = cadaddy_doc::Drawing::new_metric();
            d.add(
                &cadaddy_doc::Space::Model,
                Default::default(),
                cadaddy_doc::EntityKind::Line(cadaddy_doc::Line { a: cadaddy_geom::Vec3::ZERO, b: cadaddy_geom::Vec3::new(1.0, 1.0, 0.0) }),
            )
            .unwrap();
            Ok(d)
        },
        write: |_, name| Ok(name.as_bytes().to_vec()),
        plot: Some(|_, _, opts| Ok(format!("%PDF {opts}").into_bytes())),
        read_pdf: None,
    });
    let services = Services { pick_folder: Some(Box::new(|| Some(folder().to_string_lossy().into_owned()))), ..Services::default() };
    let mut app = CadApp::new(Session::new(), services);
    app.ui.start_tab = false;
    app.cmdline("PUBLISH");
    let mut h = Harness::builder().with_size(vec2(1400.0, 900.0)).build_ui_state(
        |ui, app: &mut CadApp| {
            app.logic(&ui.ctx().clone());
            app.ui(ui);
        },
        app,
    );
    h.run_steps(3);
    h.get_by_label("Plot stamp").click();
    h.run_steps(2);
    h.get_by_label("Plot a Folder of Drawings...").click();
    h.run_steps(3);
    let out = dir.join("PDF");
    for n in ["a.pdf", "b.pdf"] {
        let pdf = String::from_utf8(std::fs::read(out.join(n)).unwrap_or_default()).unwrap();
        assert!(pdf.starts_with("%PDF") && pdf.contains("\"stamp\":true"), "{n}: {pdf}");
    }
    let _ = std::fs::remove_dir_all(&dir);
}
