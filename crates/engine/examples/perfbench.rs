//! Performance benchmark over generated drawings, for `cargo xtask perf` (the budgets in
//! `perf/budget.json`).
//!
//! `cargo run -p cadaddy-engine --example perfbench -- --sizes 1000,10000 [--out FILE]`
//!
//! For each size, a drawing of that many objects (lines, polylines with arcs, circles, arcs,
//! texts and 3D faces on ten layers) is made, then timed: saved and opened as DWG, DXF and .cdy,
//! regenerated (the display list), selected all, moved all, the moves undone, zoomed to extents,
//! built in 3D, plotted to PDF and closed. Each step's time (milliseconds, the best of three for
//! sizes up to 10 000) is printed and written as JSON `{"op@size": ms}`, with the process's peak
//! resident memory while that size ran as `peak_mb@size` (sampled after each step with `ps`;
//! sizes run smallest first, so each size's peak is its own; the working set on Windows).
//!
//! Memory (Q250), from 100k objects on, each measured in a fresh process: `--mem-live N` prints
//! the bytes one object of the N-object drawing holds (alone, opened in a session, and with its
//! display list too: `mem_bytes_per_object@N`, `mem_session_bytes_per_object@N`), and
//! `--mem-open FILE` the most memory opening FILE took above its bytes and what the open drawing
//! holds (for the size's DWG: `mem_open_dwg_peak_mb@N`, `mem_open_dwg_live_mb@N`). Process memory
//! is private bytes on Windows, anonymous resident memory on Linux, resident memory elsewhere
//! (sampled for the peak on macOS). `--write FILE [--objects N]` saves the N-object drawing as
//! FILE, for looking at one size by hand.
//!
//! For comparing with other programs (`docs/perf-vs.md`): `--plans DIR` writes the 2D plan of
//! each size as `plan-<n>.dxf` (the same grid without the 3D faces, which not every program
//! reads), and `--file PLAN.dxf` times one file as a user meets it in a fresh process: open
//! (read and display list), select all, zoom extents, save as DXF.

use std::collections::BTreeMap;
use std::sync::atomic::AtomicUsize;
use std::time::Instant;

use cadaddy_engine::Session;
use cadaddy_engine::cmd::file::{IoHooks, set_io};
use cadaddy_engine::doc::color::Rgb;
use cadaddy_engine::doc::geom::{PolyVertex, Vec2, Vec3};
use cadaddy_engine::doc::{Arc, Circle, Common, Drawing, EntityKind, Face3d, Layer, Line, LwPolyline, Space, Text};
use cadaddy_engine::render::raster;
use serde_json::json;

fn hooks() {
    set_io(IoHooks {
        read: |b, name| cadaddy_io::read(b, name).map_err(|e| e.to_string()),
        write: |d, name| cadaddy_io::write(d, name).map_err(|e| e.to_string()),
        plot: Some(|d, space, opts| cadaddy_io::plotter::plot(d, space, opts).map_err(|e| e.to_string())),
        read_pdf: None,
        read_dwf: None,
        write_dwfx: None,
    });
}

/// A drawing of `n` objects laid out on a grid.
fn drawing(n: usize) -> Drawing {
    let mut d = Drawing::new_imperial();
    for i in 0..10 {
        d.layers.push(Layer { name: format!("L{i}"), ..Layer::default() });
    }
    let side = (n as f64).sqrt().ceil().max(1.0) as usize;
    for i in 0..n {
        let (x, y) = ((i % side) as f64 * 12.0, (i / side) as f64 * 12.0);
        let c = Common { layer: format!("L{}", i % 10).into(), ..Common::default() };
        let kind = match i % 6 {
            0 => EntityKind::Line(Line { a: Vec3::new(x, y, 0.0), b: Vec3::new(x + 10.0, y + 5.0, 0.0) }),
            1 => EntityKind::Circle(Circle { center: Vec3::new(x + 5.0, y + 5.0, 0.0), radius: 4.0 }),
            2 => EntityKind::Arc(Arc { center: Vec3::new(x + 5.0, y + 5.0, 0.0), radius: 4.0, start: 0.2, end: 2.5 }),
            3 => EntityKind::LwPolyline(LwPolyline {
                vertices: vec![
                    PolyVertex { p: Vec2::new(x, y), bulge: 0.0, start_width: 0.0, end_width: 0.0 },
                    PolyVertex { p: Vec2::new(x + 8.0, y), bulge: 0.4, start_width: 0.0, end_width: 0.0 },
                    PolyVertex { p: Vec2::new(x + 8.0, y + 8.0), bulge: 0.0, start_width: 0.0, end_width: 0.0 },
                ],
                closed: true,
                const_width: 0.0,
                elevation: 0.0,
                plinegen: false,
            }),
            // Text has many optional fields: their defaults come from its serde form.
            4 => match serde_json::from_value::<Text>(json!({ "insert": { "x": x, "y": y, "z": 0.0 }, "height": 2.0, "value": format!("T{i}") })) {
                Ok(t) => EntityKind::Text(t),
                Err(_) => continue,
            },
            _ => {
                let q = [Vec3::new(x, y, 0.0), Vec3::new(x + 8.0, y, 0.0), Vec3::new(x + 8.0, y + 8.0, 3.0), Vec3::new(x, y + 8.0, 3.0)];
                EntityKind::Face3d(Face3d { corners: q, hidden_edges: 0 })
            }
        };
        let _ = d.add(&Space::Model, c, kind);
    }
    d
}

/// The 2D plan of `n` objects for other programs: the bench drawing without its 3D faces (lines
/// in their place).
fn plan(n: usize) -> Drawing {
    let mut d = drawing(n);
    let faces: Vec<_> = d.model.iter().filter(|e| matches!(e.kind, EntityKind::Face3d(_))).map(|e| e.handle).collect();
    for h in faces {
        let _ = d.modify_entity(h, |e| {
            if let EntityKind::Face3d(f) = &e.kind {
                let [a, _, c, _] = f.corners;
                e.kind = EntityKind::Line(Line { a: Vec3::new(a.x, a.y, 0.0), b: Vec3::new(c.x, c.y, 0.0) });
            }
        });
    }
    d
}

/// Time one plan file as a user meets it, each step once, in this fresh process: printed as JSON
/// `{"open": ms, "select_all": ms, "zoom_extents": ms, "save_dxf": ms, "objects": n}`.
fn one_file(path: &str) {
    let t = Instant::now();
    let mut s = Session::new();
    // PERF_STAGE=tags|read|open: stop after that stage (its peak memory, measured outside).
    let stage = std::env::var("PERF_STAGE").unwrap_or_default();
    if stage == "tags" || stage == "read" {
        let bytes = std::fs::read(path).unwrap_or_default();
        if stage == "tags" {
            println!("{}", cadaddy_dxf::parse(&bytes).map(|t| t.len()).unwrap_or(0));
        } else {
            println!("{}", cadaddy_io::read(&bytes, path).map(|d| d.model.len()).unwrap_or(0));
        }
        return;
    }
    if let Err(e) = s.execute("open", &json!({ "path": path })) {
        eprintln!("{path}: {e}");
        return;
    }
    if stage == "open" {
        return;
    }
    let d = s.doc().cloned().unwrap_or_default();
    drop(cadaddy_engine::render::build(&d, &Space::Model, &Default::default()));
    let open = t.elapsed().as_secs_f64() * 1000.0;
    let select = time(1, || drop(s.execute("selectall", &json!({}))));
    let zoom = time(1, || drop(s.execute("zoom", &json!({ "mode": "extents" }))));
    let save = time(1, || drop(cadaddy_io::write(&d, "out.dxf")));
    let r = json!({ "open": open, "select_all": select, "zoom_extents": zoom, "save_dxf": save, "objects": d.model.len() });
    println!("{r}");
}

/// The process's resident memory now, in MB (`ps`; its working set on Windows; `None` where it
/// can't be read).
fn rss_mb() -> Option<f64> {
    if cfg!(windows) {
        return powershell_process("WorkingSet64");
    }
    let out = std::process::Command::new("ps").args(["-o", "rss=", "-p", &std::process::id().to_string()]).output().ok()?;
    String::from_utf8_lossy(&out.stdout).trim().parse::<f64>().ok().map(|kb| kb / 1024.0)
}

/// This process's memory now, in MB: its private bytes on Windows, anonymous resident memory on
/// Linux, resident memory elsewhere (`None` where it can't be read).
fn private_mb() -> Option<f64> {
    if cfg!(windows) {
        return powershell_process("PrivateMemorySize64");
    }
    if let Some(kb) = proc_status_kb("RssAnon:") {
        return Some(kb / 1024.0);
    }
    rss_mb()
}

/// The most memory this process has held, in MB: its peak commit (private bytes) on Windows, its
/// peak resident memory on Linux; `None` elsewhere (macOS has no peak to ask for: the caller
/// samples).
fn peak_private_mb() -> Option<f64> {
    if cfg!(windows) {
        return powershell_process("PeakPagedMemorySize64");
    }
    proc_status_kb("VmHWM:").map(|kb| kb / 1024.0)
}

fn powershell_process(field: &str) -> Option<f64> {
    let script = format!("(Get-Process -Id {}).{field}", std::process::id());
    let out = std::process::Command::new("powershell").args(["-NoProfile", "-NonInteractive", "-Command", &script]).output().ok()?;
    String::from_utf8_lossy(&out.stdout).trim().parse::<f64>().ok().map(|b| b / 1_048_576.0)
}

fn proc_status_kb(key: &str) -> Option<f64> {
    let status = std::fs::read_to_string("/proc/self/status").ok()?;
    status.lines().find_map(|l| l.strip_prefix(key))?.trim().trim_end_matches("kB").trim().parse::<f64>().ok()
}

/// `f` run while a thread samples the resident memory every 20 ms where the system keeps no peak
/// (macOS): its result and the most memory seen (the system's peak where it keeps one), in MB.
fn with_peak<R>(f: impl FnOnce() -> R) -> (R, f64) {
    if peak_private_mb().is_some() {
        let r = f();
        return (r, peak_private_mb().unwrap_or(0.0));
    }
    let done = std::sync::atomic::AtomicBool::new(false);
    std::thread::scope(|sc| {
        let sampler = sc.spawn(|| {
            let mut most = 0.0f64;
            while !done.load(std::sync::atomic::Ordering::Relaxed) {
                most = most.max(rss_mb().unwrap_or(0.0));
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
            most
        });
        let r = f();
        done.store(true, std::sync::atomic::Ordering::Relaxed);
        let most = sampler.join().unwrap_or(0.0).max(rss_mb().unwrap_or(0.0));
        (r, most)
    })
}

/// `--mem-live N`, in a fresh process: the memory one object of the bench drawing holds, in bytes
/// (the drawing alone, opened in a session, then with its display list too), printed as JSON
/// `{"bytes_per_object": b, "opened_bytes_per_object": b, "session_bytes_per_object": b}`.
fn mem_live(n: usize) {
    let n = n.max(1);
    let before = private_mb().unwrap_or(0.0);
    let d = drawing(n);
    let doc = private_mb().unwrap_or(0.0);
    let mut s = Session::new();
    s.open_drawing(d, "Bench", None);
    let opened = private_mb().unwrap_or(0.0);
    let list = s.doc().map(|d| cadaddy_engine::render::build(d, &Space::Model, &Default::default()));
    let session = private_mb().unwrap_or(0.0);
    let per = |mb: f64| (mb * 1_048_576.0 / n as f64).round();
    println!(
        "{}",
        json!({ "bytes_per_object": per(doc - before), "opened_bytes_per_object": per(opened - before), "session_bytes_per_object": per(session - before) })
    );
    drop(list);
}

/// `--mem-open FILE`, in a fresh process: the file opened (read into a drawing), printed as JSON
/// `{"peak_mb": the most memory the open took above the file's bytes, "live_mb": what the drawing
/// holds once open}`.
fn mem_open(path: &str) {
    let bytes = std::fs::read(path).unwrap_or_default();
    let before = private_mb().unwrap_or(0.0);
    // The peak so far (start-up, the file read) is not the open's: Windows and Linux keep the
    // process's peak, so the open's is only seen when it goes above the start.
    let start_peak = peak_private_mb().unwrap_or(0.0).max(before);
    let (d, peak) = with_peak(|| cadaddy_io::read(&bytes, path));
    let live = private_mb().unwrap_or(0.0);
    let objects = d.as_ref().map_or(0, |d| d.model.len());
    let peak = if peak > start_peak { peak } else { live };
    let r = |mb: f64| (mb.max(0.0) * 10.0).round() / 10.0;
    println!("{}", json!({ "peak_mb": r(peak - before), "live_mb": r(live - before), "objects": objects }));
    drop(d);
}

/// Measure pan frame time: the display list is built once, then rasterised at shifted views.
/// This measures the CPU cost of drawing one frame after a pan (the GPU path is faster: it only
/// changes a view-matrix uniform).  Printed as JSON `{"pan_frame_ms": avg, "pan_fps": fps,
/// "objects": n, "steps": steps}`.
fn pan_file(path: &str) {
    let bytes = std::fs::read(path).unwrap_or_default();
    let d = match cadaddy_io::read(&bytes, path) {
        Ok(d) => d,
        Err(e) => {
            eprintln!("{path}: {e}");
            return;
        }
    };
    let list = cadaddy_engine::render::build(&d, &Space::Model, &Default::default());
    let bounds = list.bounds;
    if bounds.is_empty() {
        println!("{}", json!({"pan_frame_ms": 0, "pan_fps": 0, "objects": 0, "steps": 0}));
        return;
    }
    let objects = d.model.len();
    let steps: usize = 100;
    let w: u32 = 1600;
    let h: u32 = 900;
    let opts = raster::RasterOptions::default();
    // Start fitted to the drawing, pan rightward by 1% of the drawing width each step.
    let start_view = raster::View::fit(&bounds, w, h, 0.02);
    let dx = bounds.width() * 0.01;
    let mut total_ms = 0.0;
    for i in 0..steps {
        let view = raster::View { center: Vec2::new(start_view.center.x + dx * i as f64, start_view.center.y), ..start_view };
        let t = Instant::now();
        drop(raster::render(&list, &view, &opts));
        total_ms += t.elapsed().as_secs_f64() * 1000.0;
    }
    let avg = total_ms / steps as f64;
    let fps = if avg > 0.0 { 1000.0 / avg } else { 0.0 };
    let r = json!({"pan_frame_ms": (avg * 100.0).round() / 100.0, "pan_fps": (fps * 10.0).round() / 10.0, "objects": objects, "steps": steps});
    println!("{r}");
}

/// The smallest drawing whose memory is measured (`mem_…` results).
const MEMORY_FROM: usize = 100_000;

/// This program again (a fresh process) with `args`: its JSON answer.
fn child(args: &[&str]) -> Option<serde_json::Value> {
    let exe = std::env::current_exe().ok()?;
    let out = std::process::Command::new(exe).args(args).output().ok()?;
    let text = String::from_utf8_lossy(&out.stdout);
    text.lines().rev().find_map(|l| serde_json::from_str(l.trim()).ok())
}

/// The best time of `runs` runs of `f`, in milliseconds.
fn time(runs: usize, mut f: impl FnMut()) -> f64 {
    (0..runs.max(1))
        .map(|_| {
            let t = Instant::now();
            f();
            t.elapsed().as_secs_f64() * 1000.0
        })
        .fold(f64::INFINITY, f64::min)
}

fn main() {
    hooks();
    let args: Vec<String> = std::env::args().skip(1).collect();
    let arg = |k: &str| args.iter().position(|a| a == k).and_then(|i| args.get(i + 1)).cloned();
    let sizes: Vec<usize> = arg("--sizes").unwrap_or_else(|| "1000,10000".into()).split(',').filter_map(|s| s.trim().parse().ok()).collect();
    let mut out: BTreeMap<String, f64> = BTreeMap::new();
    if let Some(dir) = arg("--plans") {
        let _ = std::fs::create_dir_all(&dir);
        for &n in &sizes {
            let path = std::path::Path::new(&dir).join(format!("plan-{n}.dxf"));
            let name = path.to_string_lossy().into_owned();
            match cadaddy_io::write(&plan(n), &name) {
                Ok(bytes) => {
                    let _ = std::fs::write(&path, bytes);
                    println!("wrote {name}");
                }
                Err(e) => eprintln!("{name}: {e}"),
            }
        }
        return;
    }
    if let Some(n) = arg("--mem-live").and_then(|n| n.parse().ok()) {
        mem_live(n);
        return;
    }
    // `--write FILE [--objects N]`: the bench drawing of N objects (1000) saved as FILE, its
    // format by the name.
    if let Some(path) = arg("--write") {
        let n = arg("--objects").and_then(|n| n.parse().ok()).unwrap_or(1000);
        match cadaddy_io::write(&drawing(n), &path).map(|b| std::fs::write(&path, b)) {
            Ok(Ok(())) => println!("wrote {path}"),
            Ok(Err(e)) => eprintln!("{path}: {e}"),
            Err(e) => eprintln!("{path}: {e}"),
        }
        return;
    }
    if let Some(path) = arg("--mem-open") {
        mem_open(&path);
        return;
    }
    if let Some(path) = arg("--pan") {
        pan_file(&path);
        return;
    }
    if let Some(path) = arg("--file") {
        one_file(&path);
        return;
    }
    for n in sizes {
        let runs = if n <= 10_000 { 3 } else { 1 };
        let d = drawing(n);
        let mut peak = rss_mb().unwrap_or(0.0);
        let mut rec = |op: &str, ms: f64| {
            let now = rss_mb().unwrap_or(0.0);
            println!("{op:>14} @ {n:>7}: {ms:10.1} ms   {now:8.0} MB resident after");
            out.insert(format!("{op}@{n}"), (ms * 10.0).round() / 10.0);
            peak = peak.max(now);
        };
        for ext in ["dwg", "dxf", "cdy"] {
            let name = format!("bench.{ext}");
            let mut bytes = Vec::new();
            rec(&format!("save_{ext}"), time(runs, || bytes = cadaddy_io::write(&d, &name).unwrap_or_default()));
            rec(&format!("open_{ext}"), time(runs, || drop(cadaddy_io::read(&bytes, &name))));
        }
        rec("regen", time(runs, || drop(cadaddy_engine::render::build(&d, &Space::Model, &Default::default()))));
        let mut s = Session::new();
        s.open_drawing(d.clone(), "Bench", None);
        rec("select_all", time(runs, || drop(s.execute("selectall", &json!({})))));
        rec(
            "move_all",
            time(runs, || {
                let _ = s.execute("move", &json!({ "delta": [1, 1] }));
            }),
        );
        // The moves taken back, one undo each (the drawing as it was opened).
        rec(
            "undo",
            time(1, || {
                for _ in 0..runs {
                    let _ = s.execute("undo", &json!({}));
                }
            }) / runs as f64,
        );
        rec("zoom_extents", time(runs, || drop(s.execute("zoom", &json!({ "mode": "extents" })))));
        rec(
            "build_3d",
            time(runs, || {
                let (p, t) = (AtomicUsize::new(0), AtomicUsize::new(0));
                drop(cadaddy_engine::render::model3::build(&d, &Space::Model, 0.01, Rgb(255, 255, 255), &p, &t));
            }),
        );
        rec("plot_pdf", time(runs, || drop(cadaddy_io::plotter::plot(&d, &Space::Model, &json!({})))));
        // Closed with its changes discarded (saved under no name; the session drops it).
        rec(
            "close",
            time(1, || {
                if let Ok(st) = s.state_mut() {
                    st.saved = st.doc.clone();
                }
                let _ = s.execute("close", &json!({}));
            }),
        );
        drop(s);
        // Memory, each in a fresh process, from 100k objects on (smaller drawings are within the
        // process memory's noise): what one object holds, and opening the DWG.
        let measure = n >= MEMORY_FROM;
        let dwg = std::env::temp_dir().join(format!("cadaddy-perfbench-{}-{n}.dwg", std::process::id()));
        let wrote = measure && cadaddy_io::write(&d, "bench.dwg").ok().is_some_and(|b| std::fs::write(&dwg, b).is_ok());
        drop(d);
        let mb = (peak * 10.0).round() / 10.0;
        println!("{:>14} @ {n:>7}: {mb:10.1} MB", "peak memory");
        out.insert(format!("peak_mb@{n}"), mb);
        if let Some(v) = child(&["--mem-live", &n.to_string()]).filter(|_| measure) {
            for (key, field) in [("mem_bytes_per_object", "bytes_per_object"), ("mem_session_bytes_per_object", "session_bytes_per_object")] {
                if let Some(b) = v.get(field).and_then(serde_json::Value::as_f64) {
                    println!("{key:>30} @ {n:>7}: {b:10.0} bytes");
                    out.insert(format!("{key}@{n}"), b);
                }
            }
        }
        if wrote && let Some(v) = child(&["--mem-open", &dwg.to_string_lossy()]) {
            for (key, field) in [("mem_open_dwg_peak_mb", "peak_mb"), ("mem_open_dwg_live_mb", "live_mb")] {
                if let Some(mb) = v.get(field).and_then(serde_json::Value::as_f64) {
                    println!("{key:>30} @ {n:>7}: {mb:10.1} MB");
                    out.insert(format!("{key}@{n}"), mb);
                }
            }
        }
        let _ = std::fs::remove_file(&dwg);
    }
    if let Some(path) = arg("--out") {
        let _ = std::fs::write(&path, serde_json::to_string_pretty(&out).unwrap_or_default());
    }
}
