//! A frame of the window at rest costs about the same with a large model open as with an empty
//! drawing (Q6482): what the palettes keep between frames is not copied every frame, and nothing
//! else on the frame path grows with the number of objects or the size of the building spec.
//!
//! The synthetic model: built BIM elements (boxes on levels, trades, categories and types, as a
//! build writes them) and plain drawing objects up to `FRAMECOST_N` (default 120,000; the
//! benchmark is 1,000,000), with a building spec of every level's rooms. The window shows the
//! palettes a coordinator works with docked: Coordination (its Model Tree), Properties, Layers and
//! External References. Run by hand for the tables (`perf.sections` of both drawings), and to
//! save the model for the GUI benchmarks:
//!
//! ```text
//! FRAMECOST_N=1000000 FRAMECOST_SAVE=/tmp/big.cdy cargo test --release -p cadaddy-ui-egui --test frame_cost -- --nocapture
//! ```
//!
//! What is compared is the frame less the drawing's own paint (`perf.frameMs` − `perf.drawMs`):
//! the test has no GPU, so its software canvas paints the visible linework on every frame, where
//! the GPU canvas keeps its last frame while nothing changes (the GUI benchmark measures that).
//!
//! It asserts wall-clock times, so it is a program of its own.

use cadaddy_engine::Session;
use cadaddy_engine::doc::{Drawing, Entity, EntityKind, Line, Mesh, MeshFace, RawTag};
use cadaddy_geom::Vec3;
use cadaddy_ui_egui::{CadApp, Services, framecost};
use serde_json::{Value, json};

const LEVELS: usize = 6;
const SYSTEMS: [&str; 8] = ["SA", "RA", "EA", "CW", "HW", "SAN", "PWR", "LTG"];
const KINDS: [&str; 3] = ["duct", "pipe", "conduit"];
const SIZES: usize = 6;
/// Elements per type: more than the Model Tree lists of one (50).
const PER_TYPE: usize = 55;
const ROOMS_PER_LEVEL: usize = 300;

fn tag(code: i32, v: &str) -> RawTag {
    RawTag { code, value: v.into() }
}

/// A built MEP element: a box 2 ft on a side at `(x, y)` on level `level`, a piece of `system`'s
/// `kind` run of size `size`.
fn element(d: &mut Drawing, level: usize, kind: &str, system: &str, size: usize, x: f64, y: f64) -> Entity {
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
        tag(1000, &format!("size:{}", 4 + 2 * size)),
    ]
    .into();
    e
}

/// The synthetic model of `n` objects (at least its built elements), with its building spec.
fn model(n: usize) -> Drawing {
    let mut d = Drawing::default();
    for l in ["0", "M-SA", "M-RA", "M-EA", "M-CW", "M-HW", "M-SAN", "M-PWR", "M-LTG", "A-WALL"] {
        if !d.layers.iter().any(|x| x.name == l) {
            let mut layer = d.layers.first().cloned().unwrap_or_default();
            layer.name = l.into();
            d.layers.push(layer);
        }
    }
    let mut count = 0usize;
    for level in 0..LEVELS {
        for (k, kind) in KINDS.iter().enumerate() {
            for (s, system) in SYSTEMS.iter().enumerate() {
                for size in 0..SIZES {
                    for i in 0..PER_TYPE {
                        let x = (i * 3) as f64;
                        let y = ((((level * KINDS.len() + k) * SYSTEMS.len() + s) * SIZES + size) * 3) as f64;
                        let e = element(&mut d, level, kind, system, size, x, y);
                        d.model.push(e);
                        count += 1;
                    }
                }
            }
        }
    }
    // Plain drawing objects up to `n`: the plans' linework.
    let cols = 1000usize;
    for i in count..n {
        let (x, y) = (((i % cols) * 4) as f64, -10.0 - ((i / cols) * 4) as f64);
        let h = d.new_handle();
        let mut e = Entity::new(h, EntityKind::Line(Line { a: Vec3::new(x, y, 0.0), b: Vec3::new(x + 3.0, y + 1.0, 0.0) }));
        e.common.layer = "A-WALL".into();
        d.model.push(e);
    }
    let levels: Vec<Value> =
        (0..LEVELS).map(|l| json!({"name": format!("Level {}", l + 1), "elevation_ft": l as f64 * 12.0, "height_ft": 12.0})).collect();
    let rooms: Vec<Value> = (0..LEVELS)
        .flat_map(|l| {
            (0..ROOMS_PER_LEVEL).map(move |r| {
                let (x, y) = ((r % 30) as f64 * 14.0, (r / 30) as f64 * 12.0);
                json!({"name": format!("ROOM {}", r + 1), "number": format!("{}{:03}", l + 1, r + 1), "level": format!("Level {}", l + 1),
                       "outline": [[x, y], [x + 13.0, y], [x + 13.0, y + 11.0], [x, y + 11.0]], "ceiling_ft": 9.0})
            })
        })
        .collect();
    d.building_spec = Some(json!({"levels": levels, "rooms": rooms}).to_string()).into();
    d
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

/// The window with drawing `d` open and the palettes docked; the median of `frames` frames at rest
/// (after it has settled) less the drawing's paint, the median paint, and the frame's parts.
fn at_rest(d: Drawing, frames: usize) -> (f64, f64, Value) {
    let mut s = Session::empty();
    s.open_drawing(d, "Sample Job", None);
    let mut app = CadApp::new(s, Services::default());
    let ctx = egui::Context::default();
    frame(&mut app, &ctx);
    for (op, panel) in [
        ("pin", "coordination"),
        ("open", "coordination"),
        ("pin", "properties"),
        ("open", "properties"),
        ("pin", "layers"),
        ("open", "layers"),
        ("open", "externalReferences"),
    ] {
        let p = if op == "pin" { json!({"operation": op, "panel": panel, "pinned": true}) } else { json!({"operation": op, "panel": panel}) };
        app.run("ui.dock", p).unwrap_or_else(|e| panic!("ui.dock {op} {panel}: {e}"));
        frame(&mut app, &ctx);
    }
    assert!(app.ui.docking.showing(cadaddy_ui_egui::docking::Panel::Coordination), "the Coordination palette is open");
    // Settled: the palettes have read what they show, the drawing is drawn.
    for _ in 0..20 {
        frame(&mut app, &ctx);
    }
    framecost::reset();
    let (mut ms, mut paint): (Vec<f64>, Vec<f64>) = (Vec::with_capacity(frames), Vec::with_capacity(frames));
    for _ in 0..frames {
        frame(&mut app, &ctx);
        ms.push(app.frame_ms - app.canvas.draw_ms);
        paint.push(app.canvas.draw_ms);
    }
    let median = |v: &mut Vec<f64>| {
        v.sort_by(f64::total_cmp);
        v.get(v.len() / 2).copied().unwrap_or(0.0)
    };
    (median(&mut ms), median(&mut paint), framecost::report())
}

fn table(name: &str, median: f64, paint: f64, sections: &Value) {
    println!("\n{name}: at rest (median) frame less the paint {median:.3} ms, the drawing's paint (perf.drawMs) {paint:.3} ms");
    for row in sections["costliest"].as_array().into_iter().flatten() {
        println!("  {:<28} {:>8.3} ms", row[0].as_str().unwrap_or("?"), row[1].as_f64().unwrap_or(0.0));
    }
}

#[test]
fn a_frame_at_rest_costs_the_same_with_a_large_model_open() {
    let n = std::env::var("FRAMECOST_N").ok().and_then(|v| v.parse().ok()).unwrap_or(120_000);
    let big = model(n);
    assert!(big.model.len() >= n.min(LEVELS * KINDS.len() * SYSTEMS.len() * SIZES * PER_TYPE));
    if let Ok(path) = std::env::var("FRAMECOST_SAVE") {
        let mut s = Session::empty();
        s.open_drawing(big.clone(), "Sample Job", None);
        cadaddy_engine::cmd::file::set_io(cadaddy_engine::cmd::file::IoHooks {
            read: |b, name| cadaddy_io::read(b, name).map_err(|e| e.to_string()),
            write: |d, name| cadaddy_io::write(d, name).map_err(|e| e.to_string()),
            plot: None,
            read_pdf: None,
            read_dwf: None,
            write_dwfx: None,
        });
        s.execute("saveas", &json!({ "path": path })).unwrap();
        println!("saved {path}");
    }
    let (empty_ms, empty_paint, empty_sections) = at_rest(Drawing::default(), 40);
    let (big_ms, big_paint, big_sections) = at_rest(big, 40);
    table("empty drawing", empty_ms, empty_paint, &empty_sections);
    table(&format!("synthetic model, {n} objects"), big_ms, big_paint, &big_sections);
    // Twice the empty drawing's at most; below a millisecond the clock's own noise rules.
    let limit = 2.0 * empty_ms.max(1.0);
    assert!(
        big_ms <= limit,
        "a frame at rest: {big_ms:.2} ms with the model open, {empty_ms:.2} ms with an empty drawing (limit {limit:.2}); the parts: {}",
        big_sections["costliest"]
    );
}
