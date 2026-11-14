//! Opening a dialog or palette shows it within a frame or two however large the drawing (Q6574):
//! what takes long to work out (Quick Select's object types and properties, the Model Tree and
//! the BIM view's counts, the plot window's extents) is worked out off the UI thread or when it is
//! needed, and the dialog is usable meanwhile. On a print set of 96,653 objects Quick Select's
//! first frame took 7.9 s, the Coordination palette's 1.0 s and the Plot dialog's 0.8 s.
//!
//! The synthetic drawing: plain objects of several kinds (lines, circles, arcs, polylines, texts)
//! and built BIM elements (boxes on levels, of trades, categories and types, as a build writes
//! them), `FIRSTFRAME_N` objects in all (default 150,000). What is measured is the dialog's or
//! palette's own part of each frame (`dialogs`, `panel:<id>` in `perf.sections`): the test has
//! no GPU, so its software canvas paints the drawing on every frame.
//!
//! It asserts wall-clock times, so it is a program of its own; its tests take turns.

use std::time::{Duration, Instant};

use cadaddy_engine::Session;
use cadaddy_engine::doc::{Arc, Circle, Drawing, Entity, EntityKind, Line, LwPolyline, Mesh, MeshFace, RawTag};
use cadaddy_geom::{PolyVertex, Vec2, Vec3};
use cadaddy_ui_egui::bgread::{self, READING};
use cadaddy_ui_egui::control::{ControlRequest, Outcome, handle};
use cadaddy_ui_egui::{CadApp, Services, framecost};
use serde_json::{Value, json};

/// The most a dialog's or palette's part of a frame may take, ms (the finding's target).
const LIMIT_MS: f64 = 250.0;

fn tag(code: i32, v: &str) -> RawTag {
    RawTag { code, value: v.into() }
}

/// A built element: a box 2 ft on a side at `(x, y)` on level `level`, a piece of `system`'s
/// `kind` run.
fn element(d: &mut Drawing, level: usize, kind: &str, system: &str, x: f64, y: f64) -> Entity {
    let z = level as f64 * 12.0;
    let vertices: Vec<Vec3> = [(x, y), (x + 2.0, y), (x + 2.0, y + 2.0), (x, y + 2.0)]
        .iter()
        .flat_map(|(px, py)| [Vec3::new(*px, *py, z), Vec3::new(*px, *py, z + 2.0)])
        .collect();
    let faces = [[0, 2, 4, 6], [1, 3, 5, 7], [0, 1, 3, 2], [4, 5, 7, 6], [0, 1, 7, 6], [2, 3, 5, 4]]
        .iter()
        .map(|v| MeshFace { v: v.to_vec(), hidden: 0 })
        .collect();
    let h = d.new_handle();
    let mut e = Entity::new(h, EntityKind::Mesh(Mesh { vertices, faces, kind: Default::default() }));
    e.common.layer = format!("M-{system}").as_str().into();
    e.common.xdata = vec![
        tag(1001, "CADADDY_WALLGEN"),
        tag(1000, "mep"),
        tag(1000, &format!("Level {}", level + 1)),
        tag(1000, &h.hex()),
        tag(1000, &format!("kind:{kind}")),
        tag(1000, &format!("system:{system}")),
        tag(1000, "size:6"),
    ]
    .into();
    e
}

/// The synthetic drawing of `n` objects: a fifth built elements, the rest plain objects.
fn model(n: usize) -> Drawing {
    let mut d = Drawing::default();
    for l in ["M-SA", "M-CW", "E-PWR", "A-WALL", "A-TEXT"] {
        let mut layer = d.layers.first().cloned().unwrap_or_default();
        layer.name = l.into();
        d.layers.push(layer);
    }
    let built = n / 5;
    for i in 0..built {
        let (kind, system) = [("duct", "SA"), ("pipe", "CW"), ("conduit", "PWR")][i % 3];
        let e = element(&mut d, i % 4, kind, system, (i % 500) as f64 * 3.0, (i / 500) as f64 * 3.0);
        d.model.push(e);
    }
    for i in built..n {
        let (x, y) = ((i % 1000) as f64 * 4.0, -10.0 - (i / 1000) as f64 * 4.0);
        let p = Vec3::new(x, y, 0.0);
        let kind = match i % 5 {
            0 => EntityKind::Line(Line { a: p, b: Vec3::new(x + 3.0, y + 1.0, 0.0) }),
            1 => EntityKind::Circle(Circle { center: p, radius: 1.0 }),
            2 => EntityKind::Arc(Arc { center: p, radius: 1.0, start: 0.0, end: 1.5 }),
            3 => EntityKind::LwPolyline(LwPolyline {
                vertices: (0..4).map(|k| PolyVertex { p: Vec2::new(x + k as f64, y + (k % 2) as f64), ..Default::default() }).collect(),
                closed: i % 2 == 0,
                const_width: 0.0,
                elevation: 0.0,
                plinegen: false,
            }),
            _ => serde_json::from_value(json!({ "type": "text", "insert": p, "height": 0.5, "value": format!("NOTE {i}") })).unwrap(),
        };
        let h = d.new_handle();
        let mut e = Entity::new(h, kind);
        e.common.layer = if i % 5 == 4 { "A-TEXT" } else { "A-WALL" }.into();
        d.model.push(e);
    }
    d
}

/// The tests take turns: each measures wall-clock times.
static TURN: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn objects() -> usize {
    std::env::var("FIRSTFRAME_N").ok().and_then(|v| v.parse().ok()).unwrap_or(150_000)
}

fn frame(app: &mut CadApp, ctx: &egui::Context) {
    let mut raw = egui::RawInput { screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1600.0, 1000.0))), ..Default::default() };
    app.raw_input_hook(&mut raw);
    let mut out = ctx.run_ui(raw, |ui| {
        app.logic(ui.ctx());
        app.ui(ui);
    });
    out.textures_delta.clear();
}

/// The labels of the controls the last frame drew (`ui.elements`).
fn labels(app: &mut CadApp, ctx: &egui::Context) -> Vec<String> {
    let (req, _reply) = ControlRequest::new("ui.elements", json!({}));
    let v = match handle(app, ctx, &req) {
        Outcome::Done(v) => v,
        _ => Value::Null,
    };
    let list = v.get("result").cloned().unwrap_or(v);
    let list = list.as_array().cloned().or_else(|| list["elements"].as_array().cloned()).unwrap_or_default();
    list.iter().filter_map(|e| e["label"].as_str().map(str::to_string)).collect()
}

/// The general notes of a print set: `n` paragraphs of multi-line text, wrapped to a column (each
/// is measured line by line, so the drawing's extents take long to work out).
fn notes(d: &mut Drawing, n: usize) {
    let words = "PROVIDE AND INSTALL ALL WORK SHOWN AS REQUIRED BY THE CODES IN FORCE, COORDINATE WITH OTHER TRADES BEFORE ROUGH-IN. ";
    for i in 0..n {
        let (x, y) = ((i % 200) as f64 * 60.0, 50.0 + (i / 200) as f64 * 30.0);
        let kind = serde_json::from_value(json!({
            "type": "mText", "insert": Vec3::new(x, y, 0.0), "height": 0.25, "width": 40.0, "contents": words.repeat(8)
        }));
        let Ok(kind) = kind else { panic!("an MTEXT: {kind:?}") };
        let h = d.new_handle();
        let mut e = Entity::new(h, kind);
        e.common.layer = "A-TEXT".into();
        d.model.push(e);
    }
}

/// The synthetic drawing with a job in it: started on an empty prints folder (it stops at its
/// import), its record of the set's sheets as large as a large set's, so reading the job takes long.
fn with_job(mut d: Drawing, sheets: usize) -> Drawing {
    cadaddy_engine::cmd::file::set_io(cadaddy_engine::cmd::file::IoHooks {
        read: |b, name| cadaddy_io::read(b, name).map_err(|e| e.to_string()),
        write: |d, name| cadaddy_io::write(d, name).map_err(|e| e.to_string()),
        plot: None,
        read_pdf: Some(cadaddy_io::pdf_read::read_json),
        read_dwf: None,
        write_dwfx: None,
    });
    let dir = std::env::temp_dir().join(format!("cadaddy-firstframe-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let mut s = Session::new();
    let _ = s.execute("job.start", &json!({ "pdfs": dir.to_string_lossy() }));
    let _ = std::fs::remove_dir_all(&dir);
    let agent = s.doc().ok().and_then(|d| d.agent.as_deref().map(str::to_string)).unwrap_or_default();
    let mut agent: Value = serde_json::from_str(&agent).unwrap_or(Value::Null);
    assert!(agent["guided_job"]["id"].is_string(), "a job: {agent}");
    let list: Vec<Value> = (0..sheets)
        .map(|i| json!({ "number": format!("X-{i}"), "title": format!("SHEET {i} PLAN"), "page": i, "kind": "plan", "level": format!("Level {}", i % 4 + 1) }))
        .collect();
    agent["guided_job"]["sheets"] = json!({ "sheets": list });
    d.agent = Some(agent.to_string()).into();
    d
}

/// The window on drawing `d`, settled, with nothing open beside the drawing.
fn window_on(d: Drawing) -> (CadApp, egui::Context) {
    let mut s = Session::empty();
    s.open_drawing(d, "Sample Floor Plan", None);
    let mut app = CadApp::new(s, Services::default());
    let ctx = egui::Context::default();
    for _ in 0..3 {
        frame(&mut app, &ctx);
    }
    (app, ctx)
}

/// What opening `target` (`dialog:<id>` or `panel:<id>`) cost: its part of the first frame, the
/// labels that frame drew, the worst of its part over the frames until what it reads came, and
/// the labels drawn then.
fn open(target: &str) -> (f64, Vec<String>, f64, Vec<String>) {
    open_on(target, model(objects()))
}

/// [`open`], on drawing `d`.
fn open_on(target: &str, d: Drawing) -> (f64, Vec<String>, f64, Vec<String>) {
    let (mut app, ctx) = window_on(d);
    let (kind, id) = target.split_once(':').unwrap_or(("dialog", target));
    let part = if kind == "dialog" { "dialogs".to_string() } else { target.to_string() };
    if kind == "dialog" {
        app.ui.dialog = Some(id.to_string());
    } else {
        app.run("ui.dock", json!({ "operation": "open", "panel": id })).unwrap_or_else(|e| panic!("ui.dock open {id}: {e}"));
    }
    let measure = |app: &mut CadApp| {
        framecost::reset();
        frame(app, &ctx);
        framecost::report()["lastMs"][part.as_str()].as_f64().unwrap_or(0.0)
    };
    let first = measure(&mut app);
    let first_labels = labels(&mut app, &ctx);
    let (mut worst, t) = (0.0f64, Instant::now());
    while (bgread::running() || labels(&mut app, &ctx).iter().any(|l| l == READING)) && t.elapsed() < Duration::from_secs(300) {
        std::thread::sleep(Duration::from_millis(20));
        worst = worst.max(measure(&mut app));
    }
    for _ in 0..2 {
        worst = worst.max(measure(&mut app));
    }
    (first, first_labels, worst, labels(&mut app, &ctx))
}

fn check(target: &str, first: f64, worst: f64) {
    println!("{target}: first frame {first:.1} ms, worst frame after {worst:.1} ms ({} objects)", objects());
    assert!(first <= LIMIT_MS, "{target}: its part of the first frame took {first:.0} ms (limit {LIMIT_MS} ms)");
    assert!(worst <= LIMIT_MS, "{target}: a frame while it read took {worst:.0} ms (limit {LIMIT_MS} ms)");
}

#[test]
fn quick_select_opens_at_once_and_fills_in_its_properties() {
    let _turn = TURN.lock().unwrap_or_else(|e| e.into_inner());
    let (first, first_labels, worst, after) = open("dialog:qselect");
    check("Quick Select", first, worst);
    assert!(first_labels.iter().any(|l| l == READING), "the dialog says it is reading: {first_labels:?}");
    assert!(!after.iter().any(|l| l == READING), "{after:?}");
    for p in ["layer", "color", "radius", "length"] {
        assert!(after.iter().any(|l| l == p), "the property {p} once read: {after:?}");
    }
}

#[test]
fn the_coordination_palette_opens_at_once_and_fills_in_its_model_tree() {
    let _turn = TURN.lock().unwrap_or_else(|e| e.into_inner());
    let (first, first_labels, worst, after) = open("panel:coordination");
    check("Coordination", first, worst);
    assert!(first_labels.iter().any(|l| l == READING), "the palette says it is reading: {first_labels:?}");
    assert!(!after.iter().any(|l| l == READING), "{after:?}");
}

#[test]
fn the_plot_dialog_opens_at_once() {
    let _turn = TURN.lock().unwrap_or_else(|e| e.into_inner());
    // With the general notes of a set: its extents take long to measure.
    let mut d = model(objects());
    notes(&mut d, objects() / 5);
    let t = Instant::now();
    let _ = d.extents(&cadaddy_engine::doc::Space::Model);
    let extents = t.elapsed().as_secs_f64() * 1000.0;
    let objects = d.model.len();
    let (first, _, worst, _) = open_on("dialog:plot", d);
    println!("the drawing's extents: {extents:.0} ms ({objects} objects)");
    check("Plot", first, worst);
    // The plot window (the extents, until one is picked) is measured when the plot area becomes a
    // window, not as the dialog opens: its first frame costs a small part of measuring the
    // drawing, on any machine (it cost all of it).
    if extents >= 20.0 {
        assert!(
            first < 0.5 * extents,
            "the Plot dialog's first frame took {first:.0} ms, the drawing's extents {extents:.0} ms: it measured the drawing"
        );
    }
}

#[test]
fn the_job_palette_opens_at_once() {
    let _turn = TURN.lock().unwrap_or_else(|e| e.into_inner());
    let d = with_job(model(objects()), objects() / 2);
    let (first, first_labels, worst, after) = open_on("panel:job", d);
    check("Job", first, worst);
    assert!(first_labels.iter().any(|l| l == READING), "the palette says it is reading: {first_labels:?}");
    assert!(!after.iter().any(|l| l == READING), "{after:?}");
    assert!(after.iter().any(|l| l == "Worklist"), "the job's tabs once read: {after:?}");
}
