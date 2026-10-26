//! The assembled BIM pipeline, end to end, on the synthetic hotel: for `cargo xtask perf` (its
//! budgets in `perf/budget.json`) and `docs/bim-perf.md`.
//!
//! `cargo run -p cadaddy-mepfix --example bimbench -- [--sizes full,double] [--out FILE] [--detail FILE] [--seed N]`
//!
//! Each size (`small`, `full` = 10 rooms a side per floor, `double` = 20) generates the hotel's
//! print set (vector PDF and DXF sheets, made-up building) into a scratch folder, then times every
//! stage a person or an agent takes it through, as engine commands (and MCP tools):
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
//! - the whole job built as one model, every level and every discipline: `compose` (each level's
//!   architectural, structural, mechanical, plumbing, fire protection and electrical plans
//!   inserted side by side into one drawing, `insert` of the sheet files), `spec` (`bspec.draft`,
//!   then the levels with their plan regions, the trades' systems, and what the readers found
//!   that a drawing can't say: the columns, the electrical devices and the sprinkler heads,
//!   `bspec.set`), `mep_read` (`mep.read {apply: true}`), `build` (`bim.build {discipline:
//!   all}`), `rooms_build` (`rooms.build`). Its element counts per discipline are written as
//!   `elems_<discipline>@bim_<size>`;
//! - on that model: `validate` (`bspec.validate`, `model.issues`), `validate_full`
//!   (`bim.validate`, the model against the prints), `edit_validate` (an agent's edit, then the
//!   incremental check: `bim.validate` on the edited objects and `model.issues`), `clashes`
//!   (`model.clashes`), `review` (`review.list`, `rfi.list`), `briefing` (`project.briefing`),
//!   `takeoff`, `export_ifc`, `export_bcf`, `render` (`model.render`), `save_cdy` / `open_cdy`;
//! - `spec_intake`: the hotel's spec book read into proposed types (`bim.spec_import`, as the MCP
//!   tool `bim_spec_import` does), accepted (`bim.spec_accept`), and a submittal logged per
//!   section (`procure.submittal`, `procure.submittals`);
//! - `viewer_open` / `viewer_orbit`: the 3D scene Walk mode derives when it opens the model, then
//!   eight frames orbiting it. The viewer draws with the GPU, which has no headless path here: the
//!   frames are the software renderer's (`model.render` at 640×480), an upper bound on a GPU frame;
//! - `soak`: a long agent session, 200 cycles of edit → validate → `model.issues` with undo off
//!   (each edit's undo step is kept by design), with the resident memory it grew by as
//!   `leak_mb@bim_full` (after ten warm-up cycles);
//! - `mcp_<tool>_ms` / `mcp_<tool>_kb`: one call of each agent tool (`execute`, `bim_job`,
//!   `project`, `mep`, `rooms`) through the MCP server on the built model, its time and answer
//!   size, checked against the agent answer caps (Q319);
//! - `revision`: one sheet changed and the set re-imported (`printset.reimport`): exactly one
//!   sheet must be reported changed.
//!
//! Every stage is also checked: a command that errors, or a key result that is empty where the
//! hotel's truth has items, is a FAILED line, and the run exits non-zero (so `cargo xtask perf`
//! fails), as does the double hotel's model having under 1.8× the full hotel's elements.
//!
//! Written as JSON `{"bim_<stage>@<size>": ms}` with `peak_mb@bim_<size>` (resident memory,
//! sampled after each stage with `ps`), `cpu_cores@bim_<size>` (CPU time ÷ wall time over the
//! stages: how many cores were busy), `cdy_kb@bim_<size>`, `ifc_kb@bim_<size>` and
//! `resp_kb@bim_<size>` (the largest command answer, what an agent reads: about 250 tokens per
//! KB). `--detail FILE` also writes every command's time and answer size.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Instant;

use cadaddy_engine::Session;
use cadaddy_engine::cmd::agentready::{ANSWER_MAX, BRIEFING_MAX};
use cadaddy_engine::cmd::file::{IoHooks, set_io};
use cadaddy_mepfix::hotel::HotelSize;
use cadaddy_mepfix::truth::Truth;
use serde_json::{Value, json};

/// The agent-session soak: cycles, and the warm-up ones before memory is first read.
const SOAK_CYCLES: usize = 200;
const SOAK_WARMUP: usize = 10;
/// Orbit frames for the viewer stage.
const ORBIT_FRAMES: usize = 8;
/// The double hotel's model must have at least this many times the full hotel's elements.
const DOUBLE_MIN_RATIO: f64 = 1.8;

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

fn round1(v: f64) -> f64 {
    (v * 10.0).round() / 10.0
}

fn len(v: &Value) -> usize {
    v.as_array().map_or(0, Vec::len)
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
    /// The stage running now (failures name it).
    current: String,
    /// What went wrong: an errored command or an empty key result, as `stage: what`.
    failures: Vec<String>,
}

impl Bench {
    fn new(size: &str) -> Bench {
        Bench {
            size: size.to_string(),
            out: BTreeMap::new(),
            detail: BTreeMap::new(),
            peak: rss_mb().unwrap_or(0.0),
            notes: Vec::new(),
            current: "setup".into(),
            failures: Vec::new(),
        }
    }

    /// Run a command, counting its time and answer size. An error fails the stage.
    fn exec(&mut self, s: &mut Session, cmd: &str, p: Value) -> Option<Value> {
        let r = self.run(s, cmd, p);
        if let Err(e) = &r {
            self.fail(format!("{cmd} failed: {}", e.chars().take(300).collect::<String>()));
        }
        r.ok()
    }

    /// Run a command whose error is an answer (a probe), counted like any other.
    fn run(&mut self, s: &mut Session, cmd: &str, p: Value) -> Result<Value, String> {
        let t = Instant::now();
        let r = s.execute(cmd, &p);
        let ms = t.elapsed().as_secs_f64() * 1000.0;
        let c = self.detail.entry(cmd.to_string()).or_default();
        c.calls += 1;
        c.ms += ms;
        match r {
            Ok(v) => {
                c.max_bytes = c.max_bytes.max(v.to_string().len());
                Ok(v)
            }
            Err(e) => {
                let e = e.to_string();
                if c.errors.len() < 3 {
                    c.errors.push(e.clone());
                }
                Err(e)
            }
        }
    }

    fn fail(&mut self, what: String) {
        self.failures.push(format!("{} @ {}: {what}", self.current, self.size));
    }

    /// A key result: `ok` or the stage fails with `what`.
    fn check(&mut self, ok: bool, what: impl FnOnce() -> String) {
        if !ok {
            self.fail(what());
        }
    }

    /// A count the truth says is above zero.
    fn nonzero(&mut self, n: usize, what: &str) {
        self.check(n > 0, || format!("{what}: none found, the hotel has some"));
    }

    /// Time a stage; record it and the memory after it.
    fn stage<T>(&mut self, name: &str, f: impl FnOnce(&mut Self) -> T) -> T {
        self.current = name.to_string();
        let t = Instant::now();
        let r = f(self);
        let ms = round1(t.elapsed().as_secs_f64() * 1000.0);
        println!("{name:>16} @ {:<7}: {ms:10.1} ms", self.size);
        self.out.insert(format!("bim_{name}@{}", self.size), ms);
        if let Some(m) = rss_mb() {
            self.peak = self.peak.max(m);
        }
        r
    }

    fn put(&mut self, key: &str, v: f64) {
        self.out.insert(format!("{key}@bim_{}", self.size), v);
    }

    #[cfg(test)]
    fn exit_code(&self) -> i32 {
        exit_code(&self.failures)
    }
}

/// The exit code a run ends with: non-zero when any stage failed its check.
fn exit_code(failures: &[String]) -> i32 {
    i32::from(!failures.is_empty())
}

/// The revision check: exactly one sheet reported changed.
fn revision_ok(changed: Option<usize>) -> bool {
    changed == Some(1)
}

/// The double hotel's model against the full one's: `None` when it has enough elements, else why.
fn scaling_failure(full: f64, double: f64) -> Option<String> {
    (double < DOUBLE_MIN_RATIO * full)
        .then(|| format!("build: the double hotel's model has {double} elements, under {DOUBLE_MIN_RATIO}× the full hotel's {full}"))
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

/// The sheets each level's model is assembled from: the full-floor plan of each discipline (its
/// sheet number's start) and the layers taken from it (every plan repeats the architectural
/// background; it is taken once, from the architectural plan).
const JOB_SHEETS: &[(&str, &str, &[&str])] = &[
    ("A", "A-1", &["ARCH", "ARCH-TEXT", "LIGHT"]),
    ("S", "S-1", &["STRUCT"]),
    ("M", "M-1", &["DUCT", "EQUIP"]),
    ("P", "P-1", &["PIPE", "EQUIP"]),
    ("FP", "FP-1", &["PIPE"]),
    ("E", "E-2", &["DEVICE", "CONDUIT", "TRAY"]),
];

/// Where each level sits in the assembled drawing (inches): side by side, `pitch` apart.
struct JobLayout {
    pitch: f64,
    min_ft: [f64; 2],
    max_ft: [f64; 2],
}

impl JobLayout {
    fn new(t: &Truth) -> JobLayout {
        let (mut lo, mut hi) = ([f64::MAX; 2], [f64::MIN; 2]);
        for r in &t.building.rooms {
            for (i, (l, h)) in lo.iter_mut().zip(hi.iter_mut()).enumerate() {
                *l = l.min(r.min_ft.get(i).copied().unwrap_or(0.0));
                *h = h.max(r.max_ft.get(i).copied().unwrap_or(0.0));
            }
        }
        if lo[0] > hi[0] {
            (lo, hi) = ([0.0; 2], [100.0; 2]);
        }
        JobLayout { pitch: (hi[0] - lo[0] + 60.0) * 12.0, min_ft: lo, max_ft: hi }
    }

    /// Level `i`'s building origin in the drawing.
    fn base(&self, i: usize) -> [f64; 2] {
        [i as f64 * self.pitch, 0.0]
    }

    /// Level `i`'s plan region: the building and ten feet round it.
    fn region(&self, i: usize) -> [f64; 4] {
        let [x, y] = self.base(i);
        [
            x + (self.min_ft[0] - 10.0) * 12.0,
            y + (self.min_ft[1] - 10.0) * 12.0,
            x + (self.max_ft[0] + 10.0) * 12.0,
            y + (self.max_ft[1] + 10.0) * 12.0,
        ]
    }

    /// A point of level `i` in building feet, in the drawing.
    fn at(&self, i: usize, ft: [f64; 2]) -> [f64; 2] {
        let [x, y] = self.base(i);
        [x + ft[0] * 12.0, y + ft[1] * 12.0]
    }
}

/// What the readers found that goes into the model as data: columns (grid, plan point in feet,
/// footprint inches), devices (level code, kind, room) and sprinkler heads.
struct ReadIn {
    columns: Vec<(String, [f64; 2], [f64; 2])>,
    devices: Vec<(String, String, String)>,
    heads: cadaddy_mepfix::sprinkler_reader::SprinklerResult,
}

/// Each level's plans into one drawing, at the level's place (`insert` of each sheet's file with
/// only its discipline's layers kept, exploded). The sheets inserted.
fn compose(b: &mut Bench, job: &mut Session, dir: &Path, t: &Truth, lay: &JobLayout) -> usize {
    let parts = dir.join("job");
    let _ = std::fs::create_dir_all(&parts);
    let mut inserted = 0;
    for (i, l) in t.building.levels.iter().enumerate() {
        for (disc, start, keep) in JOB_SHEETS {
            let Some(sh) = t.sheets.iter().find(|s| {
                s.discipline == *disc && s.level.as_deref() == Some(l.code.as_str()) && s.number.starts_with(start) && s.window_ft.is_none()
            }) else {
                continue;
            };
            let Some(mut s) = opened(b, &dir.join(&sh.dxf)) else { continue };
            let drop: Vec<String> = s
                .doc()
                .map(|d| {
                    d.layers.iter().map(|x| x.name.to_string()).filter(|n| n != "0" && n != "Defpoints" && !keep.contains(&n.as_str())).collect()
                })
                .unwrap_or_default();
            if !drop.is_empty() {
                b.exec(&mut s, "laydel", json!({ "names": drop }));
            }
            let part = parts.join(format!("{}.dxf", sh.number));
            if b.exec(&mut s, "saveas", json!({ "path": part.to_string_lossy() })).is_none() {
                continue;
            }
            // The sheet's model space is the building turned and moved (`model_from_building`):
            // undo that, then move it to the level's place.
            let m = &sh.model_from_building;
            let (sin, cos) = (-m.rotation_deg).to_radians().sin_cos();
            let [ox, oy] = m.offset_in;
            let [bx, by] = lay.base(i);
            let at = [bx - (ox * cos - oy * sin), by - (ox * sin + oy * cos)];
            let name = format!("*JOB_{}_{}", l.code, sh.number.replace('-', "_"));
            if b.exec(job, "insert", json!({ "name": name, "file": part.to_string_lossy(), "at": at, "rotation": -m.rotation_deg })).is_some() {
                inserted += 1;
            }
        }
    }
    inserted
}

/// The building spec of the assembled job: `bspec.draft` for the layers, wall types and rooms,
/// then each level with its region, the trades' systems, and what the readers found.
fn job_spec(b: &mut Bench, job: &mut Session, t: &Truth, lay: &JobLayout, read: &ReadIn) -> Option<Value> {
    let draft = b.exec(job, "bspec.draft", json!({ "store": true }))?;
    let mut spec = b.exec(job, "bspec.get", json!({}))?.get("spec").cloned().unwrap_or_else(|| draft["spec"].clone());
    let names: Vec<(String, String)> = t.building.levels.iter().map(|l| (l.code.clone(), l.name.clone())).collect();
    let name_of = |code: &str| names.iter().find(|(c, _)| c == code).map(|(_, n)| n.clone());
    let levels: Vec<Value> = t
        .building
        .levels
        .iter()
        .enumerate()
        .map(|(i, l)| {
            let next = t.building.levels.get(i + 1).map_or(l.elevation_ft + 11.0, |n| n.elevation_ft);
            json!({ "name": l.name, "elevation_ft": l.elevation_ft, "height_ft": (next - l.elevation_ft).max(8.0), "ceiling_ft": 9, "region": lay.region(i), "base": lay.base(i) })
        })
        .collect();
    // The plan's walls keep the type the draft measured for them.
    let wall_type = draft["spec"]["layers"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|r| r["layer"] == "ARCH")
        .and_then(|r| r["wall_type"].as_str())
        .map(String::from);
    let layers = json!([
        { "layer": "ARCH", "role": "wall", "wall_type": wall_type },
        { "layer": "ARCH-TEXT", "role": "room_tag" },
        { "layer": "LIGHT", "role": "fixture" },
        { "layer": "*", "role": "ignore" }
    ]);
    let systems = json!([
        { "name": "SUPPLY", "layer": "DUCT", "kind": "duct", "size_in": [12, 10], "bottom_ft": 9.5 },
        { "name": "PIPING", "layer": "PIPE", "kind": "pipe", "size_in": [2, 0], "bottom_ft": 10 },
        { "name": "CONDUIT", "layer": "CONDUIT", "kind": "conduit", "size_in": [1, 0], "bottom_ft": 10 },
        { "name": "TRAY", "layer": "TRAY", "kind": "tray", "size_in": [12, 4], "bottom_ft": 10.5 },
        { "name": "SPRINKLER", "layer": "FP-SPRINKLER", "kind": "pipe", "size_in": [1, 0], "bottom_ft": 10 }
    ]);
    // Columns stand on every level with one above (floor to floor).
    let mut columns = Vec::new();
    for (i, l) in t.building.levels.iter().enumerate().filter(|(i, _)| *i + 1 < t.building.levels.len()) {
        for (grid, at, size) in &read.columns {
            columns.push(json!({ "level": l.name, "grid": grid, "at": lay.at(i, *at), "size_in": size }));
        }
    }
    let devices: Vec<Value> = read
        .devices
        .iter()
        .enumerate()
        .filter_map(|(k, (level, kind, room))| {
            Some(json!({ "tag": format!("D-{}", k + 1), "kind": kind, "level": name_of(level)?, "room": room, "source": "bimbench" }))
        })
        .collect();
    let level_names: Vec<(&str, &str)> = names.iter().map(|(c, n)| (c.as_str(), n.as_str())).collect();
    let mut terminals = cadaddy_mepfix::sprinkler_reader::spec_terminals(&read.heads, "SPRINKLER", &level_names);
    for term in &mut terminals {
        let i = t.building.levels.iter().position(|l| term["level"] == json!(l.name)).unwrap_or(0);
        let ft = [term["at"][0].as_f64().unwrap_or(0.0) / 12.0, term["at"][1].as_f64().unwrap_or(0.0) / 12.0];
        term["at"] = json!(lay.at(i, ft));
    }
    if let Some(o) = spec.as_object_mut() {
        o.insert("units_per_foot".into(), json!(12));
        o.insert("levels".into(), json!(levels));
        o.insert("layers".into(), layers);
        o.insert("columns".into(), json!(columns));
        o.insert("mep".into(), json!({ "systems": systems, "devices": devices, "terminals": terminals }));
    }
    let set = b.exec(job, "bspec.set", json!({ "replace": true, "spec": spec }))?;
    // What the prints say, for the print validator: the room schedule (building feet).
    let rooms: Vec<Value> = t
        .building
        .rooms
        .iter()
        .filter_map(|r| {
            let (a, c) = (r.min_ft, r.max_ft);
            Some(json!({ "level": name_of(&r.level)?, "number": r.number, "name": r.name, "outline": [a, [c[0], a[1]], c, [a[0], c[1]]], "sheet": "A-601" }))
        })
        .collect();
    let p = b.exec(job, "bim.prints", json!({ "prints": { "rooms": rooms } }))?;
    b.nonzero(p["rooms"].as_u64().unwrap_or(0) as usize, "rooms in the prints");
    Some(set)
}

/// The built model's elements per discipline (`architecture`, `structure`, `mep`, `electrical`,
/// `rooms`), read from the objects the builds tag, with the spec's electrical devices.
fn element_counts(job: &Session) -> BTreeMap<&'static str, usize> {
    let mut m: BTreeMap<&'static str, usize> = ["architecture", "structure", "mep", "electrical", "rooms"].into_iter().map(|k| (k, 0)).collect();
    let Ok(d) = job.doc() else { return m };
    for e in d.model.iter() {
        let x = &e.common.xdata;
        let Some(at) = x.iter().position(|t| t.code == 1001) else { continue };
        let app = x.get(at).map(|t| t.value.to_ascii_uppercase()).unwrap_or_default();
        let part = x.iter().skip(at + 1).find(|t| t.code == 1000).map(|t| t.value.to_ascii_lowercase()).unwrap_or_default();
        let layer = e.common.layer.to_ascii_uppercase();
        let disc = if app == "CADADDY_ROOM" {
            "rooms"
        } else if app != "CADADDY_WALLGEN" {
            continue;
        } else if ["column", "beam", "grid", "gridcolumn", "slab", "joist"].contains(&part.as_str()) {
            "structure"
        } else if part == "mep" && (layer.contains("CONDUIT") || layer.contains("TRAY")) || part == "fixture" || part == "device" {
            "electrical"
        } else if part == "mep" || part == "terminal" {
            "mep"
        } else {
            "architecture"
        };
        *m.entry(disc).or_default() += 1;
    }
    let devices = d.building_spec.as_deref().and_then(|s| cadaddy_walk3d::BuildingSpec::from_json(s).ok()).map_or(0, |sp| sp.mep.devices.len());
    *m.entry("electrical").or_default() += devices;
    m
}

/// The hotel's spec book as a spec book's text: a section per item, its type marks, maker and
/// model.
fn spec_book_text(t: &Truth) -> String {
    let mut s = String::new();
    for item in &t.spec_items {
        s.push_str(&format!("SECTION {} - {}\n\n", item.section, item.title));
        let marks: Vec<&str> =
            if item.referenced_by.is_empty() { vec![item.id.as_str()] } else { item.referenced_by.iter().map(String::as_str).collect() };
        for mark in marks {
            s.push_str(&format!("Type {mark}: {}\nManufacturer: {}\nModel: {}\n", item.title.to_lowercase(), item.manufacturer, item.model));
            if item.or_equal {
                s.push_str("Substitutions: or equal\n");
            }
            s.push('\n');
        }
    }
    s
}

/// One MCP tool call through the server: its time (ms), answer size (bytes of the answer's JSON,
/// as the caps count it) and whether it worked.
fn mcp_call(server: &mut cadaddy_mcp::Server, tool: &str, args: Value) -> (f64, usize, Result<Value, String>) {
    let line = json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": { "name": tool, "arguments": args } }).to_string();
    let t = Instant::now();
    let reply = server.handle_line(&line);
    let ms = t.elapsed().as_secs_f64() * 1000.0;
    let reply: Value = reply.and_then(|r| serde_json::from_str(&r).ok()).unwrap_or(Value::Null);
    let result = &reply["result"];
    let text = result["content"][0]["text"].as_str().unwrap_or_default();
    if reply.get("error").is_some() || result["isError"] == json!(true) || result.is_null() {
        let why = if text.is_empty() { reply["error"].to_string() } else { text.to_string() };
        return (ms, text.len(), Err(why.chars().take(300).collect()));
    }
    let answer: Value = serde_json::from_str(text).unwrap_or_else(|_| Value::String(text.to_string()));
    (ms, answer.to_string().len(), Ok(answer))
}

fn run_size(size_name: &str, size: HotelSize, seed: u64, scratch: &Path) -> Result<Bench, String> {
    let mut b = Bench::new(size_name);
    let dir = scratch.join(format!("hotel-{size_name}"));
    let _ = std::fs::remove_dir_all(&dir);
    let t = Instant::now();
    let set = cadaddy_mepfix::generate_hotel(seed, size)?;
    set.write_to(&dir).map_err(|e| e.to_string())?;
    let truth = &set.truth;
    let sheets: Vec<String> = truth.sheets.iter().map(|s| s.number.clone()).collect();
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
    b.stage("import", |b| {
        let opened = b.exec(&mut proj, "printset.open", json!({ "path": dir.to_string_lossy(), "name": "Sample Hotel" }));
        let n = opened.as_ref().and_then(|v| v["sheets"].as_u64()).unwrap_or(0) as usize;
        b.check(n >= sheets.len(), || format!("printset.open found {n} sheets of {}", sheets.len()));
        let c = b.exec(&mut proj, "printset.classify", json!({}));
        let n = c.as_ref().and_then(|v| v["classified"].as_u64()).unwrap_or(0) as usize;
        b.nonzero(n, "printset.classify sheets classified");
    });
    // Every plan PDF to .cdy, the architectural ones read as plans.
    let cdy_dir = dir.join("cdy");
    let _ = std::fs::create_dir_all(&cdy_dir);
    let (mut classify_ms, mut cdy_bytes, mut cdys, mut rooms_found) = (0.0, 0usize, 0usize, 0usize);
    b.stage("pdf_to_cdy", |b| {
        for n in &plans {
            let mut s = Session::new();
            if b.exec(&mut s, "pdfimport", json!({ "path": file(&dir, n, "pdf").to_string_lossy(), "layout": false, "images": false })).is_none() {
                continue;
            }
            if n.starts_with("A-1") {
                let t = Instant::now();
                let r = b.exec(&mut s, "plan.classify", json!({}));
                classify_ms += t.elapsed().as_secs_f64() * 1000.0;
                rooms_found += r.as_ref().map_or(0, |v| len(&v["rooms"]));
            }
            if let Ok(d) = s.doc()
                && let Ok(bytes) = cadaddy_io::write(d, "sheet.cdy")
            {
                cdy_bytes += bytes.len();
                cdys += 1;
                let _ = std::fs::write(cdy_dir.join(format!("{n}.cdy")), bytes);
            }
        }
        b.check(cdys == plans.len(), || format!("{cdys} of {} plans became .cdy", plans.len()));
        b.nonzero(rooms_found, "plan.classify rooms");
    });
    b.out.insert(format!("bim_classify@{size_name}"), round1(classify_ms));
    b.put("cdy_kb", (cdy_bytes as f64 / 1024.0).round());
    b.stage("open_dxf", |b| {
        let n = sheets.iter().filter(|n| opened(b, &file(&dir, n, "dxf")).is_some()).count();
        b.check(n == sheets.len(), || format!("{n} of {} DXF sheets opened", sheets.len()));
    });

    // The print readers (what turns the set into a model), on every sheet they read. Each
    // stage's key result is everything its reader found: the hotel has all of them.
    let dxf: Vec<(String, Vec<u8>)> = set.files.iter().filter(|(n, _)| n.ends_with(".dxf")).cloned().collect();
    let mut parsed: Vec<(String, cadaddy_engine::doc::Drawing)> = Vec::new();
    let map = b.stage("read_sheets", |b| {
        parsed = dxf.iter().filter_map(|(n, bytes)| cadaddy_io::read(bytes, n).ok().map(|d| (n.trim_end_matches(".dxf").to_string(), d))).collect();
        let map = cadaddy_mepfix::sheet_reader::recognise_fixture_set(&dxf);
        b.check(parsed.len() == dxf.len(), || format!("{} of {} DXF sheets parsed", parsed.len(), dxf.len()));
        b.nonzero(map.sheets.len(), "sheets recognised");
        map
    });
    let refs: Vec<(String, &cadaddy_engine::doc::Drawing)> = parsed.iter().map(|(n, d)| (n.clone(), d)).collect();
    let ceil = b.stage("read_ceilings", |b| {
        let c = cadaddy_mepfix::ceiling_reader::read_ceilings(&dxf, &map, None);
        b.nonzero(c.rooms.len() + c.ceilings.len(), "rooms and ceilings");
        c
    });
    let st = b.stage("read_struct", |b| {
        let s = cadaddy_mepfix::struct_reader::read_structure(&map, &refs);
        b.nonzero(s.columns.len(), "columns");
        s
    });
    let walls = b.stage("read_walls", |b| {
        let w = cadaddy_mepfix::wall_reader::read_walls(&dxf, &map, &ceil, &st);
        b.nonzero(w.walls.len() + w.types.len(), "walls and wall types");
        w
    });
    let arch_r = b.stage("read_arch", |b| {
        let a = cadaddy_mepfix::arch_reader::read_arch(&dxf, &map, &ceil);
        let n = a.doors.len() + a.windows.len() + a.stairs.len() + a.elevators.len() + a.rooms.len() + a.finish_codes.len() + a.door_rows.len();
        b.nonzero(n, "doors, windows, stairs, elevators, rooms and finishes");
        a
    });
    let asm = b.stage("read_assemblies", |b| {
        let a = cadaddy_mepfix::ceilasm_reader::read_assemblies(&dxf, &map, &ceil, &walls);
        b.nonzero(
            a.ceiling_types.len() + a.soffit_types.len() + a.rooms.len() + a.walls.len() + a.heads.len(),
            "ceiling, soffit and wall head assemblies",
        );
        a
    });
    let sheet_of = |n: &str| truth.sheets.iter().find(|s| s.number == n);
    let (runs, terminals) = b.stage("read_ducts", |b| {
        let (mut runs, mut terms) = (0, 0);
        for (n, d) in &parsed {
            if let (true, Some(sh)) = (n.starts_with("M-1"), sheet_of(n)) {
                let net = cadaddy_mepfix::duct_reader::read_ducts(d, sh);
                runs += net.runs.len();
                terms += net.terminals.len();
            }
        }
        b.nonzero(runs, "duct runs");
        (runs, terms)
    });
    let (pipes, risers) = b.stage("read_pipes", |b| {
        let (mut p, mut r) = (0, 0);
        for (n, d) in &parsed {
            if let (true, Some(sh)) = (n.starts_with("P-1"), sheet_of(n)) {
                let (found, rs) = cadaddy_mepfix::pipe_reader::read_pipes(d, sh);
                p += found.len();
                r += rs.len();
            }
        }
        b.nonzero(p, "pipe runs");
        (p, r)
    });
    let heads = b.stage("read_sprinklers", |b| {
        let h = cadaddy_mepfix::sprinkler_reader::read_sprinklers(&dxf, &map, &ceil);
        b.nonzero(h.heads.len(), "sprinkler heads");
        h
    });
    let (devices, panels) = b.stage("read_devices", |b| {
        let r = cadaddy_mepfix::elec_reader::read_devices(&dxf, &map, truth);
        b.nonzero(r.0.len(), "electrical devices");
        r
    });
    let numbers: Vec<String> = truth.sheets.iter().map(|s| s.number.clone()).collect();
    let (resolved, broken) = b.stage("read_callouts", |b| {
        let (res, brk, _errors) = cadaddy_mepfix::callout_reader::resolve_callouts(&dxf, &numbers);
        b.nonzero(res.len(), "resolved callouts");
        (res, brk)
    });
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

    // The whole job as one model: every level, every discipline.
    let lay = JobLayout::new(truth);
    let read = ReadIn {
        columns: st.columns.iter().map(|c| (c.grid.clone(), c.at_ft, c.footprint_in)).collect(),
        devices: devices.iter().map(|d| (d.level.clone(), d.kind.clone(), d.room.clone())).collect(),
        heads,
    };
    let mut job = Session::new();
    b.stage("compose", |b| {
        let n = compose(b, &mut job, &dir, truth, &lay);
        let want = truth
            .building
            .levels
            .iter()
            .map(|l| {
                JOB_SHEETS
                    .iter()
                    .filter(|(d, start, _)| {
                        truth.sheets.iter().any(|s| {
                            s.discipline == *d && s.level.as_deref() == Some(l.code.as_str()) && s.number.starts_with(start) && s.window_ft.is_none()
                        })
                    })
                    .count()
            })
            .sum::<usize>();
        b.check(n == want && n > 0, || format!("{n} of {want} level plans inserted"));
    });
    b.stage("spec", |b| {
        let r = job_spec(b, &mut job, truth, &lay, &read);
        let levels = r.as_ref().map_or(0, |v| len(&v["spec"]["levels"]));
        b.check(levels == truth.building.levels.len(), || format!("the spec has {levels} levels of {}", truth.building.levels.len()));
    });
    b.stage("mep_read", |b| {
        let r = b.exec(&mut job, "mep.read", json!({ "apply": true }));
        b.nonzero(r.as_ref().map_or(0, |v| len(&v["runs"])), "MEP runs read from the plans");
    });
    b.stage("build", |b| {
        let r = b.exec(&mut job, "bim.build", json!({ "discipline": "all" }));
        let built = r.as_ref().and_then(|v| v["built"].as_object()).map_or(0, serde_json::Map::len);
        b.check(built + 1 >= truth.building.levels.len(), || format!("bim.build built {built} levels of {}", truth.building.levels.len()));
    });
    b.stage("rooms_build", |b| {
        let r = b.exec(&mut job, "rooms.build", json!({}));
        b.nonzero(r.as_ref().and_then(|v| v["built"].as_u64()).unwrap_or(0) as usize, "room volumes");
    });
    let counts = element_counts(&job);
    let total: usize = counts.values().sum();
    for (disc, n) in &counts {
        let short = match *disc {
            "architecture" => "arch",
            "structure" => "struct",
            "electrical" => "elec",
            other => other,
        };
        b.put(&format!("elems_{short}"), *n as f64);
        b.current = "build".into();
        b.nonzero(*n, &format!("{disc} elements in the model"));
    }
    b.put("elems_total", total as f64);
    b.notes.push(format!("model: {total} elements {counts:?}"));

    // On the whole model.
    b.stage("validate", |b| {
        let v = b.exec(&mut job, "bspec.validate", json!({}));
        b.check(v.as_ref().is_some_and(|v| v["ok"].is_boolean()), || "bspec.validate gave no verdict".into());
        let i = b.exec(&mut job, "model.issues", json!({}));
        b.check(i.as_ref().is_some_and(|v| v["total"].is_u64()), || "model.issues gave no total".into());
    });
    b.stage("validate_full", |b| {
        let v = b.exec(&mut job, "bim.validate", json!({}));
        b.check(v.as_ref().is_some_and(|v| v["total"].is_u64() || v["checked"].is_object() || v["checked"].is_u64()), || {
            "bim.validate gave no result".into()
        });
    });
    b.stage("clashes", |b| {
        b.exec(&mut job, "model.clashes", json!({}));
    });
    b.stage("review", |b| {
        b.exec(&mut job, "review.list", json!({ "limit": 50 }));
        b.exec(&mut job, "rfi.list", json!({}));
    });
    b.stage("briefing", |b| {
        let r = b.exec(&mut job, "project.briefing", json!({}));
        let n = r.as_ref().map_or(0, |v| v.to_string().len());
        b.check(r.is_some() && n <= BRIEFING_MAX, || format!("project.briefing is {n} characters (cap {BRIEFING_MAX})"));
    });
    b.stage("takeoff", |b| {
        let r = b.exec(&mut job, "takeoff", json!({}));
        b.nonzero(r.as_ref().map_or(0, |v| len(&v["rows"])), "takeoff rows");
    });
    b.stage("export_ifc", |b| match job.doc().map(|d| cadaddy_io::ifc::write_ifc(d, "hotel.ifc")) {
        Ok(Ok(bytes)) => {
            let walls = String::from_utf8_lossy(&bytes).matches("IFCWALL").count();
            b.nonzero(walls, "walls in the IFC");
            b.put("ifc_kb", (bytes.len() as f64 / 1024.0).round());
        }
        Ok(Err(e)) => b.fail(format!("write_ifc: {e}")),
        Err(e) => b.fail(format!("no drawing: {e}")),
    });
    let bcf = dir.join("clashes.bcfzip");
    b.stage("export_bcf", |b| {
        b.exec(&mut job, "coordination.bcf", json!({ "path": bcf.to_string_lossy() }));
        b.check(bcf.exists(), || "no BCF file written".into());
    });
    let png = dir.join("render.png");
    b.stage("render", |b| {
        let r = b.exec(&mut job, "model.render", json!({ "path": png.to_string_lossy(), "width": 800, "height": 600 }));
        b.nonzero(r.as_ref().and_then(|v| v["triangles"].as_u64()).unwrap_or(0) as usize, "rendered triangles");
    });
    let cdy = dir.join("hotel-model.cdy");
    let objects = job.doc().map_or(0, |d| d.model.iter().count());
    b.stage("save_cdy", |b| {
        b.exec(&mut job, "saveas", json!({ "path": cdy.to_string_lossy() }));
    });
    b.stage("open_cdy", |b| {
        let back = opened(b, &cdy).map_or(0, |s| s.doc().map_or(0, |d| d.model.iter().count()));
        b.check(back == objects && back > 0, || format!("the saved model reopened with {back} objects of {objects}"));
    });
    // An agent's edit, then the incremental check it runs after it.
    let level = truth.building.levels.first().map(|l| l.name.clone()).unwrap_or_default();
    b.stage("edit_validate", |b| {
        b.exec(&mut job, "bspec.set", json!({ "spec": { "levels": [{ "name": level, "ceiling_ft": 9.5 }] } }));
        let handles: Vec<String> = job.doc().map(|d| d.model.iter().take(50).map(|e| e.handle.hex()).collect()).unwrap_or_default();
        b.exec(&mut job, "bim.validate", json!({ "handles": handles }));
        b.exec(&mut job, "model.issues", json!({}));
    });
    // The spec book into types, and the submittal log.
    let book = dir.join("spec-book.txt");
    let _ = std::fs::write(&book, spec_book_text(truth));
    b.stage("spec_intake", |b| {
        let r = b.exec(&mut job, "bim.spec_import", json!({ "file": book.to_string_lossy(), "format": "spec" }));
        let proposals = r.as_ref().map(|v| v["proposals"].clone()).unwrap_or(Value::Null);
        b.nonzero(len(&proposals), "types proposed from the spec book");
        if len(&proposals) > 0 {
            b.exec(&mut job, "bim.spec_accept", json!({ "proposals": proposals }));
        }
        for item in &truth.spec_items {
            b.exec(&mut job, "procure.submittal", json!({ "item": item.title, "spec_section": item.section, "status": "submitted" }));
        }
        let log = b.exec(&mut job, "procure.submittals", json!({}));
        let n = log.as_ref().map_or(0, |v| len(&v["submittals"]));
        b.check(n == truth.spec_items.len(), || format!("the submittal log has {n} entries of {}", truth.spec_items.len()));
    });
    // What the 3D viewer does on opening the model, then frames orbiting it.
    b.stage("viewer_open", |b| {
        let scene = job.doc().ok().and_then(|d| {
            let sp = cadaddy_walk3d::BuildingSpec::from_json(d.building_spec.as_deref()?).ok()?;
            Some(cadaddy_walk3d::derive(d, &sp.to_rules()))
        });
        let tris = scene.map_or(0, |s| s.tris.len() / 3);
        b.nonzero(tris, "viewer scene triangles");
        b.put("viewer_tris", tris as f64);
    });
    b.stage("viewer_orbit", |b| {
        for k in 0..ORBIT_FRAMES {
            let a = k as f64 / ORBIT_FRAMES as f64 * std::f64::consts::TAU;
            let r = b.exec(&mut job, "model.render", json!({ "path": dir.join(format!("orbit-{k}.png")).to_string_lossy(), "width": 640, "height": 480, "style": "shaded", "dir": [a.cos(), a.sin(), 0.6] }));
            b.nonzero(r.as_ref().and_then(|v| v["triangles"].as_u64()).unwrap_or(0) as usize, "orbit frame triangles");
        }
    });
    // A long agent session: does memory grow?
    if size_name == "full" {
        let mut start = 0.0;
        // Undo off: each spec edit's undo step (the spec before it) is kept by design, up to the
        // undo budget; what is left growing is what leaks.
        b.exec(&mut job, "undo", json!({ "mode": "control", "control": "none" }));
        b.stage("soak", |b| {
            for k in 0..SOAK_CYCLES {
                if k == SOAK_WARMUP {
                    start = rss_mb().unwrap_or(0.0);
                }
                let ceiling = if k % 2 == 0 { 9.0 } else { 9.5 };
                let ok = b.exec(&mut job, "bspec.set", json!({ "spec": { "levels": [{ "name": level, "ceiling_ft": ceiling }] } })).is_some()
                    && b.exec(&mut job, "bspec.validate", json!({})).is_some()
                    && b.exec(&mut job, "model.issues", json!({ "limit": 20 })).is_some();
                if !ok {
                    break;
                }
            }
        });
        let grown = rss_mb().map_or(0.0, |end| (end - start).max(0.0));
        b.put("leak_mb", round1(grown));
        b.exec(&mut job, "undo", json!({ "mode": "control", "control": "all" }));
    }
    let mut proj_dir = dir.join("project");
    proj_dir.push("Sample Hotel.cdyproj");
    // The agent tools through the MCP server, on the built model.
    let mut agent = cadaddy_mcp::Headless { session: std::mem::take(&mut job) };
    agent.session.agent.confirm = false;
    let mut server = cadaddy_mcp::Server::new(Box::new(agent));
    let _ =
        server.handle_line(&json!({ "jsonrpc": "2.0", "id": 0, "method": "initialize", "params": { "protocolVersion": "2025-06-18" } }).to_string());
    let _ = mcp_call(&mut server, "project", json!({ "action": "create", "path": proj_dir.to_string_lossy(), "name": "Sample Hotel" }));
    b.current = "mcp".into();
    let calls: [(&str, Value, usize); 5] = [
        ("execute", json!({ "command": "model.issues", "params": { "limit": 100 } }), ANSWER_MAX),
        ("bim_job", json!({ "action": "briefing" }), BRIEFING_MAX),
        ("project", json!({ "action": "info" }), ANSWER_MAX),
        ("mep", json!({ "action": "list" }), ANSWER_MAX),
        ("rooms", json!({ "action": "list" }), ANSWER_MAX),
    ];
    for (tool, args, cap) in calls {
        let (ms, bytes, r) = mcp_call(&mut server, tool, args);
        b.put(&format!("mcp_{tool}_ms"), round1(ms));
        b.put(&format!("mcp_{tool}_kb"), round1(bytes as f64 / 1024.0));
        match r {
            Ok(_) => b.check(bytes <= cap, || format!("MCP {tool}: a {bytes}-character answer, over the {cap} cap")),
            Err(e) => b.fail(format!("MCP {tool} failed: {e}")),
        }
    }
    // A revision: one sheet changes; only it should be redone.
    let changed = file(&dir, "M-102", "dxf");
    if let Ok(mut text) = std::fs::read_to_string(&changed) {
        text.push_str("999\nrevision 2\n");
        let _ = std::fs::write(&changed, text);
    }
    b.stage("revision", |b| {
        let r = b.exec(&mut proj, "printset.reimport", json!({ "path": dir.to_string_lossy(), "revision": "2" }));
        let n = r.as_ref().and_then(|v| v["delta"]["changed"].as_array().map(Vec::len).or_else(|| v["changed"].as_array().map(Vec::len)));
        b.check(revision_ok(n), || format!("{n:?} sheets reported changed, expected exactly 1"));
    });
    let wall = wall0.elapsed().as_secs_f64();
    if let (Some(c0), Some(c1)) = (cpu0, cpu_s())
        && wall > 0.0
    {
        b.put("cpu_cores", ((c1 - c0) / wall * 100.0).round() / 100.0);
    }
    let peak = round1(b.peak);
    b.put("peak_mb", peak);
    let resp = b.detail.values().map(|c| c.max_bytes).max().unwrap_or(0);
    b.put("resp_kb", round1(resp as f64 / 1024.0));
    let _ = std::fs::remove_dir_all(&dir);
    Ok(b)
}

fn main() {
    hooks();
    let args: Vec<String> = std::env::args().skip(1).collect();
    let arg = |k: &str| args.iter().position(|a| a == k).and_then(|i| args.get(i + 1)).cloned();
    let sizes = arg("--sizes").unwrap_or_else(|| "full".into());
    let seed = arg("--seed").and_then(|s| s.parse().ok()).unwrap_or(7);
    let scratch = std::env::temp_dir().join(format!("cadaddy-bimbench-{}", std::process::id()));
    let mut out = BTreeMap::new();
    let mut detail = serde_json::Map::new();
    let mut failures: Vec<String> = Vec::new();
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
        match run_size(name, size, seed, &scratch) {
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
                    .map(|(k, c)| (k.clone(), json!({ "calls": c.calls, "ms": round1(c.ms), "max_kb": (c.max_bytes as f64 / 102.4).round() / 10.0, "errors": c.errors })))
                    .collect();
                detail.insert(name.to_string(), Value::Object(d));
                println!("  peak {:.1} MB", b.peak);
                failures.extend(b.failures.iter().cloned());
                out.extend(b.out);
            }
            Err(e) => {
                eprintln!("{name}: {e}");
                std::process::exit(1);
            }
        }
    }
    if let (Some(full), Some(double)) = (out.get("elems_total@bim_full"), out.get("elems_total@bim_double"))
        && let Some(f) = scaling_failure(*full, *double)
    {
        failures.push(f);
    }
    let _ = std::fs::remove_dir_all(&scratch);
    if let Some(path) = arg("--out") {
        let _ = std::fs::write(&path, serde_json::to_string_pretty(&out).unwrap_or_default());
    }
    if let Some(path) = arg("--detail") {
        let _ = std::fs::write(&path, serde_json::to_string_pretty(&Value::Object(detail)).unwrap_or_default());
    }
    for f in &failures {
        eprintln!("FAILED {f}");
    }
    let code = exit_code(&failures);
    if code != 0 {
        eprintln!("bimbench: {} stage check(s) failed", failures.len());
        std::process::exit(code);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_erroring_command_fails_the_run() {
        let mut b = Bench::new("full");
        let mut s = Session::new();
        b.stage("validate", |b| {
            assert!(b.exec(&mut s, "bspec.validate", json!({})).is_some());
        });
        assert_eq!(b.exit_code(), 0, "{:?}", b.failures);
        b.stage("takeoff", |b| {
            assert!(b.exec(&mut s, "no.such.command", json!({})).is_none());
        });
        assert_eq!(b.exit_code(), 1);
        assert!(b.failures.iter().any(|f| f.starts_with("takeoff @ full: no.such.command failed")), "{:?}", b.failures);
        // A probe's error is an answer, not a failure.
        let mut p = Bench::new("full");
        assert!(p.run(&mut s, "no.such.command", json!({})).is_err());
        assert_eq!(p.exit_code(), 0);
    }

    #[test]
    fn an_empty_key_result_fails_the_run() {
        let mut b = Bench::new("double");
        b.stage("read_pipes", |b| b.nonzero(3, "pipe runs"));
        assert_eq!(b.exit_code(), 0);
        b.stage("read_ducts", |b| b.nonzero(0, "duct runs"));
        assert_eq!(b.exit_code(), 1);
        assert_eq!(b.failures, ["read_ducts @ double: duct runs: none found, the hotel has some"]);
    }

    #[test]
    fn the_revision_must_change_exactly_one_sheet() {
        assert!(revision_ok(Some(1)));
        for n in [None, Some(0), Some(2), Some(46)] {
            assert!(!revision_ok(n), "{n:?}");
        }
    }

    #[test]
    fn the_double_hotel_must_have_nearly_twice_the_elements() {
        assert_eq!(scaling_failure(1000.0, 1900.0), None);
        assert_eq!(scaling_failure(1000.0, 1800.0), None);
        assert!(scaling_failure(1000.0, 1100.0).is_some_and(|f| f.contains("1100")));
    }

    #[test]
    fn levels_sit_side_by_side_without_overlapping() {
        let t = cadaddy_mepfix::hotel_truth_for(7, HotelSize::Small);
        let lay = JobLayout::new(&t);
        for i in 0..t.building.levels.len() {
            let (a, b) = (lay.region(i), lay.region(i + 1));
            assert!(a[2] < b[0], "level {i}: {a:?} {b:?}");
            let p = lay.at(i, [1.0, 2.0]);
            assert!(p[0] > a[0] && p[0] < a[2] && p[1] > a[1] && p[1] < a[3]);
        }
    }

    #[test]
    fn the_spec_book_names_every_section() {
        let t = cadaddy_mepfix::hotel_truth_for(7, HotelSize::Small);
        let text = spec_book_text(&t);
        assert!(!t.spec_items.is_empty());
        for item in &t.spec_items {
            assert!(text.contains(&format!("SECTION {} - {}", item.section, item.title)), "{}", item.section);
            assert!(text.contains(&item.manufacturer));
        }
    }
}
