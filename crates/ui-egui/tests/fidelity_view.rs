//! Fidelity levels in the 3D view: far out the view holds only the building's elements (a
//! room's outlets are not built, not uploaded); zoomed into a room the room's outlets come in
//! as a layer of their own (instanced, only the ones near the view); the level can be chosen by
//! hand (Building, Floor, Room, Detail) or follow the zoom (Auto); picks follow the level shown;
//! on the CPU and on the GPU.

use cadaddy_doc::{Common, Drawing, EntityKind, Mesh, MeshFace, MeshKind, RawTag, Space};
use cadaddy_engine::Session;
use cadaddy_geom::Vec3;
use cadaddy_ui_egui::{CadApp, Services};
use cadaddy_walk3d::fidelity::Fidelity;
use egui::vec2;
use egui_kittest::Harness;
use serde_json::json;

fn boxmesh(lo: Vec3, hi: Vec3) -> Mesh {
    let v = |x: bool, y: bool, z: bool| Vec3::new(if x { hi.x } else { lo.x }, if y { hi.y } else { lo.y }, if z { hi.z } else { lo.z });
    let vertices = vec![
        v(false, false, false),
        v(true, false, false),
        v(true, true, false),
        v(false, true, false),
        v(false, false, true),
        v(true, false, true),
        v(true, true, true),
        v(false, true, true),
    ];
    let f = |a: [u32; 4]| MeshFace { v: a.to_vec(), hidden: 0 };
    Mesh {
        vertices,
        faces: vec![f([0, 3, 2, 1]), f([4, 5, 6, 7]), f([0, 1, 5, 4]), f([1, 2, 6, 5]), f([2, 3, 7, 6]), f([3, 0, 4, 7])],
        kind: MeshKind::Subd,
    }
}

/// What a build made: `[APP, part, level, source, more…]`.
fn made(part: &str, more: &[&str]) -> Vec<RawTag> {
    let mut x = vec![
        RawTag { code: 1001, value: "CADADDY_WALLGEN".into() },
        RawTag { code: 1000, value: part.into() },
        RawTag { code: 1000, value: "Level 1".into() },
        RawTag { code: 1000, value: "0".into() },
    ];
    x.extend(more.iter().map(|m| RawTag { code: 1000, value: (*m).into() }));
    x
}

/// Inches a foot.
const FT: f64 = 12.0;
/// Rooms in the row, outlets in each.
const ROOMS: usize = 20;
const OUTLETS: usize = 4;

/// A made-up one-storey building (inches): a 400' × 20' slab, a row of 20' rooms with walls
/// between them, a 24" supply main down the corridor side, and four outlets in each room.
fn drawing() -> Drawing {
    let mut d = Drawing { building_spec: Some(json!({"units_per_foot": FT}).to_string()), ..Drawing::default() };
    let mut add = |layer: &str, x: Vec<RawTag>, m: Mesh| {
        d.add(&Space::Model, Common { layer: layer.into(), xdata: x.into(), ..Common::default() }, EntityKind::Mesh(m)).unwrap();
    };
    let w = ROOMS as f64 * 20.0 * FT;
    add("S-SLAB", made("floor", &[]), boxmesh(Vec3::new(0.0, 0.0, -6.0), Vec3::new(w, 20.0 * FT, 0.0)));
    let duct = ["system:SA-1", "kind:duct", "part:segment", "duct_type:supply", "size:24x12", "mepid:main/segment"];
    add("M-DUCT", made("mep", &duct), boxmesh(Vec3::new(0.0, 16.0 * FT, 96.0), Vec3::new(w, 18.0 * FT, 108.0)));
    for r in 0..ROOMS {
        let x0 = r as f64 * 20.0 * FT;
        add("A-ROOM", made("room", &[]), boxmesh(Vec3::new(x0 + 3.0, 0.0, 0.0), Vec3::new(x0 + 20.0 * FT - 3.0, 20.0 * FT, 108.0)));
        add("A-WALL", made("wall", &[]), boxmesh(Vec3::new(x0 - 3.0, 0.0, 0.0), Vec3::new(x0 + 3.0, 20.0 * FT, 108.0)));
        for k in 0..OUTLETS {
            let x = x0 + 24.0 + k as f64 * 48.0;
            add("E-DEVICE", made("device", &["type:Duplex"]), boxmesh(Vec3::new(x, 1.0, 18.0), Vec3::new(x + 4.0, 3.0, 22.0)));
        }
    }
    d
}

fn app(gpu: bool) -> (CadApp, Option<egui_wgpu::RenderState>) {
    let mut s = Session::empty();
    s.open_drawing(drawing(), "Sample Building", None);
    let mut app = CadApp::new(s, Services::default());
    let rs = gpu.then(|| egui_kittest::wgpu::create_render_state(egui_kittest::wgpu::default_wgpu_setup(), egui_wgpu::RendererOptions::default()));
    if let Some(rs) = &rs {
        app.set_wgpu(rs, None);
    }
    app.session.execute("vscurrent", &json!({"style": "shadededges"})).unwrap();
    (app, rs)
}

fn harness(app: CadApp, rs: Option<egui_wgpu::RenderState>) -> Harness<'static, CadApp> {
    let b = Harness::builder().with_size(vec2(1200.0, 800.0));
    let b = match rs {
        Some(rs) => b.renderer(egui_kittest::wgpu::WgpuTestRenderer::from_render_state(rs)),
        None => b,
    };
    b.build_ui_state(
        |ui, app: &mut CadApp| {
            app.logic(ui.ctx());
            app.ui(ui);
        },
        app,
    )
}

/// The view `height_ft` tall round (x, y) feet, then frames until `done` (at most ~8 s).
fn look(h: &mut Harness<'static, CadApp>, at: [f64; 2], height_ft: f64, done: impl Fn(&CadApp) -> bool) {
    h.state_mut().session.execute("zoom", &json!({"mode": "center", "center": [at[0] * FT, at[1] * FT], "height": height_ft * FT})).unwrap();
    for _ in 0..800 {
        h.run_steps(1);
        if h.state().canvas.shade.stats.building.is_none() && done(h.state()) {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    h.run_steps(2);
}

fn levels_follow_the_zoom(gpu: bool) {
    let (app, rs) = app(gpu);
    let mut h = harness(app, rs);
    // The whole building: the slab and the main, nothing of the rooms.
    look(&mut h, [200.0, 10.0], 420.0, |a| {
        let s = a.canvas.shade.stats;
        s.tris_total > 0 && s.fidelity == "building"
    });
    let far = h.state().canvas.shade.stats;
    assert_eq!(far.fidelity, "building", "{far:?}");
    assert!(!far.fidelity_fixed);
    assert_eq!(far.tris_total, 24, "the slab and the main only: {far:?}");
    assert_eq!((far.instances, far.fine_elements), (0, 0));
    assert_eq!(h.state().session.fidelity_shown, Some(Fidelity::Building));
    // Into one room: walls and rooms over the whole building, the outlets near the view only,
    // the identical ones instanced.
    look(&mut h, [50.0, 10.0], 16.0, |a| {
        let s = a.canvas.shade.stats;
        s.fidelity == "room" && s.fine_elements > 0
    });
    let near = h.state().canvas.shade.stats;
    assert_eq!(near.fidelity, "room", "{near:?}");
    assert!(near.tris_total > far.tris_total, "walls and rooms: {near:?}");
    assert!(near.fine_elements >= OUTLETS && near.fine_elements < ROOMS * OUTLETS, "only the outlets near the view: {near:?}");
    assert!(near.fine_bytes > 0);
    assert_eq!(near.tris_total, far.tris_total + 12 * 2 * ROOMS, "no outlet in the whole-building layer: {near:?}");
    if gpu {
        assert!(near.instances_drawn >= OUTLETS, "the outlets drawn as instances: {near:?}");
    }
    assert_eq!(h.state().session.fidelity_shown, Some(Fidelity::Room));
    // Chosen by hand: the building while zoomed in; the fine layer let go.
    h.state_mut().canvas.shade.fidelity.fixed = Some(Fidelity::Building);
    look(&mut h, [50.0, 10.0], 16.0, |a| {
        let s = a.canvas.shade.stats;
        s.fidelity == "building" && s.tris_total == 24
    });
    let fixed = h.state().canvas.shade.stats;
    assert_eq!((fixed.fidelity, fixed.fidelity_fixed, fixed.fine_elements), ("building", true, 0), "{fixed:?}");
    assert_eq!(h.state().session.fidelity_shown, Some(Fidelity::Building));
    // Auto again, then out to the whole building.
    h.state_mut().canvas.shade.fidelity.fixed = None;
    look(&mut h, [50.0, 10.0], 16.0, |a| {
        let s = a.canvas.shade.stats;
        s.fidelity == "room" && s.fine_elements > 0
    });
    let back = h.state().canvas.shade.stats;
    assert_eq!(back.fidelity, "room");
    look(&mut h, [200.0, 10.0], 420.0, |a| {
        let s = a.canvas.shade.stats;
        s.fidelity == "building" && s.tris_total == 24
    });
    let out = h.state().canvas.shade.stats;
    assert_eq!((out.fidelity, out.fine_elements, out.fine_bytes), ("building", 0, 0), "{out:?}");
    // Out of the 3D view: picks are the drawing's again.
    h.state_mut().session.execute("vscurrent", &json!({"style": "2dwireframe"})).unwrap();
    h.run_steps(3);
    assert_eq!(h.state().session.fidelity_shown, None);
}

#[test]
fn levels_follow_the_zoom_on_the_cpu() {
    levels_follow_the_zoom(false);
}

#[test]
fn levels_follow_the_zoom_on_the_gpu() {
    levels_follow_the_zoom(true);
}

/// A drawing with nothing built shows as it is: no levels, picks the drawing's.
#[test]
fn a_plain_drawing_has_no_levels() {
    let mut s = Session::empty();
    let mut d = Drawing::default();
    for k in 0..6 {
        let x = f64::from(k) * 10.0;
        d.add(&Space::Model, Common::default(), EntityKind::Mesh(boxmesh(Vec3::new(x, 0.0, 0.0), Vec3::new(x + 1.0, 1.0, 1.0)))).unwrap();
    }
    s.open_drawing(d, "Plain", None);
    let mut app = CadApp::new(s, Services::default());
    app.session.execute("vscurrent", &json!({"style": "shadededges"})).unwrap();
    let mut h = harness(app, None);
    look(&mut h, [2.5, 0.0], 6.0, |a| {
        let s = a.canvas.shade.stats;
        s.tris_total > 0
    });
    let st = h.state().canvas.shade.stats;
    assert_eq!(st.fidelity, "");
    assert_eq!(st.tris_total, 72, "everything drawn: {st:?}");
    assert_eq!(st.instances, 6, "six identical boxes: one prototype, six placements");
    assert_eq!(h.state().session.fidelity_shown, None);
}

/// What each level costs on a real model (its path in `CADADDY_FIDELITY_BENCH`, a file kept
/// outside the repo), drawn as the app draws it: time to the first frame after opening, the
/// model's and the GPU's bytes and the orbit benchmark's frames a second at each level, and how
/// long a zoom into the room holding the most devices takes to show them. Run with
/// `--ignored --nocapture`; prints one JSON line.
#[test]
#[ignore = "a benchmark of a model from outside the repo"]
fn bench_a_model_at_each_level() {
    let Ok(path) = std::env::var("CADADDY_FIDELITY_BENCH") else { return };
    let gpu = std::env::var("CADADDY_FIDELITY_BENCH_CPU").is_err();
    let started = std::time::Instant::now();
    let bytes = std::fs::read(&path).unwrap();
    let d = cadaddy_io::read(&bytes, &path).unwrap();
    let read_ms = started.elapsed().as_secs_f64() * 1000.0;
    // The room holding the most room-level elements, in the view's (stacked) plan.
    let tiers = cadaddy_engine::cmd::fidelity::Tiered::of(&d).expect("a built model");
    let room = tiers
        .display
        .model
        .iter()
        .filter(|e| cadaddy_engine::cmd::wallgen::made_as(e).is_some_and(|p| p.eq_ignore_ascii_case("room")))
        .map(|e| cadaddy_doc::entity_bounds(&tiers.display, e, 0))
        .filter(|b| !b.is_empty())
        .max_by_key(|b| tiers.count_in(b)[Fidelity::Room.index() as usize])
        .unwrap();
    let upf = tiers.upf;
    drop(tiers);
    let mut s = Session::empty();
    s.open_drawing(d, "Bench", None);
    let mut app = CadApp::new(s, Services::default());
    let rs = gpu.then(|| egui_kittest::wgpu::create_render_state(egui_kittest::wgpu::default_wgpu_setup(), egui_wgpu::RendererOptions::default()));
    if let Some(rs) = &rs {
        app.set_wgpu(rs, None);
    }
    app.session.execute("vscurrent", &json!({"style": "shadededges"})).unwrap();
    app.session.execute("view.dir", &json!({"dir": [-1, -1.5, 1], "fit": true})).unwrap();
    let mut h = harness(app, rs);
    let opened = std::time::Instant::now();
    let wait = |h: &mut Harness<'static, CadApp>, done: &dyn Fn(&CadApp) -> bool| {
        for _ in 0..30_000 {
            h.run_steps(1);
            if h.state().canvas.shade.stats.building.is_none() && done(h.state()) {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
    };
    let fps = |h: &mut Harness<'static, CadApp>| {
        h.state_mut().run("ui.bench3d", json!({"frames": 10})).unwrap();
        for _ in 0..20_000 {
            h.run_steps(1);
            if h.state().canvas.shade.stats.bench_fps.is_some() {
                break;
            }
        }
        h.state().canvas.shade.stats.bench_fps
    };
    let mut out = serde_json::Map::new();
    out.insert("read_ms".into(), json!(read_ms.round()));
    wait(&mut h, &|a| a.canvas.shade.stats.tris_total > 0);
    out.insert("first_frame_ms".into(), json!(opened.elapsed().as_secs_f64() * 1000.0));
    out.insert("opened_at".into(), json!(h.state().canvas.shade.stats.fidelity));
    // The whole building at each level chosen by hand.
    for level in Fidelity::ALL {
        h.state_mut().canvas.shade.fidelity.fixed = Some(level);
        let t0 = std::time::Instant::now();
        wait(&mut h, &|a| a.canvas.shade.stats.fidelity == level.name() && (level < Fidelity::Room || a.canvas.shade.stats.fine_ms > 0.0));
        let ready_ms = t0.elapsed().as_secs_f64() * 1000.0;
        let f = fps(&mut h);
        let st = h.state().canvas.shade.stats;
        out.insert(
            format!("whole_{}", level.name()),
            json!({"ready_ms": ready_ms, "fps": f, "tris_total": st.tris_total, "tris_drawn": st.tris_drawn, "model_mb": st.model_bytes as f64 / 1e6,
                   "gpu_mb": st.gpu_bytes as f64 / 1e6, "fine_elements": st.fine_elements, "fine_capped": st.fine_capped, "fine_mb": st.fine_bytes as f64 / 1e6, "fine_ms": st.fine_ms}),
        );
    }
    // Auto: from the whole building into the busiest room (from above).
    h.state_mut().canvas.shade.fidelity.fixed = None;
    h.state_mut().session.execute("view.top", &json!({})).unwrap();
    h.state_mut().session.execute("zoom", &json!({"mode": "extents"})).unwrap();
    wait(&mut h, &|a| a.canvas.shade.stats.fidelity == "building");
    let c = room.center();
    let height = room.height().max(room.width() * 0.6) * 1.3;
    let t0 = std::time::Instant::now();
    h.state_mut().session.execute("zoom", &json!({"mode": "center", "center": [c.x, c.y], "height": height})).unwrap();
    wait(&mut h, &|a| a.canvas.shade.stats.fine_elements > 0 && a.canvas.shade.stats.fidelity == "room");
    let zoom_ms = t0.elapsed().as_secs_f64() * 1000.0;
    let st = h.state().canvas.shade.stats;
    out.insert(
        "zoom_into_room".into(),
        json!({"room_ft": [room.width() / upf, room.height() / upf], "shown_ms": zoom_ms, "level": st.fidelity, "fine_elements": st.fine_elements, "fine_ms": st.fine_ms,
               "tris_drawn": st.tris_drawn, "gpu_mb": st.gpu_bytes as f64 / 1e6, "fps": fps(&mut h)}),
    );
    out.insert("gpu".into(), json!(gpu));
    println!("FIDELITY_BENCH {}", serde_json::Value::Object(out));
}
