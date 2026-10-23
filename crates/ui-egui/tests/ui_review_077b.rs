//! UI review 0.7.7b (REPORT-0.7.7b-ui) findings that can be checked headless, so they stay
//! fixed: a long prompt scrolls inside the command bar (#2), the Layer Properties Manager's last
//! column scrolls into view (#4), dialogs open centred clear of the Quick Access toolbar and
//! menus (#5), the application menu has Close, Quit and recent drawings (#9), the canvas menu's
//! Redo says Ctrl+Y (#10), External References has a close button (#11), opening a file
//! replaces an untouched Drawing1 (#12), About shows the release's version (#13), the OS title
//! marks unsaved changes with `*` (#14) and the ViewCube shrinks on small windows (#15).
//! The rest are listed with their tests in docs/regressions.md.

use cadaddy_engine::Session;
use cadaddy_engine::cmd::file::{IoHooks, set_io};
use cadaddy_engine::doc::Drawing;
use cadaddy_ui_egui::{CadApp, Services};
use egui::{Order, ViewportCommand, ViewportId};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use std::collections::HashSet;

const SCREEN: egui::Vec2 = egui::vec2(1600.0, 1000.0);

/// One frame of the whole app; returns its output.
fn frame(app: &mut CadApp, ctx: &egui::Context, events: Vec<egui::Event>) -> egui::FullOutput {
    let raw = egui::RawInput { screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, SCREEN)), events, ..Default::default() };
    let mut out = ctx.run_ui(raw, |ui| {
        app.logic(ui.ctx());
        app.ui(ui);
    });
    out.textures_delta.clear();
    out
}

fn new_app() -> CadApp {
    let mut app = CadApp::new(Session::new(), Services::default());
    app.ui.start_tab = false;
    app
}

/// The text painted in a frame.
fn painted(out: &egui::FullOutput) -> String {
    fn collect(shape: &egui::Shape, text: &mut String) {
        match shape {
            egui::Shape::Text(s) => {
                text.push_str(s.galley.text());
                text.push('\n');
            }
            egui::Shape::Vec(shapes) => shapes.iter().for_each(|s| collect(s, text)),
            _ => {}
        }
    }
    let mut text = String::new();
    for s in &out.shapes {
        collect(&s.shape, &mut text);
    }
    text
}

/// #5: dialogs typed at the command line open in the middle of the window, not pinned to the
/// top-left corner over the application menu, Quick Access toolbar and ribbon tabs.
#[test]
fn dialogs_open_centred_clear_of_the_quick_access_toolbar_and_menus() {
    for typed in ["LAYER", "STYLE", "UNITS", "PAGESETUP", "QSELECT", "DSETTINGS", "HATCH", "TABLESTYLE", "MLEADERSTYLE", "LINETYPE", "ABOUT"] {
        let ctx = egui::Context::default();
        let mut app = new_app();
        for _ in 0..3 {
            frame(&mut app, &ctx, Vec::new());
        }
        let before: HashSet<_> = ctx.memory(|m| m.areas().visible_layer_ids()).into_iter().collect();
        app.cmdline(typed);
        for _ in 0..4 {
            frame(&mut app, &ctx, Vec::new());
        }
        let opened: Vec<_> =
            ctx.memory(|m| m.areas().visible_layer_ids()).into_iter().filter(|l| l.order == Order::Middle && !before.contains(l)).collect();
        let rects: Vec<egui::Rect> = opened.iter().filter_map(|l| ctx.memory(|m| m.area_rect(l.id))).filter(|r| r.width() > 150.0).collect();
        let [dialog] = rects.as_slice() else { panic!("{typed}: expected one dialog window, got {rects:?} (dialog {:?})", app.ui.dialog) };
        let centre = SCREEN / 2.0;
        assert!((dialog.center().x - centre.x).abs() < 2.0, "{typed}: not centred across: {dialog:?}");
        // The application menu, Quick Access toolbar and ribbon tabs are the top 60 points.
        assert!(dialog.top() > 60.0 || (dialog.center().y - centre.y).abs() < 2.0, "{typed}: over the toolbar: {dialog:?}");
        assert!(dialog.left() > 100.0, "{typed}: pinned left: {dialog:?}");
    }
}

/// #10: the canvas menu labels Redo with the Windows/Linux shortcut, Ctrl+Y (macOS keeps ⇧⌘Z).
#[test]
fn the_canvas_menu_says_ctrl_y_for_redo() {
    let ctx = egui::Context::default();
    ctx.set_os(egui::os::OperatingSystem::Windows);
    let mut app = new_app();
    frame(&mut app, &ctx, Vec::new());
    let at = egui::pos2(800.0, 500.0);
    let button = |pressed| egui::Event::PointerButton { pos: at, button: egui::PointerButton::Secondary, pressed, modifiers: egui::Modifiers::NONE };
    frame(&mut app, &ctx, vec![egui::Event::PointerMoved(at)]);
    frame(&mut app, &ctx, vec![button(true)]);
    frame(&mut app, &ctx, vec![button(false)]);
    let text = painted(&frame(&mut app, &ctx, Vec::new()));
    assert!(text.contains("Redo") && text.contains("Ctrl+Y"), "the menu's Redo and its shortcut:\n{text}");
    assert!(!text.contains("Ctrl+Shift+Z"), "{text}");
}

/// The titles the app asked the OS window to show, in order.
fn os_titles(out: &egui::FullOutput) -> Vec<String> {
    out.viewport_output
        .get(&ViewportId::ROOT)
        .into_iter()
        .flat_map(|v| &v.commands)
        .filter_map(|c| match c {
            ViewportCommand::Title(t) => Some(t.clone()),
            _ => None,
        })
        .collect()
}

/// #14: the OS window title shows `*` while the drawing has unsaved changes, like its tab.
#[test]
fn the_os_title_marks_unsaved_changes() {
    let ctx = egui::Context::default();
    let mut app = new_app();
    assert_eq!(os_titles(&frame(&mut app, &ctx, Vec::new())), ["CADaddy - Drawing1"]);
    app.cmdline("LINE 0,0 10,0");
    app.cmdline("");
    assert!(app.session.docs[0].is_dirty());
    assert_eq!(os_titles(&frame(&mut app, &ctx, Vec::new())), ["CADaddy - Drawing1*"]);
    // Unchanged: not sent again.
    assert_eq!(os_titles(&frame(&mut app, &ctx, Vec::new())), Vec::<String>::new());
    // Undone back to the saved drawing: the mark goes.
    app.cmdline("U");
    assert!(!app.session.docs[0].is_dirty());
    assert_eq!(os_titles(&frame(&mut app, &ctx, Vec::new())), ["CADaddy - Drawing1"]);
}

fn settle(app: &mut CadApp) {
    let t = web_time::Instant::now();
    while app.opening.busy() && t.elapsed() < web_time::Duration::from_secs(10) {
        app.finish_opening();
        std::thread::sleep(web_time::Duration::from_millis(5));
    }
    app.finish_opening();
}

/// #12: opening a file replaces the untouched Drawing1 the app started with; a Drawing1 with
/// work in it stays.
#[test]
fn opening_a_file_replaces_an_untouched_drawing1() {
    set_io(IoHooks {
        read: |_, _| Ok(Drawing::new_metric()),
        write: |_, _| Ok(Vec::new()),
        plot: None,
        read_pdf: None,
        read_dwf: None,
        write_dwfx: None,
    });
    let dir = std::env::temp_dir().join(format!("cadaddy-ui077b-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let file = |name: &str| {
        let p = dir.join(name);
        std::fs::write(&p, b"0\nEOF\n").unwrap();
        p.to_string_lossy().to_string()
    };
    let (plan, other) = (file("Sample Floor Plan.dxf"), file("Sample Site.dxf"));
    let titles = |app: &CadApp| app.session.docs.iter().map(|d| d.title.clone()).collect::<Vec<_>>();

    let mut app = new_app();
    assert_eq!(titles(&app), ["Drawing1"]);
    app.open_path(&plan);
    settle(&mut app);
    assert_eq!(titles(&app), ["Sample Floor Plan.dxf"], "the pristine Drawing1 is replaced");

    let mut app = new_app();
    app.cmdline("LINE 0,0 10,0");
    app.cmdline("");
    app.open_path(&other);
    settle(&mut app);
    assert_eq!(titles(&app), ["Drawing1", "Sample Site.dxf"], "a Drawing1 with work in it stays");
    let _ = std::fs::remove_dir_all(&dir);
}

/// #13: About shows the release's own version (the workspace version every crate and the app
/// share), not an older one.
#[test]
fn about_shows_the_build_version() {
    let manifest = std::fs::read_to_string(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../Cargo.toml")).unwrap();
    let version = manifest
        .split("[workspace.package]")
        .nth(1)
        .and_then(|s| s.lines().find_map(|l| l.trim().strip_prefix("version = \"")?.strip_suffix('"')))
        .unwrap()
        .to_string();
    let mut app = new_app();
    app.ui.dialog = Some("about".into());
    let mut h = Harness::builder().with_size(SCREEN).build_ui_state(
        |ui, app: &mut CadApp| {
            app.logic(&ui.ctx().clone());
            app.ui(ui);
        },
        app,
    );
    h.run_steps(3);
    h.get_by_label(&format!("Version {version}"));
}

/// #2: a long prompt (PURGE's keyword list) scrolls inside the command bar instead of running
/// past it across the drawing and under the palettes.
#[test]
fn a_long_prompt_stays_inside_the_command_bar() {
    let ctx = egui::Context::default();
    let mut app = new_app();
    let small = egui::vec2(1024.0, 640.0);
    let run = |app: &mut CadApp| {
        let raw = egui::RawInput { screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, small)), ..Default::default() };
        let mut out = ctx.run_ui(raw, |ui| {
            app.logic(ui.ctx());
            app.ui(ui);
        });
        out.textures_delta.clear();
        out
    };
    run(&mut app);
    app.cmdline("-PURGE");
    let prompt = app.session.prompt_text();
    let out = run(&mut app);
    let mut bar = None;
    let mut pieces = Vec::new();
    fn walk(shape: &egui::Shape, clip: egui::Rect, bar: &mut Option<egui::Rect>, pieces: &mut Vec<(String, egui::Rect, egui::Rect)>) {
        match shape {
            egui::Shape::Text(t) if t.galley.text() == ">_" => *bar = Some(t.visual_bounding_rect()),
            egui::Shape::Text(t) => pieces.push((t.galley.text().to_string(), t.visual_bounding_rect(), clip)),
            egui::Shape::Vec(v) => v.iter().for_each(|s| walk(s, clip, bar, pieces)),
            _ => {}
        }
    }
    for s in &out.shapes {
        walk(&s.shape, s.clip_rect, &mut bar, &mut pieces);
    }
    let bar = bar.expect("the command bar's >_");
    // The prompt's pieces: on the bar's row (scrolled left, the first ones start before its >_).
    let row: Vec<_> = pieces.iter().filter(|(_, r, _)| (r.center().y - bar.center().y).abs() < 6.0).collect();
    assert!(row.iter().map(|(t, _, _)| t.as_str()).collect::<String>().len() > 40, "{prompt:?}: {row:?}");
    let full: f32 = row.iter().map(|(_, r, _)| r.width()).sum();
    for (text, _, clip) in &row {
        assert!(clip.left() >= bar.left() && clip.width() < full, "{text:?} is not clipped to the bar: clip {clip:?}, prompt {full} wide");
        assert!(clip.right() < small.x - 200.0, "{text:?} runs under the palettes: {clip:?}");
    }
}

/// #4: on a small window the Layer Properties Manager's table scrolls sideways, so its last
/// column (Description) can be brought into view instead of being cut off at the window edge.
#[test]
fn the_layer_managers_last_column_scrolls_into_view_on_a_small_window() {
    let ctx = egui::Context::default();
    let mut app = new_app();
    let small = egui::vec2(1024.0, 640.0);
    let run = |app: &mut CadApp, events: Vec<egui::Event>| {
        let raw = egui::RawInput { screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, small)), events, ..Default::default() };
        let mut out = ctx.run_ui(raw, |ui| {
            app.logic(ui.ctx());
            app.ui(ui);
        });
        out.textures_delta.clear();
        out
    };
    run(&mut app, Vec::new());
    app.cmdline("LAYER");
    for _ in 0..4 {
        run(&mut app, Vec::new());
    }
    /// The Description header: where it is drawn and the area it is clipped to.
    fn header(out: &egui::FullOutput) -> Option<(egui::Rect, egui::Rect)> {
        fn walk(shape: &egui::Shape, clip: egui::Rect, found: &mut Option<(egui::Rect, egui::Rect)>) {
            match shape {
                egui::Shape::Text(t) if t.galley.text() == "Description" => *found = Some((t.visual_bounding_rect(), clip)),
                egui::Shape::Vec(v) => v.iter().for_each(|s| walk(s, clip, found)),
                _ => {}
            }
        }
        let mut found = None;
        for s in &out.shapes {
            walk(&s.shape, s.clip_rect, &mut found);
        }
        found
    }
    // The table is wider than a small window: its last column is out of view (not drawn, or cut).
    let first = header(&run(&mut app, Vec::new()));
    assert!(first.is_none_or(|(at, clip)| at.right() > clip.right()), "{first:?}");
    // Scroll sideways over the table (the middle of the window is over its rows).
    let over = egui::pos2(small.x / 2.0, small.y / 2.0);
    let mut seen = None;
    for _ in 0..20 {
        let wheel = egui::Event::MouseWheel {
            unit: egui::MouseWheelUnit::Point,
            delta: egui::vec2(-200.0, 0.0),
            modifiers: egui::Modifiers::NONE,
            phase: egui::TouchPhase::Move,
        };
        seen = header(&run(&mut app, vec![egui::Event::PointerMoved(over), wheel]));
    }
    let (at, clip) = seen.expect("the header after scrolling");
    assert!(at.right() <= clip.right() + 0.5 && at.left() >= clip.left(), "Description scrolled into view: {at:?} in {clip:?}");
}

/// #9: the application menu has Close, Close All, Quit and the recent drawings.
#[test]
fn the_application_menu_closes_quits_and_lists_recent_drawings() {
    let mut app = new_app();
    app.ui.recent_files = vec!["drawings/Sample Floor Plan.dxf".into()];
    let mut h = Harness::builder().with_size(SCREEN).build_ui_state(
        |ui, app: &mut CadApp| {
            app.logic(&ui.ctx().clone());
            app.ui(ui);
        },
        app,
    );
    h.run_steps(2);
    h.get_by_label("Application menu").click();
    h.run_steps(2);
    for label in ["Open Recent", "Sample Floor Plan.dxf", "Close", "Close All", "Quit CADaddy"] {
        h.get_by_label(label);
    }
}

/// #11: External References has a visible close button, not only its tab's right-click menu.
#[test]
fn external_references_has_a_close_button() {
    let mut app = new_app();
    app.ui.in_window_menu = false;
    assert!(app.cmdline("XREF").is_none());
    assert!(app.ui.docking.layout.contains(&cadaddy_ui_egui::docking::Panel::ExternalRefs));
    let mut h = Harness::builder().with_size(SCREEN).build_ui_state(
        |ui, app: &mut CadApp| {
            app.logic(&ui.ctx().clone());
            app.ui(ui);
        },
        app,
    );
    h.run_steps(4);
    h.get_by_label("×").click();
    h.run_steps(3);
    assert!(!h.state().ui.docking.layout.contains(&cadaddy_ui_egui::docking::Panel::ExternalRefs), "closed");
}

/// #15: the ViewCube shrinks on a small window (at 1024×640 the full-size cube covered about a
/// quarter of the drawing area's width).
#[test]
fn the_viewcube_shrinks_on_a_small_window() {
    use cadaddy_ui_egui::viewcube::Layout;
    let size = |w: f32, h: f32| {
        let l = Layout::new(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(w, h)));
        l.ring * 2.0
    };
    let big = size(1600.0, 900.0);
    let small = size(760.0, 420.0);
    assert!(small < big * 0.7, "{small} vs {big}");
    assert!(small <= 760.0 * 0.15, "{small} of a 760-wide drawing area");
}
