//! Q6019 devicesmodel: devices are real model elements. `mep.read_devices {preview: false}` puts
//! them in the open model as elements (never wiping the rest) and saves; takeoff device lines
//! count those elements.

use std::sync::{Arc, Mutex, MutexGuard};

use cadaddy_engine::Session;
use cadaddy_engine::cmd::file::{CdyHooks, CdyWrite, IoHooks, set_cdy_hooks, set_io};
use cadaddy_engine::cmd::wallgen::made_as;
use cadaddy_engine::doc::{
    Attrib, AttribModes, AttribProps, Block, Circle, Common, Drawing, Entity, EntityKind, HAlign, Handle, Insert, Line, Space, Text, VAlign,
};
use cadaddy_engine::geom::Vec3;
use cadaddy_mepfix::generate_hotel;
use cadaddy_mepfix::hotel::HotelSize;
use serde_json::{Value, json};

static SERIAL: Mutex<()> = Mutex::new(());

fn serial() -> MutexGuard<'static, ()> {
    SERIAL.lock().unwrap_or_else(|e| e.into_inner())
}

/// File reading and writing (with .cdy and PDF), as the apps install it.
fn hooks() {
    set_io(IoHooks {
        read: |b, n| cadaddy_io::read(b, n).map_err(|e| e.to_string()),
        write: |d, n| cadaddy_io::write(d, n).map_err(|e| e.to_string()),
        plot: None,
        read_pdf: Some(cadaddy_io::pdf_read::read_json),
        read_dwf: None,
        write_dwfx: None,
    });
    set_cdy_hooks(CdyHooks {
        save: |d, prev| {
            let prev = prev.and_then(|p| p.as_ref().downcast_ref::<cadaddy_io::cdy::SaveState>());
            let (out, state) = cadaddy_io::cdy::save(d, prev, &|| None)?;
            let out = match out {
                cadaddy_io::cdy::Output::Full(bytes) => CdyWrite::Full(bytes),
                cadaddy_io::cdy::Output::Append { from, bytes, index_at } => CdyWrite::Append { from, bytes, index_at },
            };
            Ok((out, Arc::new(state)))
        },
        seed: |d, bytes| cadaddy_io::cdy::seed(d, bytes).map(|s| Arc::new(s) as cadaddy_engine::cmd::file::CdyState),
    });
}

// ─── A made-up print set imported side by side ──────────────────────────────
//
// Sheets are 110' × 60' (inches). A-101 (level L1) at x −10', A-102 (level L2) at x 105', E-101
// (L1's power plan, the same size as A-101) at x 250', E-102 (L2's) at x 366'8". A-101 has two
// 20' × 10' rooms of 6" walls, A-102 one; the E sheets a fixture schedule (types A and B) with a
// sample symbol, and device symbols.

const A101: [f64; 4] = [-120.0, -120.0, 1200.0, 600.0];
const A102: [f64; 4] = [1260.0, -120.0, 2580.0, 600.0];
const E101: [f64; 4] = [3000.0, -120.0, 4320.0, 600.0];
const E102: [f64; 4] = [4400.0, -120.0, 5720.0, 600.0];

fn new_handle(d: &mut Drawing) -> Handle {
    let h = Handle(d.handseed.max(0x100));
    d.handseed = h.0 + 1;
    h
}

fn line(d: &mut Drawing, layer: &str, a: (f64, f64), b: (f64, f64)) -> Handle {
    let p = |(x, y): (f64, f64)| Vec3 { x: x * 12.0, y: y * 12.0, z: 0.0 };
    let h = new_handle(d);
    let mut e = Entity::new(h, EntityKind::Line(Line { a: p(a), b: p(b) }));
    e.common.layer = layer.into();
    d.model.push(e);
    h
}

/// A 20' × 10' room of 6" walls with its south-west corner at (x0, 0) feet.
fn room(d: &mut Drawing, x0: f64) {
    let o = [((0., 0.), (20., 0.)), ((20., 0.), (20., 10.)), ((20., 10.), (0., 10.)), ((0., 10.), (0., 0.))];
    let i = [((0.5, 0.5), (19.5, 0.5)), ((19.5, 0.5), (19.5, 9.5)), ((19.5, 9.5), (0.5, 9.5)), ((0.5, 9.5), (0.5, 0.5))];
    for ((ax, ay), (bx, by)) in o.into_iter().chain(i) {
        line(d, "A-WALL", (ax + x0, ay), (bx + x0, by));
    }
}

fn text(v: &str, x: f64, y: f64, h: f64) -> Text {
    Text {
        insert: Vec3::new(x, y, 0.0),
        align_pt: None,
        height: h,
        value: v.into(),
        rotation: 0.0,
        width_factor: 1.0,
        oblique: 0.0,
        style: "Standard".into(),
        halign: HAlign::default(),
        valign: VAlign::default(),
    }
}

/// An E sheet's import, its frame's left edge at `ox` (inches): a fixture schedule with a sample
/// symbol, and device symbols (`PDF-SYM-1` inserts with their mark in TAG) `dx` right of `ox`.
fn e_sheet(d: &mut Drawing, ox: f64, devices: &[(&str, f64, f64)]) {
    if !d.blocks.contains_key("PDF-SYM-1") {
        let mut b = Block::new("PDF-SYM-1");
        b.entities.push(Entity::new(new_handle(d), EntityKind::Circle(Circle { center: Vec3::ZERO, radius: 3.0 })));
        d.blocks.insert("PDF-SYM-1".into(), Arc::new(b));
    }
    let common = || Common { layer: "PDF".into(), ..Common::default() };
    let x = ox + 1000.0;
    for (v, dx, y, h) in [
        ("LIGHTING FIXTURE SCHEDULE", 0.0, 500.0, 8.0),
        ("TYPE", 0.0, 485.0, 5.0),
        ("DESCRIPTION", 30.0, 485.0, 5.0),
        ("A", 0.0, 472.0, 5.0),
        ("2X4 LED TROFFER", 30.0, 472.0, 5.0),
        ("B", 0.0, 460.0, 5.0),
        ("LED DOWNLIGHT", 30.0, 460.0, 5.0),
    ] {
        d.add(&Space::Model, common(), EntityKind::Text(text(v, x + dx, y, h))).expect("text");
    }
    // The schedule's sample symbol, then the plan's devices.
    let mut syms = vec![("A", x - 10.0, 472.0)];
    syms.extend(devices.iter().map(|(t, dx, y)| (*t, ox + dx, *y)));
    for (tag, x, y) in syms {
        let attrib = Attrib {
            tag: "TAG".into(),
            text: text(tag, x, y, 2.0),
            invisible: false,
            constant: false,
            prompt: String::new(),
            props: AttribProps::default(),
            multiline: None,
            modes: AttribModes::default(),
        };
        let ins = Insert {
            block: "PDF-SYM-1".into(),
            insert: Vec3::new(x, y, 0.0),
            scale: Vec3::new(1.0, 1.0, 1.0),
            rotation: 0.0,
            attribs: vec![attrib],
            cols: 1,
            rows: 1,
            col_spacing: 0.0,
            row_spacing: 0.0,
            clip: None,
        };
        d.add(&Space::Model, common(), EntityKind::Insert(ins)).expect("symbol");
    }
}

/// The set: L1 and L2 stacked (L2's plan moves 115' left onto L1's), built. E-101 has three
/// type A devices and one B (inside A-101's rooms once moved by the sheet frame); E-102 two A.
/// Also a stray wall stub far from the rooms (`stub`), to change the plan with.
struct Set {
    s: Session,
    stub: Handle,
}

fn set(path: Option<String>) -> Set {
    let mut d = Drawing::new_imperial();
    room(&mut d, 0.0);
    room(&mut d, 30.0);
    room(&mut d, 115.0);
    let stub = line(&mut d, "A-WALL", (70.0, 40.0), (75.0, 40.0));
    // Where a device `ft` right of its plan's first room is on its E sheet (the plans sit 120"
    // inside their frames): A at 5', 10' and 40' (L1's second room), B at 45'; on L2 A at 5', 12'.
    let at = |ft: f64| ft * 12.0 + 120.0;
    e_sheet(&mut d, E101[0], &[("A", at(5.0), 60.0), ("A", at(10.0), 60.0), ("A", at(40.0), 60.0), ("B", at(45.0), 96.0)]);
    e_sheet(&mut d, E102[0], &[("A", at(5.0), 60.0), ("A", at(12.0), 60.0)]);
    let mut s = Session::empty();
    let title = if path.is_some() { "Sample Floor Plan.cdy" } else { "Sample Floor Plan" };
    s.open_drawing(d, title, path);
    let sheet = |n: &str, dis: &str, lv: &str, r: [f64; 4]| json!({"number": n, "discipline": dis, "level": lv, "source": "model", "rect": r});
    let spec = json!({
        "version": 1, "units_per_foot": 12, "stack": true,
        "levels": [
            {"name": "L1", "elevation_ft": 0, "height_ft": 10, "ceiling_ft": 9, "region": A101},
            {"name": "L2", "elevation_ft": 10, "height_ft": 10, "ceiling_ft": 9, "region": A102}
        ],
        "layers": [{"layer": "A-WALL", "role": "wall", "wall_type": "Partition"}],
        "wall_types": [{"name": "Partition", "thickness_in": 6}],
        "sheets": [sheet("A-101", "A", "L1", A101), sheet("A-102", "A", "L2", A102), sheet("E-101", "E", "L1", E101), sheet("E-102", "E", "L2", E102)]
    });
    s.execute("bspec.set", &json!({"replace": true, "spec": spec})).expect("spec");
    s.execute("model.build", &json!({})).expect("build");
    Set { s, stub }
}

fn summary(s: &mut Session) -> Value {
    s.execute("bim.props.summary", &json!({})).expect("summary")["kinds"].clone()
}

fn count(k: &Value, kind: &str) -> u64 {
    k[kind]["count"].as_u64().unwrap_or(0)
}

/// Every built object that isn't a device: (part, handle) sorted.
fn not_devices(s: &Session) -> Vec<(String, Handle)> {
    let d = s.doc().expect("doc");
    let mut v: Vec<(String, Handle)> = d.model.iter().filter_map(|e| made_as(e).filter(|p| p != "device").map(|p| (p, e.handle))).collect();
    v.sort();
    v
}

/// The built device elements' plan centres (feet) and level tags.
fn device_spots(s: &Session) -> Vec<(f64, f64, f64)> {
    let d = s.doc().expect("doc");
    let mut v: Vec<(f64, f64, f64)> = d
        .model
        .iter()
        .filter(|e| made_as(e).as_deref() == Some("device"))
        .filter_map(|e| match &e.kind {
            EntityKind::Mesh(m) if !m.vertices.is_empty() => {
                let n = m.vertices.len() as f64;
                let (x, y, z) = m.vertices.iter().fold((0.0, 0.0, 0.0), |a, p| (a.0 + p.x, a.1 + p.y, a.2 + p.z));
                Some((x / n / 12.0, y / n / 12.0, z / n / 12.0))
            }
            _ => None,
        })
        .collect();
    v.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.total_cmp(&b.1)));
    v
}

fn read(s: &mut Session, p: Value) -> Value {
    s.execute("mep.read_devices", &p).expect("read")["model"].clone()
}

#[test]
fn read_devices_puts_device_elements_in_the_model_and_keeps_everything_else() {
    let _g = serial();
    let Set { mut s, .. } = set(None);
    let built = not_devices(&s);
    assert!(built.iter().filter(|(p, _)| p == "wall").count() >= 8, "walls built: {built:?}");
    assert_eq!(count(&summary(&mut s), "device"), 0, "no device read yet");
    // A preview changes nothing.
    let m = read(&mut s, json!({}));
    assert_eq!((&m["inserted"], &m["would_insert"]), (&json!(0), &json!(6)), "{m:#}");
    assert_eq!(count(&summary(&mut s), "device"), 0);

    let m = read(&mut s, json!({"preview": false}));
    assert_eq!((&m["inserted"], &m["elements"], &m["elements_replaced"]), (&json!(6), &json!(6), &json!(0)), "{m:#}");
    assert_eq!(m["by_level"], json!({"L1": 4, "L2": 2}), "each device on its sheet's level: {m:#}");
    assert_eq!(m["not_saved"].as_str().map(|t| t.contains("no file")), Some(true), "{m:#}");
    let k = summary(&mut s);
    assert_eq!(count(&k, "device"), 6, "{:#}", k["device"]);
    for key in ["type_mark", "sheet", "level", "location", "mount_height"] {
        assert_eq!(k["device"]["filled"][key], json!(6), "{key}: {:#}", k["device"]);
    }
    // B is 1'6" from the north wall's inside face: on it (the others stand off their walls).
    assert_eq!(k["device"]["filled"]["host_wall"], json!(1), "{:#}", k["device"]);
    assert_eq!(not_devices(&s), built, "nothing else in the model changed");

    // Moved onto the architectural plans by the sheet frames, L2's then stacked onto L1's like
    // its walls: all six over the two L1 rooms (x 0–20' and 30–50'), L2's a storey up.
    let spots = device_spots(&s);
    assert_eq!(spots.len(), 6);
    for (x, y, z) in &spots {
        assert!((0.0..=50.0).contains(x) && (0.0..=10.0).contains(y), "device over a room: {spots:?}");
        assert!(*z > 0.0 && *z < 20.0, "{spots:?}");
    }
    let up: Vec<_> = spots.iter().filter(|p| p.2 > 10.0).collect();
    assert_eq!(up.len(), 2, "two L2 devices a storey up: {spots:?}");

    // Read again: the same six elements, replaced, not twelve; the model is untouched.
    let m = read(&mut s, json!({"preview": false}));
    assert_eq!((&m["inserted"], &m["replaced"], &m["elements"], &m["elements_replaced"]), (&json!(6), &json!(6), &json!(6), &json!(6)), "{m:#}");
    assert_eq!(count(&summary(&mut s), "device"), 6);
    assert_eq!(device_spots(&s), spots);
    assert_eq!(not_devices(&s), built);

    // One undo takes the read back out.
    s.execute("undo", &json!({})).expect("undo");
    assert_eq!(count(&summary(&mut s), "device"), 6, "the first read's elements are back");
}

#[test]
fn takeoff_device_lines_count_the_device_elements() {
    let _g = serial();
    let Set { mut s, .. } = set(None);
    read(&mut s, json!({"preview": false}));
    let t = s.execute("bim.takeoff", &json!({"filter": {"category": "devices"}, "keep": false})).expect("takeoff");
    let lines = t["lines"].as_array().expect("lines");
    assert!(!lines.is_empty(), "{t:#}");
    let mut total = 0;
    for l in lines {
        let q = l["qty"].as_f64().unwrap_or(-1.0);
        assert_eq!(l["element_count"].as_f64(), Some(q), "element_count = quantity: {l:#}");
        total += l["element_count"].as_u64().unwrap_or(0);
    }
    assert_eq!(total, 6, "{t:#}");
    // Each line's elements are its devices: type A on L1 is three of them.
    let a1 = lines.iter().find(|l| l["level"] == "L1" && l["item"].as_str().is_some_and(|i| i.ends_with(" A"))).expect("type A on L1");
    assert_eq!(a1["element_count"], json!(3), "{a1:#}");
    let d = s.doc().expect("doc");
    for h in a1["elements"].as_array().expect("elements") {
        let e = h.as_str().and_then(Handle::parse_hex).and_then(|h| d.entity(h)).expect("element in the model");
        assert_eq!(made_as(e).as_deref(), Some("device"));
    }
}

#[test]
fn a_rebuild_after_a_plan_edit_keeps_the_read_devices() {
    let _g = serial();
    let Set { mut s, stub } = set(None);
    read(&mut s, json!({"preview": false}));
    let spots = device_spots(&s);
    // Erasing a plan wall rebuilds the model (associative): the devices come back the same.
    s.execute("erase", &json!({"handles": [stub.hex()]})).expect("erase");
    assert_eq!(count(&summary(&mut s), "device"), 6);
    assert_eq!(device_spots(&s), spots);
    let m = s.execute("model.build", &json!({})).expect("build");
    assert_eq!(m["deviceElements"], json!(6), "{m:#}");
    assert_eq!(device_spots(&s), spots);
}

#[test]
fn read_devices_saves_the_drawing_with_its_device_elements() {
    let _g = serial();
    hooks();
    let dir = std::env::temp_dir().join(format!("cadaddy-devicesmodel-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("dir");
    let path = dir.join("Sample Floor Plan.cdy").display().to_string();
    let Set { mut s, .. } = set(Some(path.clone()));
    let m = read(&mut s, json!({"preview": false}));
    assert_eq!(m["saved"], json!(path), "{m:#}");
    let mut again = Session::empty();
    again.execute("open", &json!({"path": path})).expect("open");
    let k = summary(&mut again);
    assert_eq!(count(&k, "device"), 6, "the saved file has the device elements: {:#}", k["device"]);
    assert_eq!(k["device"]["filled"]["location"], json!(6));
    // save: false leaves the file alone.
    let before = std::fs::metadata(&path).and_then(|m| m.modified()).expect("mtime");
    std::thread::sleep(std::time::Duration::from_millis(1100));
    let m = read(&mut s, json!({"preview": false, "save": false}));
    assert_eq!(m["saved"], json!(false));
    assert_eq!(std::fs::metadata(&path).and_then(|m| m.modified()).expect("mtime"), before);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn hostile_or_empty_reads_never_fail_or_wipe() {
    let _g = serial();
    // No drawing model at all: still the old answer, nothing built.
    let mut s = Session::empty();
    s.open_drawing(Drawing::new_imperial(), "Empty", None);
    let m = read(&mut s, json!({"preview": false}));
    assert_eq!(m["inserted"], json!(0), "{m:#}");
    for p in [json!({"preview": false, "save": "yes"}), json!({"preview": false, "limit": -3}), json!({"preview": "no"})] {
        let _ = s.execute("mep.read_devices", &p);
    }
    // A built model with no devices on its sheets: nothing inserted, nothing removed.
    let mut d = Drawing::new_imperial();
    room(&mut d, 0.0);
    let mut s = Session::empty();
    s.open_drawing(d, "Plan", None);
    s.execute(
        "bspec.set",
        &json!({"replace": true, "spec": {"version": 1, "units_per_foot": 12, "levels": [{"name": "L1", "elevation_ft": 0, "height_ft": 10}],
        "layers": [{"layer": "A-WALL", "role": "wall"}]}}),
    )
    .expect("spec");
    s.execute("model.build", &json!({})).expect("build");
    let built = not_devices(&s);
    let m = read(&mut s, json!({"preview": false}));
    assert_eq!(m["inserted"], json!(0), "{m:#}");
    assert_eq!(not_devices(&s), built);
}

// ─── The guided job ─────────────────────────────────────────────────────────

/// A made-up print set as one merged PDF holding `only`'s sheets.
fn job_folder(tag: &str, only: &[&str]) -> std::path::PathBuf {
    let mut set = generate_hotel(42, HotelSize::Small).expect("generate hotel");
    set.truth.sheets.retain(|s| only.contains(&s.number.as_str()));
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos());
    let dir = std::env::temp_dir().join(format!("cadaddy_devicesmodel_{tag}_{}_{nanos}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("folder");
    let pages: Vec<(Vec<u8>, String, String)> = set
        .truth
        .sheets
        .iter()
        .map(|sh| (set.file(&sh.pdf).expect("sheet pdf").to_vec(), format!("{} {}", sh.number, sh.title), String::new()))
        .collect();
    std::fs::write(dir.join("Full Set.pdf"), cadaddy_io::pdf_merge::merge(&pages).expect("merge")).expect("write set");
    dir
}

/// The guided job's devices are elements, its takeoff device lines count them, and reading the
/// devices again on the job's drawing (the round 4 failure) keeps the model and its elements.
#[test]
fn the_guided_jobs_devices_are_elements_its_takeoff_counts() {
    let _g = serial();
    hooks();
    let dir = job_folder("job", &["A-101", "E-201"]);
    let mut s = Session::new();
    let st =
        s.execute("job.start", &json!({"pdfs": dir.display().to_string(), "goal": "takeoff", "auto": true, "checkpoint": false})).expect("job.start");
    assert_eq!(st["done"], json!(true), "{st}");
    let k = summary(&mut s);
    let devices = count(&k, "device");
    assert!(devices > 0, "the job's devices are elements: {k:#}");
    assert_eq!(k["device"]["filled"]["location"], json!(devices), "{:#}", k["device"]);
    let t = s.execute("bim.takeoff", &json!({"filter": {"category": "devices"}, "keep": false})).expect("takeoff");
    let lines = t["lines"].as_array().expect("lines");
    let mut counted = 0;
    for l in lines {
        assert_eq!(l["element_count"].as_f64(), l["qty"].as_f64(), "element_count = quantity: {l:#}");
        counted += l["element_count"].as_u64().unwrap_or(0);
    }
    assert_eq!(counted, devices, "every device element on a takeoff line: {t:#}");

    let built = not_devices(&s);
    let r = s.execute("mep.read_devices", &json!({"preview": false})).expect("read");
    let m = &r["model"];
    assert!(m["why_not"].is_null(), "{m:#}");
    assert_eq!(not_devices(&s), built, "the model is not wiped");
    let k = summary(&mut s);
    assert_eq!(json!(count(&k, "device")), m["elements"], "{m:#}");
    assert!(count(&k, "device") > 0, "{m:#}");
    let _ = std::fs::remove_dir_all(&dir);
}

/// A device's type is what the legend or fixture schedule says its mark is, not the letter: the
/// schedule's A and B are luminaires, so the six devices are light fixtures A and B, each with
/// the schedule's description; the read counts them by type and by the job's classes (Q6002).
#[test]
fn read_devices_types_each_mark_by_its_legend_row() {
    let _g = serial();
    let Set { mut s, .. } = set(None);
    let m = read(&mut s, json!({"preview": false}));
    assert_eq!(m["by_device_type"], json!({"light fixture A": 5, "light fixture B": 1}), "{m:#}");
    assert_eq!(m["by_class"], json!({"light": 6}), "{m:#}");
    assert_eq!(m["legend_types"]["A"]["legend"], "2X4 LED TROFFER", "{m:#}");
    let spec = s.execute("bspec.get", &json!({})).expect("spec")["spec"].clone();
    let a = spec["tags"].as_array().expect("tags").iter().find(|t| t["fields"]["type"]["value"] == "A").cloned().expect("a type A device");
    assert_eq!(a["fields"]["device_type"], json!({"value": "light fixture A", "source": "legend"}), "{a:#}");
    let hs = s.execute("bim.props.find", &json!({"kind": "device", "where": {"type_mark": "B"}})).expect("find")["handles"].clone();
    let h = hs[0].as_str().expect("a B device");
    let p = s.execute("bim.props.get", &json!({"handle": h})).expect("get")["elements"][0]["props"].clone();
    assert_eq!(p["type"]["value"], "light fixture B", "{p:#}");
    assert_eq!(p["type"]["source"]["kind"], "legend");
    assert_eq!(p["description"]["value"], "LED DOWNLIGHT");
}
