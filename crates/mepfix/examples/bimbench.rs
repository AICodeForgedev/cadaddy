//! The assembled BIM pipeline, end to end, on the synthetic hotel: for `cargo xtask perf` (its
//! budgets in `perf/budget.json`) and `docs/bim-perf.md`.
//!
//! `cargo run -p cadaddy-mepfix --example bimbench -- [--sizes full,double] [--out FILE] [--seed N] [--only STAGE,…]`
//!
//! Each size (`small`, `full` = 10 rooms a side per floor, `double` = 20) generates the hotel's
//! print set (vector PDF and DXF sheets, made-up building) into a scratch folder, then times every
//! stage a person or an agent takes it through, as engine commands:
//!
//! - `import`: the folder as a print-set project (`printset.open`) and its sheets classified
//!   (`printset.classify`);
//! - `pdf_to_cdy`: every plan PDF imported (`pdfimport`) and saved as `.cdy`; `classify`: each
//!   architectural plan read as a plan (`plan.classify`);
//! - `open_dxf`: every DXF sheet opened;
//! - the print readers on every sheet they read: `read_sheets` (every DXF parsed, sheets
//!   recognised and aligned), `read_ceilings`, `read_struct`, `read_walls`, `read_arch`,
//!   `read_assemblies`, `read_ducts` (mechanical plans), `read_pipes` (plumbing plans),
//!   `read_sprinklers`, `read_devices` (electrical), `read_callouts`;
//! - `build_arch` (plan → spec → `model.build`);
//! - `validate` (`bspec.validate`, `model.issues`), `clashes` (`model.clashes`), `review`
//!   (`rfi.list`), `edit_validate` (a spec edit, then `bspec.validate` and `model.issues`);
//! - `takeoff`, `export_ifc`, `export_bcf`, `render` (`model.render`), `save_cdy` / `open_cdy`;
//! - `revision`: one sheet changed and the set re-imported (`printset.reimport`): only it is
//!   reported changed;
//! - `briefing` (`project.briefing`) and `validate_full` (`project.validate`) when the build has
//!   them.
//!
//! Written as JSON `{"bim_<stage>@<size>": ms}` with `peak_mb@bim_<size>` (resident memory,
//! sampled after each stage with `ps`), `cpu_cores@bim_<size>` (CPU time ÷ wall time over the
//! stages: how many cores were busy), `cdy_kb@bim_<size>`, and `resp_kb@bim_<size>` (the largest
//! command answer, what an agent reads: about 250 tokens per KB). `--detail FILE` also writes
//! every command's time and answer size.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Instant;

use cadaddy_engine::Session;
use cadaddy_engine::cmd::file::{IoHooks, set_io};
use cadaddy_mepfix::hotel::HotelSize;
use serde_json::{Value, json};

fn hooks() {
    set_io(IoHooks {
        read: |b, name| cadaddy_io::read(b, name).map_err(|e| e.to_string()),
        write: |d, name| cadaddy_io::write(d, name).map_err(|e| e.to_string()),
        plot: Some(|d, space, opts| cadaddy_io::plotter::plot(d, space, opts).map_err(|e| e.to_string())),
        read_pdf: Some(cadaddy_io::pdf_read::read_json),
        read_dwf: None,
        write_dwfx: None,
    });
}

/// This process's resident memory, MB (`ps`; none where there is no `ps`).
fn rss_mb() -> Option<f64> {
    let out = std::process::Command::new("ps").args(["-o", "rss=", "-p", &std::process::id().to_string()]).output().ok()?;
    String::from_utf8_lossy(&out.stdout).trim().parse::<f64>().ok().map(|kb| kb / 1024.0)
}

/// This process's CPU time so far, seconds (`ps -o time=`: `[[dd-]hh:]mm:ss[.cc]`).
fn cpu_s() -> Option<f64> {
    let out = std::process::Command::new("ps").args(["-o", "time=", "-p", &std::process::id().to_string()]).output().ok()?;
    let t = String::from_utf8_lossy(&out.stdout).trim().to_string();
    let (days, rest) = t.split_once('-').map_or((0.0, t.as_str()), |(d, r)| (d.parse().unwrap_or(0.0), r));
    let mut secs = 0.0;
    for part in rest.split(':') {
        secs = secs * 60.0 + part.parse::<f64>().ok()?;
    }
    Some(days * 86_400.0 + secs)
}

/// One command's cost: calls, total time and largest answer.
#[derive(Default)]
struct Cost {
    calls: usize,
    ms: f64,
    max_bytes: usize,
    errors: Vec<String>,
}

struct Bench {
    size: String,
    out: BTreeMap<String, f64>,
    detail: BTreeMap<String, Cost>,
    peak: f64,
    notes: Vec<String>,
}

impl Bench {
    /// Run a command, counting its time and answer size; errors are noted, not fatal.
    fn exec(&mut self, s: &mut Session, cmd: &str, p: Value) -> Option<Value> {
        let t = Instant::now();
        let r = s.execute(cmd, &p);
        let ms = t.elapsed().as_secs_f64() * 1000.0;
        let c = self.detail.entry(cmd.to_string()).or_default();
        c.calls += 1;
        c.ms += ms;
        match r {
            Ok(v) => {
                c.max_bytes = c.max_bytes.max(v.to_string().len());
                Some(v)
            }
            Err(e) => {
                if c.errors.len() < 3 {
                    c.errors.push(e.to_string());
                }
                None
            }
        }
    }

    /// Time a stage; record it and the memory after it.
    fn stage<T>(&mut self, name: &str, f: impl FnOnce(&mut Self) -> T) -> T {
        let t = Instant::now();
        let r = f(self);
        let ms = t.elapsed().as_secs_f64() * 1000.0;
        let ms = (ms * 10.0).round() / 10.0;
        println!("{name:>16} @ {:<7}: {ms:10.1} ms", self.size);
        self.out.insert(format!("bim_{name}@{}", self.size), ms);
        if let Some(m) = rss_mb() {
            self.peak = self.peak.max(m);
        }
        r
    }
}

/// A fresh session with one file opened.
fn opened(b: &mut Bench, path: &Path) -> Option<Session> {
    let mut s = Session::new();
    b.exec(&mut s, "open", json!({ "path": path.to_string_lossy() }))?;
    Some(s)
}

fn file(dir: &Path, sheet: &str, ext: &str) -> PathBuf {
    dir.join(format!("{sheet}.{ext}"))
}

fn run_size(size_name: &str, size: HotelSize, seed: u64, only: &[String], scratch: &Path) -> Result<Bench, String> {
    let mut b =
        Bench { size: size_name.to_string(), out: BTreeMap::new(), detail: BTreeMap::new(), peak: rss_mb().unwrap_or(0.0), notes: Vec::new() };
    let want = |s: &str| only.is_empty() || only.iter().any(|o| o == s);
    let dir = scratch.join(format!("hotel-{size_name}"));
    let _ = std::fs::remove_dir_all(&dir);
    let t = Instant::now();
    let set = cadaddy_mepfix::generate_hotel(seed, size)?;
    set.write_to(&dir).map_err(|e| e.to_string())?;
    let sheets: Vec<String> = set.truth.sheets.iter().map(|s| s.number.clone()).collect();
    println!(
        "{:>16} @ {size_name:<7}: {:10.1} ms ({} sheets, setup, not timed as a stage)",
        "generate",
        t.elapsed().as_secs_f64() * 1000.0,
        sheets.len()
    );
    let (cpu0, wall0) = (cpu_s(), Instant::now());
    let plans: Vec<&String> = sheets.iter().filter(|n| ["A-1", "M-1", "P-1", "FP-1", "E-1", "S-1"].iter().any(|p| n.starts_with(p))).collect();

    // The project.
    let mut proj = Session::new();
    if want("import") {
        b.stage("import", |b| {
            b.exec(&mut proj, "printset.open", json!({ "path": dir.to_string_lossy(), "name": "Sample Hotel" }));
            b.exec(&mut proj, "printset.classify", json!({}));
        });
    }
    // Every plan PDF to .cdy, the architectural ones read as plans.
    if want("pdf_to_cdy") {
        let cdy_dir = dir.join("cdy");
        let _ = std::fs::create_dir_all(&cdy_dir);
        let mut classify_ms = 0.0;
        let mut cdy_bytes = 0usize;
        b.stage("pdf_to_cdy", |b| {
            for n in &plans {
                let mut s = Session::new();
                if b.exec(&mut s, "pdfimport", json!({ "path": file(&dir, n, "pdf").to_string_lossy(), "layout": false, "images": false })).is_none()
                {
                    continue;
                }
                if n.starts_with("A-1") {
                    let t = Instant::now();
                    b.exec(&mut s, "plan.classify", json!({}));
                    classify_ms += t.elapsed().as_secs_f64() * 1000.0;
                }
                if let Ok(d) = s.doc()
                    && let Ok(bytes) = cadaddy_io::write(d, "sheet.cdy")
                {
                    cdy_bytes += bytes.len();
                    let _ = std::fs::write(cdy_dir.join(format!("{n}.cdy")), bytes);
                }
            }
        });
        b.out.insert(format!("bim_classify@{size_name}"), (classify_ms * 10.0).round() / 10.0);
        b.out.insert(format!("cdy_kb@bim_{size_name}"), (cdy_bytes as f64 / 1024.0).round());
    }
    if want("open_dxf") {
        b.stage("open_dxf", |b| {
            for n in &sheets {
                let _ = opened(b, &file(&dir, n, "dxf"));
            }
        });
    }
    // The builds, on level 1 (level 2 for structure: its first framing plan).
    let mut arch = None;
    if want("build_arch") {
        arch = b.stage("build_arch", |b| {
            let mut s = opened(b, &file(&dir, "A-101", "dxf"))?;
            b.exec(&mut s, "plan.classify", json!({ "apply": true, "draft": true }));
            b.exec(&mut s, "model.build", json!({}));
            Some(s)
        });
    }
    // The print readers (what turns the set into a model), on every sheet they read.
    let dxf: Vec<(String, Vec<u8>)> = set.files.iter().filter(|(n, _)| n.ends_with(".dxf")).cloned().collect();
    let mut parsed: Vec<(String, cadaddy_engine::doc::Drawing)> = Vec::new();
    let mut map = None;
    if want("read_sheets") {
        map = Some(b.stage("read_sheets", |_| {
            parsed =
                dxf.iter().filter_map(|(n, bytes)| cadaddy_io::read(bytes, n).ok().map(|d| (n.trim_end_matches(".dxf").to_string(), d))).collect();
            cadaddy_mepfix::sheet_reader::recognise_fixture_set(&dxf)
        }));
    }
    if let Some(map) = map {
        let refs: Vec<(String, &cadaddy_engine::doc::Drawing)> = parsed.iter().map(|(n, d)| (n.clone(), d)).collect();
        let ceil = b.stage("read_ceilings", |_| cadaddy_mepfix::ceiling_reader::read_ceilings(&dxf, &map));
        let st = b.stage("read_struct", |_| cadaddy_mepfix::struct_reader::read_structure(&map, &refs));
        let walls = b.stage("read_walls", |_| cadaddy_mepfix::wall_reader::read_walls(&dxf, &map, &ceil, &st));
        let arch_r = b.stage("read_arch", |_| cadaddy_mepfix::arch_reader::read_arch(&dxf, &map, &ceil));
        let asm = b.stage("read_assemblies", |_| cadaddy_mepfix::ceilasm_reader::read_assemblies(&dxf, &map, &ceil, &walls));
        let sheet_of = |n: &str| set.truth.sheets.iter().find(|s| s.number == n);
        let (runs, terminals) = b.stage("read_ducts", |_| {
            let (mut runs, mut terms) = (0, 0);
            for (n, d) in &parsed {
                if let (true, Some(sh)) = (n.starts_with("M-1"), sheet_of(n)) {
                    let net = cadaddy_mepfix::duct_reader::read_ducts(d, sh);
                    runs += net.runs.len();
                    terms += net.terminals.len();
                }
            }
            (runs, terms)
        });
        let (pipes, risers) = b.stage("read_pipes", |_| {
            let (mut p, mut r) = (0, 0);
            for (n, d) in &parsed {
                if let (true, Some(sh)) = (n.starts_with("P-1"), sheet_of(n)) {
                    let (found, rs) = cadaddy_mepfix::pipe_reader::read_pipes(d, sh);
                    p += found.len();
                    r += rs.len();
                }
            }
            (p, r)
        });
        let heads = b.stage("read_sprinklers", |_| cadaddy_mepfix::sprinkler_reader::read_sprinklers(&dxf, &map, &ceil));
        let (devices, panels) = b.stage("read_devices", |_| {
            let inputs = cadaddy_mepfix::elec_reader::DeviceInputs::read(&dxf, &map);
            let r = cadaddy_mepfix::elec_reader::read_devices(&dxf, &map, &inputs);
            (r.devices, r.panels)
        });
        let numbers: Vec<String> = set.truth.sheets.iter().map(|s| s.number.clone()).collect();
        let (resolved, broken) = b.stage("read_callouts", |_| cadaddy_mepfix::callout_reader::resolve_callouts(&dxf, &numbers));
        b.notes.push(format!(
            "read: {} sheets, {} rooms/{} ceilings, {} columns, {} walls, {} doors, {} wall types, {} duct runs/{} terminals, {} pipe runs/{} risers, {} sprinkler heads, {} devices/{} panels, {} callouts ({} unresolved)",
            map.sheets.len(),
            ceil.rooms.len(),
            ceil.ceilings.len(),
            st.columns.len(),
            walls.walls.len(),
            arch_r.doors.len(),
            asm.walls.len(),
            runs,
            terminals,
            pipes,
            risers,
            heads.heads.len(),
            devices.len(),
            panels.len(),
            resolved.len(),
            broken.len()
        ));
    }
    if let Some(s) = arch.as_mut() {
        b.stage("validate", |b| {
            b.exec(s, "bspec.validate", json!({}));
            b.exec(s, "model.issues", json!({}));
        });
        b.stage("review", |b| {
            b.exec(s, "rfi.list", json!({}));
        });
        b.stage("takeoff", |b| {
            b.exec(s, "takeoff", json!({}));
        });
        b.stage("export_ifc", |b| {
            if let Ok(d) = s.doc() {
                match cadaddy_io::ifc::write_ifc(d, "hotel.ifc") {
                    Ok(bytes) => b.out.insert(format!("ifc_kb@bim_{size_name}"), (bytes.len() as f64 / 1024.0).round()),
                    Err(e) => {
                        b.notes.push(format!("export_ifc: {e}"));
                        None
                    }
                };
            }
        });
        let png = dir.join("render.png");
        b.stage("render", |b| {
            b.exec(s, "model.render", json!({ "path": png.to_string_lossy(), "width": 800, "height": 600 }));
        });
        let cdy = dir.join("A-101-built.cdy");
        b.stage("save_cdy", |b| {
            b.exec(s, "saveas", json!({ "path": cdy.to_string_lossy() }));
        });
        if cdy.exists() {
            b.stage("open_cdy", |b| {
                let _ = opened(b, &cdy);
            });
        }
        b.stage("clashes", |b| {
            b.exec(s, "model.clashes", json!({}));
        });
        // An agent's edit, then the check it runs after it.
        b.stage("edit_validate", |b| {
            let level = s.execute("bspec.get", &json!({})).ok().and_then(|v| v["spec"]["levels"][0]["name"].as_str().map(String::from));
            b.exec(s, "bspec.set", json!({ "spec": { "levels": [{ "name": level, "ceiling_ft": 9 }] } }));
            b.exec(s, "bspec.validate", json!({}));
            b.exec(s, "model.issues", json!({}));
        });
        let bcf = dir.join("clashes.bcfzip");
        b.stage("export_bcf", |b| {
            b.exec(s, "coordination.bcf", json!({ "path": bcf.to_string_lossy() }));
        });
        // An agent's loop, on the project: where it stands, then a full check.
        if s.execute("project.briefing", &json!({})).is_ok() {
            b.stage("briefing", |b| {
                b.exec(s, "project.briefing", json!({}));
            });
        }
        if s.execute("project.validate", &json!({})).is_ok() {
            b.stage("validate_full", |b| {
                b.exec(s, "project.validate", json!({}));
            });
        }
    }
    // A revision: one sheet changes; only it should be redone.
    if want("revision") && want("import") {
        let changed = file(&dir, "M-102", "dxf");
        if let Ok(mut text) = std::fs::read_to_string(&changed) {
            text.push_str("999\nrevision 2\n");
            let _ = std::fs::write(&changed, text);
        }
        let r = b.stage("revision", |b| b.exec(&mut proj, "printset.reimport", json!({ "path": dir.to_string_lossy(), "revision": "2" })));
        let changed_n = r.as_ref().and_then(|v| v["delta"]["changed"].as_array().map(Vec::len).or_else(|| v["changed"].as_array().map(Vec::len)));
        b.notes.push(format!("revision: {changed_n:?} sheet(s) reported changed (expected 1)"));
    }
    let wall = wall0.elapsed().as_secs_f64();
    if let (Some(c0), Some(c1)) = (cpu0, cpu_s())
        && wall > 0.0
    {
        b.out.insert(format!("cpu_cores@bim_{size_name}"), ((c1 - c0) / wall * 100.0).round() / 100.0);
    }
    b.out.insert(format!("peak_mb@bim_{size_name}"), (b.peak * 10.0).round() / 10.0);
    let resp = b.detail.values().map(|c| c.max_bytes).max().unwrap_or(0);
    b.out.insert(format!("resp_kb@bim_{size_name}"), (resp as f64 / 1024.0 * 10.0).round() / 10.0);
    let _ = std::fs::remove_dir_all(&dir);
    Ok(b)
}

fn main() {
    hooks();
    let args: Vec<String> = std::env::args().skip(1).collect();
    let arg = |k: &str| args.iter().position(|a| a == k).and_then(|i| args.get(i + 1)).cloned();
    let sizes = arg("--sizes").unwrap_or_else(|| "full".into());
    let seed = arg("--seed").and_then(|s| s.parse().ok()).unwrap_or(7);
    let only: Vec<String> = arg("--only").map(|s| s.split(',').map(String::from).collect()).unwrap_or_default();
    let scratch = std::env::temp_dir().join(format!("cadaddy-bimbench-{}", std::process::id()));
    let mut out = BTreeMap::new();
    let mut detail = serde_json::Map::new();
    for name in sizes.split(',') {
        let size = match name {
            "small" => HotelSize::Small,
            "full" => HotelSize::Full,
            "double" => HotelSize::Double,
            other => {
                eprintln!("unknown size {other} (small, full, double)");
                std::process::exit(2);
            }
        };
        match run_size(name, size, seed, &only, &scratch) {
            Ok(b) => {
                for n in &b.notes {
                    println!("  note: {n}");
                }
                let mut rows: Vec<(&String, &Cost)> = b.detail.iter().collect();
                rows.sort_by(|a, c| c.1.ms.total_cmp(&a.1.ms));
                println!("  slowest commands @ {name}:");
                for (cmd, c) in rows.iter().take(12) {
                    println!(
                        "    {cmd:<22} {:>4}× {:>10.1} ms  answer ≤ {:>7.1} KB{}",
                        c.calls,
                        c.ms,
                        c.max_bytes as f64 / 1024.0,
                        c.errors.first().map_or(String::new(), |e| format!("  (error: {e})"))
                    );
                }
                let d: serde_json::Map<String, Value> = b
                    .detail
                    .iter()
                    .map(|(k, c)| (k.clone(), json!({ "calls": c.calls, "ms": (c.ms * 10.0).round() / 10.0, "max_kb": (c.max_bytes as f64 / 102.4).round() / 10.0, "errors": c.errors })))
                    .collect();
                detail.insert(name.to_string(), Value::Object(d));
                println!("  peak {:.1} MB", b.peak);
                out.extend(b.out);
            }
            Err(e) => {
                eprintln!("{name}: {e}");
                std::process::exit(1);
            }
        }
    }
    let _ = std::fs::remove_dir_all(&scratch);
    if let Some(path) = arg("--out") {
        let _ = std::fs::write(&path, serde_json::to_string_pretty(&out).unwrap_or_default());
    }
    if let Some(path) = arg("--detail") {
        let _ = std::fs::write(&path, serde_json::to_string_pretty(&Value::Object(detail)).unwrap_or_default());
    }
}
