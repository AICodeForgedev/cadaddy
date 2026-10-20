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
//! sizes run smallest first, so each size's peak is its own).

use std::collections::BTreeMap;
use std::sync::atomic::AtomicUsize;
use std::time::Instant;

use cadaddy_engine::Session;
use cadaddy_engine::cmd::file::{IoHooks, set_io};
use cadaddy_engine::doc::color::Rgb;
use cadaddy_engine::doc::geom::{PolyVertex, Vec2, Vec3};
use cadaddy_engine::doc::{Arc, Circle, Common, Drawing, EntityKind, Face3d, Layer, Line, LwPolyline, Space, Text};
use serde_json::json;

fn hooks() {
    set_io(IoHooks {
        read: |b, name| cadaddy_io::read(b, name).map_err(|e| e.to_string()),
        write: |d, name| cadaddy_io::write(d, name).map_err(|e| e.to_string()),
        plot: Some(|d, space, opts| cadaddy_io::plotter::plot(d, space, opts).map_err(|e| e.to_string())),
        read_pdf: None,
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
        let c = Common { layer: format!("L{}", i % 10), ..Common::default() };
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

/// The process's resident memory now, in MB (`ps`; `None` where it can't be read).
fn rss_mb() -> Option<f64> {
    let out = std::process::Command::new("ps").args(["-o", "rss=", "-p", &std::process::id().to_string()]).output().ok()?;
    String::from_utf8_lossy(&out.stdout).trim().parse::<f64>().ok().map(|kb| kb / 1024.0)
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
        drop(d);
        let mb = (peak * 10.0).round() / 10.0;
        println!("{:>14} @ {n:>7}: {mb:10.1} MB", "peak memory");
        out.insert(format!("peak_mb@{n}"), mb);
    }
    if let Some(path) = arg("--out") {
        let _ = std::fs::write(&path, serde_json::to_string_pretty(&out).unwrap_or_default());
    }
}
