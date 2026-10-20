//! A large drawing within budgets (Q182): open, regenerate, zoom, select, move, undo and save a
//! generated drawing of 200,000 objects (CI), or of `MILLION_N` objects run by hand:
//!
//! ```text
//! MILLION_N=1000000 cargo test --release -p cadaddy-ui-egui --test million -- --nocapture
//! ```
//!
//! Each step is timed and checked against its budget; the report prints the times and the
//! process's memory. The budgets are for CI machines (debug-optimized builds, shared cores): a
//! release build on a desktop runs well inside them.

use std::fmt::Write as _;
use std::time::Instant;

use cadaddy_engine::Session;
use cadaddy_engine::cmd::file::{IoHooks, set_io};
use cadaddy_engine::doc::Space;
use serde_json::json;

/// Objects per row of the grid; each object has a 10 × 10 cell.
fn columns(n: usize) -> usize {
    (n as f64).sqrt().ceil() as usize
}

/// A DXF file of `n` objects: lines, circles, arcs, polylines, text and block references on 20
/// layers, laid out on a grid.
fn generate(n: usize) -> String {
    let mut s = String::with_capacity(n * 160);
    let mut g = |code: i32, v: &str| {
        let _ = write!(s, "{code}\n{v}\n");
    };
    g(0, "SECTION");
    g(2, "TABLES");
    g(0, "TABLE");
    g(2, "LAYER");
    for l in 0..20 {
        g(0, "LAYER");
        g(2, &format!("L{l}"));
        g(70, "0");
        g(62, &((l % 7) + 1).to_string());
        g(6, "CONTINUOUS");
    }
    g(0, "ENDTAB");
    g(0, "ENDSEC");
    g(0, "SECTION");
    g(2, "BLOCKS");
    g(0, "BLOCK");
    g(2, "TAG");
    g(70, "0");
    g(10, "0");
    g(20, "0");
    g(30, "0");
    g(0, "CIRCLE");
    g(8, "0");
    g(10, "0");
    g(20, "0");
    g(30, "0");
    g(40, "2");
    for (a, b) in [((-2, 0), (2, 0)), ((0, -2), (0, 2))] {
        g(0, "LINE");
        g(8, "0");
        g(10, &a.0.to_string());
        g(20, &a.1.to_string());
        g(30, "0");
        g(11, &b.0.to_string());
        g(21, &b.1.to_string());
        g(31, "0");
    }
    g(0, "ENDBLK");
    g(0, "ENDSEC");
    g(0, "SECTION");
    g(2, "ENTITIES");
    let cols = columns(n);
    for i in 0..n {
        let (x, y) = (((i % cols) * 10) as f64, ((i / cols) * 10) as f64);
        let layer = format!("L{}", i % 20);
        let (xs, ys) = (|v: f64| format!("{v}"), |v: f64| format!("{v}"));
        match i % 20 {
            0..=9 => {
                g(0, "LINE");
                g(8, &layer);
                g(10, &xs(x + 1.0));
                g(20, &ys(y + 1.0));
                g(30, "0");
                g(11, &xs(x + 9.0));
                g(21, &ys(y + 1.0 + (i % 8) as f64));
                g(31, "0");
            }
            10..=12 => {
                g(0, "CIRCLE");
                g(8, &layer);
                g(10, &xs(x + 5.0));
                g(20, &ys(y + 5.0));
                g(30, "0");
                g(40, &format!("{}", 1.0 + (i % 4) as f64));
            }
            13..=14 => {
                g(0, "ARC");
                g(8, &layer);
                g(10, &xs(x + 5.0));
                g(20, &ys(y + 5.0));
                g(30, "0");
                g(40, "3.5");
                g(50, &format!("{}", (i % 360) as f64));
                g(51, &format!("{}", ((i + 120) % 360) as f64));
            }
            15..=17 => {
                g(0, "LWPOLYLINE");
                g(8, &layer);
                g(90, "4");
                g(70, "1");
                for (dx, dy) in [(1.0, 1.0), (9.0, 1.0), (9.0, 6.0), (1.0, 6.0)] {
                    g(10, &xs(x + dx));
                    g(20, &ys(y + dy));
                }
            }
            18 => {
                g(0, "TEXT");
                g(8, &layer);
                g(10, &xs(x + 1.0));
                g(20, &ys(y + 4.0));
                g(30, "0");
                g(40, "2");
                g(1, &format!("R{i}"));
            }
            _ => {
                g(0, "INSERT");
                g(8, &layer);
                g(2, "TAG");
                g(10, &xs(x + 5.0));
                g(20, &ys(y + 5.0));
                g(30, "0");
            }
        }
    }
    g(0, "ENDSEC");
    g(0, "EOF");
    s
}

/// The process's resident memory now, in MB (`ps`; `None` where it can't be read).
fn rss_mb() -> Option<f64> {
    let out = std::process::Command::new("ps").args(["-o", "rss=", "-p", &std::process::id().to_string()]).output().ok()?;
    String::from_utf8_lossy(&out.stdout).trim().parse::<f64>().ok().map(|kb| kb / 1024.0)
}

struct Report {
    n: usize,
    lines: Vec<(String, f64, f64, f64)>,
    peak_mb: f64,
}

impl Report {
    fn step<T>(&mut self, name: &str, budget_ms: f64, f: impl FnOnce() -> T) -> T {
        let t = Instant::now();
        let r = f();
        let ms = t.elapsed().as_secs_f64() * 1000.0;
        let mb = rss_mb().unwrap_or(0.0);
        self.peak_mb = self.peak_mb.max(mb);
        self.lines.push((name.to_string(), ms, budget_ms, mb));
        r
    }
    fn print(&self) {
        println!("\n{} objects:", self.n);
        for (name, ms, budget, mb) in &self.lines {
            println!("  {name:<28} {ms:>10.0} ms   (budget {budget:>6.0} ms)   {mb:>6.0} MB after");
        }
        println!("  {:<28} {:>10.0} MB", "peak memory", self.peak_mb);
    }
    fn over(&self) -> Vec<String> {
        self.lines.iter().filter(|(_, ms, b, _)| ms > b).map(|(n, ms, b, _)| format!("{n}: {ms:.0} ms > {b:.0} ms")).collect()
    }
}

/// Display-list options as the canvas uses them for a view `height` units tall.
fn view_opts(height: f64, view: Option<cadaddy_geom::Bounds2>) -> cadaddy_render::Options {
    let px = height / 1000.0;
    cadaddy_render::Options { tolerance: px * 0.5, min_dash: px * 2.0, view_height: height, view, ..cadaddy_render::Options::default() }
}

fn run(n: usize, memory_mb: f64) {
    set_io(IoHooks {
        read: |b, name| cadaddy_io::read(b, name).map_err(|e| e.to_string()),
        write: |d, name| cadaddy_io::write(d, name).map_err(|e| e.to_string()),
        plot: None,
        read_pdf: None,
    });
    cadaddy_engine::cmd::file::set_dwg_copy(false);
    let dir = std::env::temp_dir().join(format!("cadaddy-million-{}-{n}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let dxf = dir.join("big.dxf");
    std::fs::write(&dxf, generate(n)).unwrap();
    // The budgets are for 200,000 objects and grow in proportion for more.
    let scale = (n as f64 / 200_000.0).max(1.0);
    let mut r = Report { n, lines: Vec::new(), peak_mb: 0.0 };
    let mut s = Session::new();
    let opened = r.step("open DXF", 3000.0 * scale, || s.execute("open", &json!({ "path": dxf.to_string_lossy() })).unwrap());
    assert!(opened.get("index").is_some(), "{opened}");
    let count = s.doc().unwrap().model.len();
    assert_eq!(count, n);
    let ext = s.doc().unwrap().extents(&Space::Model);
    let doc = s.state().unwrap().doc.clone();
    let list = r.step("regen (whole drawing)", 1000.0 * scale, || cadaddy_render::build(&doc, &Space::Model, &view_opts(ext.height(), None)));
    r.step("GPU mesh (whole drawing)", 600.0 * scale, || {
        cadaddy_ui_egui::gpu::build_mesh(
            &list,
            cadaddy_ui_egui::gpu::choose_origin(&list.bounds, ext.center(), ext.height()),
            cadaddy_color::Rgb(0, 0, 0),
            false,
        )
    });
    drop(list);
    // Zoomed in on a 1% corner: a build for that area only.
    let corner = cadaddy_geom::Bounds2::new(ext.min, ext.min + (ext.max - ext.min) * 0.1);
    r.step("zoom in (1% of the drawing)", 500.0 * scale, || cadaddy_render::build(&doc, &Space::Model, &view_opts(corner.height(), Some(corner))));
    drop(doc);
    // Select a tenth of the drawing with a window, move it, undo.
    let band = [[ext.min.x - 1.0, ext.min.y - 1.0], [ext.max.x + 1.0, ext.min.y + ext.height() * 0.1]];
    let sel = r.step("select (window, 10%)", 1000.0 * scale, || s.execute("select", &json!({ "window": band })).unwrap());
    let picked = s.state().unwrap().selection.len();
    assert!(picked > n / 20, "{picked} of {n} selected: {sel}");
    r.step("move (10%)", 600.0 * scale, || s.execute("move", &json!({ "from": [0, 0], "to": [3, 7] })).unwrap());
    let moved = s.state().unwrap().doc.clone();
    r.step("regen after the move", 1000.0 * scale, || cadaddy_render::build(&moved, &Space::Model, &view_opts(ext.height(), None)));
    drop(moved);
    r.step("undo", 200.0 * scale, || s.execute("u", &json!({})).unwrap());
    let sel_again = s.execute("select", &json!({ "window": band })).unwrap();
    assert_eq!(s.state().unwrap().selection.len(), picked, "undo put everything back: {sel_again}");
    r.step("pick one object", 50.0, || s.execute("select", &json!({ "at": [ext.min.x + 5.0, ext.min.y + 1.0] })).unwrap());
    let cdy = dir.join("big.cdy");
    r.step("save .cdy", 3000.0 * scale, || s.execute("saveas", &json!({ "path": cdy.to_string_lossy() })).unwrap());
    let out = dir.join("out.dxf");
    r.step("save DXF", 6000.0 * scale, || s.execute("saveas", &json!({ "path": out.to_string_lossy() })).unwrap());
    // In a session of its own, with the first one closed: one drawing in memory at a time.
    drop(s);
    let mut s2 = Session::new();
    r.step("open .cdy", 2000.0 * scale, || s2.execute("open", &json!({ "path": cdy.to_string_lossy() })).unwrap());
    assert_eq!(s2.doc().unwrap().model.len(), n);
    drop(s2);
    r.print();
    let _ = std::fs::remove_dir_all(&dir);
    let over = r.over();
    assert!(over.is_empty(), "over budget: {over:?}");
    assert!(r.peak_mb < memory_mb, "peak memory {:.0} MB > {memory_mb} MB", r.peak_mb);
}

#[test]
fn a_large_drawing_within_budgets() {
    let n = std::env::var("MILLION_N").ok().and_then(|v| v.parse().ok()).unwrap_or(200_000);
    // Measured (Apple M-series, this test's build): 200k objects peak at about 0.6 GB, 1M at
    // 2.7 GB with two copies of the drawing alive in turn.
    run(n, 1500.0 * (n as f64 / 200_000.0).max(1.0));
}
