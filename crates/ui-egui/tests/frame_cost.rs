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
//! The Plans to Model palette is measured the same way on its own (Q6542): open on a building spec
//! of tens of thousands of parts, its part of the frame (`panel:plansToModel`) costs about what it
//! does on a spec of one part, because only the rows on screen are laid out.
//!
//! It asserts wall-clock times, so it is a program of its own; its tests take turns.

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

/// The tests take turns: each measures wall-clock times.
static TURN: std::sync::Mutex<()> = std::sync::Mutex::new(());

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
    let _turn = TURN.lock().unwrap_or_else(|e| e.into_inner());
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

/// The Plans to Model palette docked, on a drawing whose building spec has `rooms` rooms (and six
/// levels): the median of `frames` frames of its own part (`panel:plansToModel`), in ms.
fn palette_at_rest(rooms: usize, frames: usize) -> f64 {
    let mut d = Drawing::default();
    let levels: Vec<Value> =
        (0..LEVELS).map(|l| json!({"name": format!("Level {}", l + 1), "elevation_ft": l as f64 * 12.0, "height_ft": 12.0})).collect();
    let rooms: Vec<Value> = (0..rooms)
        .map(|r| {
            let l = r % LEVELS;
            let (x, y) = (((r / LEVELS) % 40) as f64 * 14.0, ((r / LEVELS) / 40) as f64 * 12.0);
            json!({"name": format!("ROOM {}", r + 1), "number": format!("{}", r + 1), "level": format!("Level {}", l + 1),
                   "outline": [[x, y], [x + 13.0, y], [x + 13.0, y + 11.0], [x, y + 11.0]], "ceiling_ft": 9.0})
        })
        .collect();
    d.building_spec = Some(json!({"levels": levels, "rooms": rooms}).to_string()).into();
    let mut s = Session::empty();
    s.open_drawing(d, "Sample Job", None);
    let mut app = CadApp::new(s, Services::default());
    let ctx = egui::Context::default();
    frame(&mut app, &ctx);
    for p in [json!({"operation": "pin", "panel": "plansToModel", "pinned": true}), json!({"operation": "open", "panel": "plansToModel"})] {
        app.run("ui.dock", p.clone()).unwrap_or_else(|e| panic!("ui.dock {p}: {e}"));
        frame(&mut app, &ctx);
    }
    assert!(app.ui.docking.showing(cadaddy_ui_egui::docking::Panel::PlansToModel), "the Plans to Model palette is open");
    // Settled: the palette has read the spec.
    for _ in 0..10 {
        frame(&mut app, &ctx);
    }
    framecost::reset();
    let mut ms: Vec<f64> = (0..frames)
        .map(|_| {
            frame(&mut app, &ctx);
            framecost::report()["lastMs"]["panel:plansToModel"].as_f64().unwrap_or(0.0)
        })
        .collect();
    ms.sort_by(f64::total_cmp);
    ms.get(ms.len() / 2).copied().unwrap_or(0.0)
}

#[test]
fn the_plans_to_model_palette_costs_what_is_on_screen() {
    let _turn = TURN.lock().unwrap_or_else(|e| e.into_inner());
    let parts = std::env::var("FRAMECOST_PARTS").ok().and_then(|v| v.parse().ok()).unwrap_or(30_000);
    let one = palette_at_rest(1, 30);
    let many = palette_at_rest(parts, 30);
    println!("\nPlans to Model palette at rest (median panel:plansToModel): one room {one:.3} ms, {parts} rooms {many:.3} ms");
    // A few times the one-part spec's at most; below half a millisecond the clock's noise rules.
    let limit = 4.0 * one.max(0.5);
    assert!(many <= limit, "the palette's frame: {many:.2} ms on {parts} rooms, {one:.2} ms on one (limit {limit:.2})");
}

/// The dialogs that list what the building spec or the drawing holds (Q6573): the RFI List,
/// Element Tags, MEP Review and the Layer Properties Manager, each with the command it lists from
/// (`None`: the dialog reads the drawing itself).
const LIST_DIALOGS: [(&str, Option<&str>); 4] =
    [("rfilist", Some("rfi.list")), ("elementtags", Some("tags.list")), ("mepreview", Some("mep.list")), ("layers", None)];

/// A drawing whose building spec has `n` RFIs, element tags and MEP runs, and `n` layers with
/// plain objects on them (`objects` in all).
fn listed(n: usize, objects: usize) -> Drawing {
    let mut d = Drawing::default();
    for i in 0..n {
        let mut layer = d.layers.first().cloned().unwrap_or_default();
        layer.name = format!("E-LIST-{i:05}").as_str().into();
        d.layers.push(layer);
    }
    for i in 0..objects {
        let (x, y) = (((i % 1000) * 4) as f64, ((i / 1000) * 4) as f64);
        let h = d.new_handle();
        let mut e = Entity::new(h, EntityKind::Line(Line { a: Vec3::new(x, y, 0.0), b: Vec3::new(x + 3.0, y + 1.0, 0.0) }));
        e.common.layer = format!("E-LIST-{:05}", i % n.max(1)).as_str().into();
        d.model.push(e);
    }
    let level = |i: usize| format!("Level {}", i % LEVELS + 1);
    let levels: Vec<Value> =
        (0..LEVELS).map(|l| json!({"name": format!("Level {}", l + 1), "elevation_ft": l as f64 * 12.0, "height_ft": 12.0})).collect();
    let severities = ["high", "medium", "low"];
    let rfis: Vec<Value> = (0..n)
        .map(|i| {
            json!({"kind": "sleeve_no_route", "severity": severities[i % 3], "level": level(i), "at": [i as f64, 10.0],
                   "sheets": ["X-101"], "issue": format!("Sleeve {i} gives no route"), "assumed": "Not routed."})
        })
        .collect();
    let tags: Vec<Value> = (0..n)
        .map(|i| {
            json!({"id": format!("device:{}:{i},0", level(i)), "kind": "receptacle", "level": level(i), "at_ft": [i as f64, 0.0],
                   "fields": {"circuit": {"value": format!("{}", i % 42 + 1), "source": "plan"}},
                   "issues": if i % 4 == 0 { json!(["untagged"]) } else { json!([]) }})
        })
        .collect();
    let systems: Vec<Value> =
        SYSTEMS.iter().map(|s| json!({"name": s, "layer": format!("M-{s}"), "kind": "duct", "size_in": [12.0, 8.0], "bottom_ft": 9.0})).collect();
    let runs: Vec<Value> = (0..n)
        .map(|i| {
            json!({"system": SYSTEMS[i % SYSTEMS.len()], "level": level(i), "points": [[i as f64, 0.0], [i as f64, 20.0]],
                   "size_in": [12.0, 8.0], "bottoms_ft": [9.0], "objects": [], "id": format!("R{}", i + 1)})
        })
        .collect();
    let spec = json!({"levels": levels, "rfis": rfis, "tags": tags, "mep": {"systems": systems, "runs": runs}});
    d.building_spec = Some(spec.to_string()).into();
    d
}

/// The window with drawing `d` open (or the file at `path`) and dialog `dialog` open: how many
/// times its command ran in `frames` frames at rest (after it has settled), and the median of
/// those frames' dialog part (`dialog`), in ms.
fn dialog_at_rest(d: Drawing, dialog: &str, command: Option<&str>, frames: usize) -> (usize, f64) {
    let mut s = Session::empty();
    s.open_drawing(d, "Sample Job", None);
    dialog_in(CadApp::new(s, Services::default()), dialog, command, frames)
}

fn dialog_in(mut app: CadApp, dialog: &str, command: Option<&str>, frames: usize) -> (usize, f64) {
    let ctx = egui::Context::default();
    frame(&mut app, &ctx);
    app.ui.dialog = Some(dialog.into());
    for _ in 0..5 {
        frame(&mut app, &ctx);
    }
    assert_eq!(app.ui.dialog.as_deref(), Some(dialog), "the dialog stays open");
    let asked = |app: &CadApp| command.map_or(0, |c| app.session.journal.lines().iter().filter(|l| l.split(' ').next() == Some(c)).count());
    let before = asked(&app);
    framecost::reset();
    let mut ms: Vec<f64> = (0..frames)
        .map(|_| {
            frame(&mut app, &ctx);
            framecost::report()["lastMs"]["dialog"].as_f64().unwrap_or(0.0)
        })
        .collect();
    ms.sort_by(f64::total_cmp);
    (asked(&app) - before, ms.get(ms.len() / 2).copied().unwrap_or(0.0))
}

#[test]
fn an_open_list_dialog_asks_once_per_change_of_the_drawing() {
    let _turn = TURN.lock().unwrap_or_else(|e| e.into_inner());
    for (dialog, command) in LIST_DIALOGS {
        let Some(command) = command else { continue };
        let (asks, _) = dialog_at_rest(listed(50, 200), dialog, Some(command), 10);
        assert_eq!(asks, 0, "{dialog}: `{command}` ran {asks} times in 10 frames with the drawing unchanged");
    }
}

#[test]
fn a_list_dialog_costs_what_is_on_screen() {
    let _turn = TURN.lock().unwrap_or_else(|e| e.into_inner());
    let n = std::env::var("FRAMECOST_ITEMS").ok().and_then(|v| v.parse().ok()).unwrap_or(3_000);
    let objects = std::env::var("FRAMECOST_N").ok().and_then(|v| v.parse().ok()).unwrap_or(120_000);
    let big = listed(n, objects);
    let mut failed = Vec::new();
    for (dialog, command) in LIST_DIALOGS {
        let (_, one) = dialog_at_rest(listed(1, 10), dialog, command, 15);
        let (_, many) = dialog_at_rest(big.clone(), dialog, command, 15);
        println!("{dialog} at rest (median dialog part): one item {one:.3} ms, {n} items and {objects} objects {many:.3} ms");
        // A few times the one-item drawing's at most; below a millisecond the clock's noise rules.
        let limit = 4.0 * one.max(1.0);
        if many > limit {
            failed.push(format!("{dialog}: {many:.2} ms on {n} items, {one:.2} ms on one (limit {limit:.2})"));
        }
    }
    assert!(failed.is_empty(), "{failed:?}");
}

/// The four dialogs on a saved drawing (`FRAMECOST_FILE`), for the numbers on a real model; by
/// hand: `FRAMECOST_FILE=<model.cdy> cargo test --release -p cadaddy-ui-egui --test frame_cost
/// list_dialogs_on_a_file -- --ignored --nocapture`.
#[test]
#[ignore]
fn list_dialogs_on_a_file() {
    let _turn = TURN.lock().unwrap_or_else(|e| e.into_inner());
    let Ok(path) = std::env::var("FRAMECOST_FILE") else { return };
    cadaddy_engine::cmd::file::set_io(cadaddy_engine::cmd::file::IoHooks {
        read: |b, name| cadaddy_io::read(b, name).map_err(|e| e.to_string()),
        write: |d, name| cadaddy_io::write(d, name).map_err(|e| e.to_string()),
        plot: None,
        read_pdf: None,
        read_dwf: None,
        write_dwfx: None,
    });
    for (dialog, command) in LIST_DIALOGS {
        let mut s = Session::empty();
        s.execute("open", &json!({ "path": path })).unwrap_or_else(|e| panic!("open {path}: {e}"));
        let objects = s.doc().map_or(0, |d| d.model.len());
        let (asks, ms) = dialog_in(CadApp::new(s, Services::default()), dialog, command, 15);
        println!("{dialog}: {ms:.3} ms a frame (median of 15, dialog part), command ran {asks} times in 15 frames; {objects} objects");
    }
}
