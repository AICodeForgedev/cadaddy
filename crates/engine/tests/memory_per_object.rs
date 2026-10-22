//! Memory per object and the DWG open spike (Q250): a drawing of 100,000 objects (lines,
//! polylines with arcs, circles, arcs, texts and 3D faces on ten layers, as `cargo xtask perf`
//! makes them) holds under [`BYTES_PER_OBJECT`] per object, and opening it as DWG takes at most
//! [`OPEN_PEAK_PER_LIVE`] times the memory the open drawing holds.
//!
//! Before Q250 the drawing held 575 bytes an object and the open peaked at 4.6 times the open
//! drawing (the converted DXF parsed a second time into text pairs, the text kept while the
//! drawing was built, its groups and every object copied twice over); after it, 413 bytes and
//! about 2.8 times (most of what is left is the DWG bridge's own conversion).
//!
//! The process's memory is the measurement, so this is the only test in its binary, and the
//! open runs in a fresh process (this binary again, one test selected): the peak a process
//! reports is its whole life's. Process memory is its private bytes on Windows and anonymous
//! resident memory on Linux; where it can't be read (macOS keeps no peak), only the structure is
//! checked.

use std::path::PathBuf;

use cadaddy_engine::doc::geom::{PolyVertex, Vec2, Vec3};
use cadaddy_engine::doc::{Arc, Circle, Common, Drawing, EntityKind, Face3d, Layer, Line, LwPolyline, Space, Text};
use serde_json::json;

/// Objects in the drawing.
const OBJECTS: usize = 100_000;
/// The most memory one object may hold, in bytes (measured: 413 on Windows; 575 before).
const BYTES_PER_OBJECT: f64 = 500.0;
/// The most memory an open may take, as a multiple of what the open drawing holds (measured:
/// 2.8; 4.6 before).
const OPEN_PEAK_PER_LIVE: f64 = 4.0;
/// Set in the child process: the DWG file it opens.
const CHILD: &str = "CADADDY_MEMDIET_OPEN";

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
            4 => EntityKind::Text(
                serde_json::from_value::<Text>(json!({ "insert": { "x": x, "y": y, "z": 0.0 }, "height": 2.0, "value": format!("T{i}") })).unwrap(),
            ),
            _ => {
                let q = [Vec3::new(x, y, 0.0), Vec3::new(x + 8.0, y, 0.0), Vec3::new(x + 8.0, y + 8.0, 3.0), Vec3::new(x, y + 8.0, 3.0)];
                EntityKind::Face3d(Face3d { corners: q, hidden_edges: 0 })
            }
        };
        d.add(&Space::Model, c, kind).unwrap();
    }
    d
}

fn powershell_mb(field: &str) -> Option<f64> {
    let script = format!("(Get-Process -Id {}).{field}", std::process::id());
    let out = std::process::Command::new("powershell").args(["-NoProfile", "-NonInteractive", "-Command", &script]).output().ok()?;
    String::from_utf8_lossy(&out.stdout).trim().parse::<f64>().ok().map(|b| b / 1_048_576.0)
}

fn status_mb(key: &str) -> Option<f64> {
    let status = std::fs::read_to_string("/proc/self/status").ok()?;
    let kb: f64 = status.lines().find_map(|l| l.strip_prefix(key))?.trim().trim_end_matches("kB").trim().parse().ok()?;
    Some(kb / 1024.0)
}

/// This process's memory now, in MB.
fn memory_mb() -> Option<f64> {
    if cfg!(windows) { powershell_mb("PrivateMemorySize64") } else { status_mb("RssAnon:") }
}

/// The most memory this process has held, in MB (Windows: peak commit; Linux: peak resident).
fn peak_mb() -> Option<f64> {
    if cfg!(windows) { powershell_mb("PeakPagedMemorySize64") } else { status_mb("VmHWM:") }
}

/// The memory [`peak_mb`] measures, now (Linux: all resident memory, code included).
fn peak_base_mb() -> Option<f64> {
    if cfg!(windows) { memory_mb() } else { status_mb("VmRSS:") }
}

/// One test (the process's memory is the measurement: nothing may run beside it): the memory
/// per object first, then the open, in a fresh process.
#[test]
fn memory_per_object_and_the_dwg_open() {
    // The child: open the file, report its memory.
    if let Ok(path) = std::env::var(CHILD) {
        let bytes = std::fs::read(&path).unwrap();
        let before = memory_mb().unwrap_or(0.0);
        let base = peak_base_mb().unwrap_or(0.0);
        let start_peak = peak_mb().unwrap_or(0.0).max(base);
        let d = cadaddy_io::read(&bytes, &path).unwrap();
        let live = memory_mb().unwrap_or(0.0) - before;
        let peak = peak_mb().unwrap_or(0.0);
        // The open's own peak only shows when it rises above what start-up took.
        let peak = if peak > start_peak { peak - base } else { live };
        println!("MEMDIET {}", json!({ "objects": d.model.len(), "live_mb": live, "peak_mb": peak }));
        return;
    }
    let before = memory_mb();
    let d = drawing(OBJECTS);
    let after = memory_mb();
    assert_eq!(d.model.len(), OBJECTS);
    let (Some(before), Some(after), Some(_)) = (before, after, peak_mb()) else {
        eprintln!("process memory or its peak can't be read here: structure checked only");
        return;
    };
    let per = (after - before) * 1_048_576.0 / OBJECTS as f64;
    eprintln!("{per:.0} bytes per object");
    assert!(per < BYTES_PER_OBJECT, "{per:.0} bytes per object (at most {BYTES_PER_OBJECT})");

    let dir = std::env::temp_dir().join(format!("cadaddy-memdiet-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path: PathBuf = dir.join("Sample Plan.dwg");
    std::fs::write(&path, cadaddy_io::write(&d, "Sample Plan.dwg").unwrap()).unwrap();
    drop(d);
    let out = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["memory_per_object_and_the_dwg_open", "--exact", "--nocapture"])
        .env(CHILD, &path)
        .output()
        .unwrap();
    let _ = std::fs::remove_dir_all(&dir);
    let text = String::from_utf8_lossy(&out.stdout);
    let line = text.lines().find_map(|l| l.strip_prefix("MEMDIET ")).unwrap_or_else(|| panic!("no report: {text}"));
    let r: serde_json::Value = serde_json::from_str(line).unwrap();
    let (live, peak) = (r["live_mb"].as_f64().unwrap(), r["peak_mb"].as_f64().unwrap());
    eprintln!("open: {live:.1} MB held, {peak:.1} MB at the peak");
    assert_eq!(r["objects"].as_u64(), Some(OBJECTS as u64));
    assert!(live > 0.0 && peak <= OPEN_PEAK_PER_LIVE * live, "the open peaked at {peak:.1} MB for a {live:.1} MB drawing");
}
