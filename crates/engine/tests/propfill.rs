//! Q6002 propfill: the guided job's readers fill the elements' properties.

use std::sync::{Mutex, MutexGuard};

use cadaddy_engine::Session;
use cadaddy_engine::cmd::propfill::{cfm_in, door_number, mount_height_in, panel_of, split_switch_leg};
use cadaddy_engine::cmd::wallgen::made_as;
use cadaddy_engine::doc::{Drawing, Entity, EntityKind, Handle, Line, RawTag};
use cadaddy_engine::geom::{Vec2, Vec3};
use cadaddy_mepfix::generate_hotel;
use cadaddy_mepfix::hotel::HotelSize;
use serde_json::{Value, json};

fn real_pdf_reader() {
    cadaddy_engine::cmd::file::set_io(cadaddy_engine::cmd::file::IoHooks {
        read: |b, n| cadaddy_io::read(b, n).map_err(|e| e.to_string()),
        write: |d, n| cadaddy_io::write(d, n).map_err(|e| e.to_string()),
        plot: None,
        read_pdf: Some(cadaddy_io::pdf_read::read_json),
        read_dwf: None,
        write_dwfx: None,
    });
}

static SERIAL: Mutex<()> = Mutex::new(());

fn serial() -> MutexGuard<'static, ()> {
    SERIAL.lock().unwrap_or_else(|e| e.into_inner())
}

/// A made-up print set as one merged PDF holding `only`'s sheets.
fn job_folder(tag: &str, only: &[&str]) -> std::path::PathBuf {
    let mut set = generate_hotel(42, HotelSize::Small).expect("generate hotel");
    set.truth.sheets.retain(|s| only.contains(&s.number.as_str()));
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos());
    let dir = std::env::temp_dir().join(format!("cadaddy_propfill_{tag}_{}_{nanos}", std::process::id()));
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

fn run_job(tag: &str, only: &[&str]) -> Session {
    let dir = job_folder(tag, only);
    let mut s = Session::new();
    let st =
        s.execute("job.start", &json!({"pdfs": dir.display().to_string(), "goal": "takeoff", "auto": true, "checkpoint": false})).expect("job.start");
    assert_eq!(st["done"], json!(true), "{st}");
    s
}

// ─── A made-up two-room plan ────────────────────────────────────────────────

fn line(d: &mut Drawing, layer: &str, a: (f64, f64), b: (f64, f64)) {
    let p = |(x, y): (f64, f64)| Vec3 { x: x * 12.0, y: y * 12.0, z: 0.0 };
    let h = Handle(d.handseed.max(0x100));
    d.handseed = h.0 + 1;
    let mut e = Entity::new(h, EntityKind::Line(Line { a: p(a), b: p(b) }));
    e.common.layer = layer.into();
    d.model.push(e);
}

/// A 20' × 10' room of 6" walls with its south-west corner at (x0, 0) feet.
fn room(d: &mut Drawing, x0: f64) {
    let o = [((0., 0.), (20., 0.)), ((20., 0.), (20., 10.)), ((20., 10.), (0., 10.)), ((0., 10.), (0., 0.))];
    let i = [((0.5, 0.5), (19.5, 0.5)), ((19.5, 0.5), (19.5, 9.5)), ((19.5, 9.5), (0.5, 9.5)), ((0.5, 9.5), (0.5, 0.5))];
    for ((ax, ay), (bx, by)) in o.into_iter().chain(i) {
        line(d, "A-WALL", (ax + x0, ay), (bx + x0, by));
    }
}

/// A device the readers put in the spec: `device:` id, type mark, symbol, sheet, circuit.
fn device(x: f64, y: f64, mark: &str, circuit: &str, symbol: &str) -> Value {
    let mut fields = json!({
        "type": {"value": mark, "source": "plan"},
        "device_type": {"value": format!("device type {mark}"), "source": "plan"},
        "sheet": {"value": "E-101", "source": "plan"},
        "symbol": {"value": symbol, "source": "plan"},
    });
    if !circuit.is_empty() {
        fields["circuit"] = json!({"value": circuit, "source": "plan"});
    }
    json!({"id": format!("device:L1:{x:.1},{y:.1}"), "kind": "device", "level": "L1", "at_ft": [x, y], "fields": fields})
}

fn text(s: &mut Session, layer: &str, at: [f64; 2], t: &str) {
    if s.execute("layer.new", &json!({"name": layer, "current": true})).is_err() {
        s.execute("layer.current", &json!({"name": layer})).expect("layer");
    }
    s.execute("text", &json!({"at": at, "text": t, "height": 6})).expect("text");
}

/// Offices 101 (x 0–20') and 102 (x 30–50'); a device legend and a general note; four devices,
/// one duct run with a diffuser, an air handler; built and filled.
fn plan() -> Session {
    plan_with(false)
}

/// [`plan`]; `on_e_sheet`: the set's sheets side by side (the plan on A-101, an electrical plan
/// E-101 of the same size to its right) and one more device read on E-101.
fn plan_with(on_e_sheet: bool) -> Session {
    let mut d = Drawing::new_imperial();
    room(&mut d, 0.0);
    room(&mut d, 30.0);
    let mut s = Session::empty();
    s.open_drawing(d, "Sample Floor Plan", None);
    text(&mut s, "A-ROOM-TAG", [110.0, 64.0], "OFFICE");
    text(&mut s, "A-ROOM-TAG", [116.0, 52.0], "101");
    text(&mut s, "A-ROOM-TAG", [470.0, 64.0], "STORAGE");
    text(&mut s, "A-ROOM-TAG", [476.0, 52.0], "102");
    // The legend: a mark and its description with the height on one line.
    text(&mut s, "E-ANNO", [900.0, 300.0], "A");
    text(&mut s, "E-ANNO", [940.0, 300.0], "DUPLEX RECEPTACLE, +20\" AFF");
    // A general note for every fire alarm device.
    text(&mut s, "E-ANNO", [900.0, 200.0], "MOUNT ALL FIRE ALARM HORN/STROBES AT 80\" AFF");
    // The airflow beside the diffuser.
    text(&mut s, "M-ANNO", [1500.0, 528.0], "150 CFM");
    let spec = json!({
        "version": 1, "units_per_foot": 12,
        "levels": [{"name": "L1", "elevation_ft": 0, "height_ft": 10, "ceiling_ft": 9, "base": [0, 0]}],
        "layers": [{"layer": "A-WALL", "role": "wall", "wall_type": "Partition"}, {"layer": "A-ROOM-TAG", "role": "room_tag"}],
        "wall_types": [{"name": "Partition", "thickness_in": 6}],
        // Office 101's south wall: its type tagged on the plan.
        "walls": [{"level": "L1", "a": [0, 3], "b": [240, 3], "wall_type": "Partition", "source": "A-101"}],
        "tags": [
            device(5.0, 0.8, "A", "LP1-3", "1F0"),
            device(10.0, 9.2, "S", "12", "1F1"),
            device(35.0, 5.0, "FA", "", "1F2"),
            device(45.0, 9.2, "J", "", "1F3"),
            // Not read off a print (no symbol): not built.
            {"id": "device:X:0", "kind": "receptacle", "level": "L1", "at_ft": [0, 0]}
        ],
        "mep": {
            "systems": [{"name": "SA", "layer": "M-DUCT", "kind": "duct", "size_in": [12, 8], "bottom_ft": 9}],
            "runs": [{"system": "SA", "level": "L1", "points": [[1200, 540], [1800, 540]], "size_in": [12, 8], "bottoms_ft": [8.5],
                      "id": "R1", "size_source": "tag", "height_source": "estimated-route", "source_sheet": "M-101"}],
            "terminals": [{"system": "SA", "level": "L1", "at": [1500, 500], "size_in": [12, 12], "bottom_ft": 8, "terminal_kind": "diffuser"}],
            "equipment": [{"tag": "AHU-1", "kind": "ahu", "level": "L1", "min": [2000, 400], "max": [2060, 440], "height_ft": 5,
                           "properties": {"CFM": "2000"}, "sources": {"schedule": "M-601"}},
                          {"tag": "LP1", "kind": "panelboard", "level": "L1", "min": [2200, 400], "max": [2220, 410], "height_ft": 6,
                           "circuits": [{"number": "3", "description": "RECEPTACLES OFFICE 101", "breaker": "20A", "poles": 1, "source": "E-601"}]}]
        }
    });
    let mut spec = spec;
    if on_e_sheet {
        spec["levels"][0]["region"] = json!([-120, -120, 1200, 600]);
        spec["sheets"] = json!([
            {"number": "A-101", "discipline": "A", "level": "L1", "source": "model", "rect": [-120, -120, 1200, 600]},
            {"number": "E-101", "discipline": "E", "level": "L1", "source": "model", "rect": [3000, -120, 4320, 600]}
        ]);
        // At 15', 5' on the plan: 3300", 60" on E-101.
        let mut b = device(275.0, 5.0, "B", "LP1-5", "1F4");
        b["fields"]["sheet"]["value"] = json!("E-101");
        if let Some(t) = spec["tags"].as_array_mut() {
            t.push(b);
        }
    }
    s.execute("bspec.set", &json!({"replace": true, "spec": spec})).expect("spec");
    s.execute("model.build", &json!({})).expect("build");
    s.execute("rooms.build", &json!({})).expect("rooms");
    s
}

fn fill(s: &mut Session) -> Value {
    s.execute("bim.props.fill", &json!({})).expect("fill")
}

fn find(s: &mut Session, kind: &str, wh: Value) -> Vec<String> {
    let r = s.execute("bim.props.find", &json!({"kind": kind, "where": wh, "limit": 1000})).expect("find");
    r["handles"].as_array().expect("handles").iter().filter_map(|h| h.as_str().map(str::to_string)).collect()
}

fn get(s: &mut Session, h: &str) -> Value {
    s.execute("bim.props.get", &json!({"handle": h})).expect("get")["elements"][0]["props"].clone()
}

/// The device whose type mark is `mark`.
fn device_of(s: &mut Session, mark: &str) -> Value {
    let hs = find(s, "device", json!({"type_mark": mark}));
    assert_eq!(hs.len(), 1, "one device {mark}: {hs:?}");
    get(s, &hs[0])
}

#[test]
fn read_devices_become_elements_with_circuit_panel_room_wall_height_and_location() {
    let mut s = plan();
    let r = fill(&mut s);
    assert_eq!(r["devices"], json!(4), "four read devices, not the unread tag: {r:#}");
    let sum = s.execute("bim.props.summary", &json!({"kind": "device"})).expect("summary");
    let dev = &sum["kinds"]["device"];
    assert_eq!(dev["count"], json!(4), "{sum:#}");
    for key in ["type", "type_mark", "mount_height", "sheet", "location", "level", "room"] {
        assert_eq!(dev["filled"][key], json!(4), "{key}: {dev:#}");
    }
    let a = device_of(&mut s, "A");
    assert_eq!(a["circuit"]["value"], "LP1-3");
    assert_eq!(a["circuit"]["source"]["kind"], "print");
    assert_eq!(a["circuit"]["source"]["doc"], "E-101");
    assert_eq!(a["panel"]["value"], "LP1");
    assert_eq!(a["fed_from"]["value"], "panel LP1, circuit LP1-3");
    let panel = find(&mut s, "equipment", json!({"type_mark": "LP1"}));
    assert_eq!(a["fed_from_handle"]["value"], json!(panel[0]), "the panel as an element: {a:#}");
    assert_eq!(a["breaker"]["value"], "20A");
    assert_eq!(a["breaker"]["source"]["kind"], "schedule");
    assert_eq!(a["breaker"]["source"]["doc"], "E-601");
    assert_eq!(a["circuit_description"]["value"], "RECEPTACLES OFFICE 101");
    assert_eq!(a["room"]["value"], "101");
    assert_eq!(a["room"]["source"]["kind"], "computed");
    assert!(a["host_wall"]["value"].as_str().is_some_and(|h| !h.is_empty()), "on the south wall: {a:#}");
    // Read at 5', 0.8' on the plan: set in the south wall, its face flush with the wall's inside
    // face (0.5'; its box's middle 1" behind it), 20" up.
    assert_eq!(a["location"]["value"], "5.00, 0.42, 1.67", "{a:#}");
    assert_eq!(a["location"]["unit"], "ft");
    // A bare circuit number names no panel.
    let sw = device_of(&mut s, "S");
    assert_eq!(sw["circuit"]["value"], "12");
    assert!(sw["panel"].is_null(), "{sw:#}");
    assert_eq!(sw["fed_from"]["value"], "circuit 12");
    assert_eq!(device_of(&mut s, "FA")["room"]["value"], "102");
    // No circuit printed: none presented.
    let j = device_of(&mut s, "J");
    assert!(j["circuit"].is_null() && j["fed_from"].is_null(), "{j:#}");
}

#[test]
fn a_legend_row_of_another_kind_of_device_is_not_a_switchs_height() {
    let mut s = plan();
    // The legend's S row is a speaker/strobe's (a fire alarm sheet's mark), not the switch's.
    text(&mut s, "E-ANNO", [900.0, 250.0], "S");
    text(&mut s, "E-ANNO", [940.0, 250.0], "SPEAKER/STROBE +80\" AFF");
    s.execute("model.build", &json!({})).expect("build");
    let sw = device_of(&mut s, "S");
    assert_eq!(sw["mount_height"]["value"], json!(48.0), "{sw:#}");
    assert_eq!(sw["mount_height"]["source"]["kind"], "default");
}

#[test]
fn a_luminaire_hangs_at_the_ceiling_over_it() {
    let mut s = plan();
    // A luminaire by its submittal, in office 101 under its 9' ceiling; no height printed.
    let mut spec = s.execute("bspec.get", &json!({})).expect("spec")["spec"].clone();
    if let Some(t) = spec["tags"].as_array_mut() {
        let mut lum = device(8.0, 5.0, "Q7", "", "1FA");
        lum["fields"]["mount"] = json!({"value": "ceiling", "source": "submittal"});
        t.push(lum);
    }
    s.execute("bspec.set", &json!({"replace": true, "spec": spec})).expect("spec");
    s.execute("model.build", &json!({})).expect("build");
    let q7 = device_of(&mut s, "Q7");
    assert_eq!(q7["mount_height"]["value"], json!(108.0), "at the 9' ceiling: {q7:#}");
    assert_eq!(q7["mount_height"]["source"]["kind"], "rule");
    // The legend's A (a receptacle at +20") is no luminaire's height: an A by its submittal hangs
    // at the ceiling all the same.
    let mut spec = s.execute("bspec.get", &json!({})).expect("spec")["spec"].clone();
    if let Some(t) = spec["tags"].as_array_mut() {
        let mut lum = device(12.0, 5.0, "A", "", "1FB");
        lum["fields"]["mount"] = json!({"value": "ceiling", "source": "submittal"});
        t.push(lum);
    }
    s.execute("bspec.set", &json!({"replace": true, "spec": spec})).expect("spec");
    s.execute("model.build", &json!({})).expect("build");
    // An exit sign by its submittal: on the wall at 7'-6".
    let mut spec = s.execute("bspec.get", &json!({})).expect("spec")["spec"].clone();
    if let Some(t) = spec["tags"].as_array_mut() {
        let mut exit = device(15.0, 9.2, "XA", "", "1FC");
        exit["fields"]["mount"] = json!({"value": "wall", "source": "submittal"});
        t.push(exit);
    }
    s.execute("bspec.set", &json!({"replace": true, "spec": spec})).expect("spec");
    s.execute("model.build", &json!({})).expect("build");
    let xa = device_of(&mut s, "XA");
    assert_eq!(xa["mount_height"]["value"], json!(90.0), "{xa:#}");
    assert_eq!(xa["mount_height"]["source"]["kind"], "rule");
    let hs = find(&mut s, "device", json!({"type_mark": "A"}));
    let heights: Vec<Value> = hs.iter().map(|h| get(&mut s, h)["mount_height"]["value"].clone()).collect();
    assert!(heights.contains(&json!(20.0)) && heights.contains(&json!(108.0)), "the receptacle at +20\", the luminaire at the ceiling: {heights:?}");
}

#[test]
fn a_switch_is_on_the_wall_it_stands_by() {
    let mut s = plan();
    // A dimmer read 2.5' off office 101's north wall (at its designation, beside the symbol), and
    // a junction box as far off the south wall: the switch is on its wall, the box isn't.
    let mut spec = s.execute("bspec.get", &json!({})).expect("spec")["spec"].clone();
    if let Some(t) = spec["tags"].as_array_mut() {
        t.push(device(10.0, 7.0, "SD", "", "1F8"));
        t.push(device(15.0, 3.0, "JB", "", "1F9"));
    }
    s.execute("bspec.set", &json!({"replace": true, "spec": spec})).expect("spec");
    s.execute("model.build", &json!({})).expect("build");
    fill(&mut s);
    let sd = device_of(&mut s, "SD");
    assert!(sd["host_wall"]["value"].as_str().is_some_and(|h| !h.is_empty()), "the dimmer is on its wall: {sd:#}");
    let jb = device_of(&mut s, "JB");
    assert!(jb["host_wall"].is_null(), "a box 2.5' off a wall is not on it: {jb:#}");
}

#[test]
fn a_device_read_on_a_trade_sheet_lands_on_its_levels_architectural_plan() {
    let mut s = plan_with(true);
    let r = fill(&mut s);
    assert_eq!(r["devices"], json!(5), "{r:#}");
    let b = device_of(&mut s, "B");
    assert_eq!(b["location"]["value"], "15.00, 5.00, 1.50", "moved by the sheet frames: {b:#}");
    assert!(b["location"]["note"].as_str().is_some_and(|n| n.contains("sheet frame")), "{b:#}");
    assert_eq!(b["room"]["value"], "101", "{b:#}");
    // Where it was read stays its source.
    assert_eq!(b["circuit"]["source"]["doc"], "E-101");
    assert_eq!(b["circuit"]["source"]["at"], json!([3300.0, 60.0]));
    // Devices read on the plan itself stay where they are, in their wall's face.
    assert_eq!(device_of(&mut s, "A")["location"]["value"], "5.00, 0.42, 1.67");
}

#[test]
fn mount_heights_come_from_the_legend_then_a_note_then_a_marked_default() {
    let mut s = plan();
    fill(&mut s);
    let a = device_of(&mut s, "A");
    assert_eq!(a["mount_height"]["value"], json!(20.0));
    assert_eq!(a["mount_height"]["unit"], "in");
    assert_eq!(a["mount_height"]["source"]["kind"], "legend");
    let fa = device_of(&mut s, "FA");
    assert_eq!(fa["mount_height"]["value"], json!(80.0));
    assert_eq!(fa["mount_height"]["source"]["kind"], "note");
    let sw = device_of(&mut s, "S");
    assert_eq!(sw["mount_height"]["value"], json!(48.0));
    assert_eq!(sw["mount_height"]["source"]["kind"], "default", "never presented as read: {sw:#}");
    assert!(sw["mount_height"]["note"].as_str().is_some_and(|n| n.contains("not printed")));
    let j = device_of(&mut s, "J");
    assert_eq!(j["mount_height"]["value"], json!(18.0));
    assert_eq!(j["mount_height"]["source"]["kind"], "default");
    // The device box stands at its height.
    let d = s.doc().expect("doc");
    let zs: Vec<f64> = d
        .model
        .iter()
        .filter(|e| made_as(e).as_deref() == Some("device"))
        .filter_map(|e| match &e.kind {
            EntityKind::Mesh(m) => Some(m.vertices.iter().map(|v| v.z).sum::<f64>() / m.vertices.len().max(1) as f64),
            _ => None,
        })
        .collect();
    assert!(zs.iter().any(|z| (z - 80.0).abs() < 0.01), "the horn/strobe's box at 80\": {zs:?}");
}

#[test]
fn ducts_terminals_and_equipment_carry_size_system_airflow_and_capacity() {
    let mut s = plan();
    fill(&mut s);
    let ducts = find(&mut s, "duct", json!({}));
    assert!(!ducts.is_empty(), "the run is built");
    for h in &ducts {
        let p = get(&mut s, h);
        assert_eq!(p["system"]["value"], "SA", "{p:#}");
        assert_eq!(p["system"]["source"]["doc"], "M-101");
        if p["size"]["value"] == "12x8" {
            assert_eq!(p["size"]["source"]["kind"], "print", "a size read from a tag: {p:#}");
            assert_eq!(p["shape"]["value"], "rectangular");
            // The route height was estimated: a default, never a reading.
            assert_eq!(p["elevation"]["source"]["kind"], "default", "{p:#}");
            assert_eq!(p["elevation"]["value"], json!(8.5));
        }
    }
    let terminals = find(&mut s, "fixture", json!({"cfm": "exists"}));
    assert_eq!(terminals.len(), 1, "the diffuser has its airflow");
    let t = get(&mut s, &terminals[0]);
    assert_eq!(t["cfm"]["value"], json!(150.0));
    assert_eq!(t["cfm"]["source"]["kind"], "print");
    assert_eq!(t["type"]["value"], "diffuser");
    let ahu = find(&mut s, "equipment", json!({"type_mark": "AHU-1"}));
    assert_eq!(ahu.len(), 1);
    let a = get(&mut s, &ahu[0]);
    assert_eq!(a["capacity"]["value"], "2000");
    assert_eq!(a["capacity"]["unit"], "cfm");
    assert_eq!(a["capacity"]["source"]["kind"], "schedule");
    assert_eq!(a["capacity"]["source"]["doc"], "M-601");
}

#[test]
fn rooms_walls_and_doors_are_filled_from_the_plan() {
    let mut s = plan();
    // A door made like the build makes them, in the wall between the offices' east wall line,
    // with its number printed beside it.
    let wall = {
        let d = s.doc().expect("doc");
        d.model
            .iter()
            .filter(|e| made_as(e).as_deref() == Some("wall"))
            .map(|e| (cadaddy_engine::doc::entity_bounds(d, e, 0).center().dist(Vec2::new(120.0, 0.0)), e.handle))
            .min_by(|a, b| a.0.total_cmp(&b.0))
            .expect("a wall")
            .1
    };
    let door = {
        let d = s.doc_mut().expect("doc");
        let mut e = (**d.entity(wall).expect("wall")).clone();
        e.handle = d.new_handle();
        e.common.layer = "3D-Doors".into();
        e.common.xdata = vec![
            RawTag { code: 1001, value: "CADADDY_WALLGEN".into() },
            RawTag { code: 1000, value: "door".into() },
            RawTag { code: 1000, value: "L1".into() },
            RawTag { code: 1000, value: "0".into() },
        ]
        .into();
        let h = e.handle;
        d.model.push(e);
        h
    };
    let at = cadaddy_engine::doc::entity_bounds(s.doc().expect("doc"), s.doc().expect("doc").entity(door).expect("door"), 0).center();
    text(&mut s, "A-ANNO", [at.x + 10.0, at.y - 14.0], "101A");
    fill(&mut s);
    let dp = get(&mut s, &door.hex());
    assert_eq!(dp["number"]["value"], "101A", "{dp:#}");
    assert_eq!(dp["number"]["source"]["kind"], "print");
    assert_eq!(dp["width"]["source"]["kind"], "computed");
    assert_eq!(dp["from_room"]["value"], "101", "the room on its inside: {dp:#}");
    // Rooms: number from the tag, level and ceiling height.
    let rooms = find(&mut s, "room", json!({"number": "exists"}));
    assert_eq!(rooms.len(), 2, "{rooms:?}");
    let r = get(&mut s, &rooms[0]);
    assert_eq!(r["level"]["value"], "L1");
    assert_eq!(r["ceiling_height"]["value"], json!(9.0));
    assert_eq!(r["ceiling_height"]["unit"], "ft");
    // Walls: the build's type is the spec's rule for the layer, not a reading; the rooms on
    // each side.
    let walls = find(&mut s, "wall", json!({"type": "exists"}));
    assert!(walls.len() >= 8, "{walls:?}");
    let kinds: Vec<String> = walls.iter().map(|h| get(&mut s, h)["type"]["source"]["kind"].as_str().unwrap_or("").to_string()).collect();
    assert_eq!(kinds.iter().filter(|k| *k == "print").count(), 1, "the tagged wall only: {kinds:?}");
    assert_eq!(kinds.iter().filter(|k| *k == "rule").count(), walls.len() - 1, "{kinds:?}");
    let w = get(&mut s, &walls[0]);
    assert_eq!(w["type"]["value"], "Partition", "{w:#}");
    assert_eq!(w["thickness"]["value"], json!(6.0));
    assert_eq!(w["height"]["source"]["kind"], "computed");
    let sided = find(&mut s, "wall", json!({"room_a": "exists"})).len() + find(&mut s, "wall", json!({"room_b": "exists"})).len();
    assert!(sided >= 8, "each room's walls have it on one side: {sided}");
}

#[test]
fn a_persons_values_survive_the_fill_and_a_rebuild() {
    let mut s = plan();
    fill(&mut s);
    let h = find(&mut s, "device", json!({"type_mark": "A"})).remove(0);
    s.execute("bim.props.set", &json!({"handle": h, "key": "mount_height", "value": 46, "unit": "in"})).expect("set");
    s.execute("bim.props.set", &json!({"handle": h, "key": "room", "value": "OPEN OFFICE"})).expect("set");
    fill(&mut s);
    let a = device_of(&mut s, "A");
    assert_eq!(a["mount_height"]["value"], json!(46));
    assert_eq!(a["mount_height"]["source"]["kind"], "user");
    assert_eq!(a["room"]["value"], "OPEN OFFICE");
    // Rebuilt from the prints and filled again: the person's values come back.
    s.execute("model.build", &json!({})).expect("rebuild");
    fill(&mut s);
    let a = device_of(&mut s, "A");
    assert_eq!(a["mount_height"]["source"]["kind"], "user", "{a:#}");
    assert_eq!(a["mount_height"]["value"], json!(46));
    assert_eq!(a["room"]["value"], "OPEN OFFICE");
}

#[test]
fn the_fill_on_an_empty_or_unbuilt_drawing_fills_nothing_and_never_fails() {
    let mut s = Session::empty();
    s.open_drawing(Drawing::new_imperial(), "Empty", None);
    let r = fill(&mut s);
    assert_eq!(r["elements"], json!(0), "{r}");
    assert_eq!(r["devices"], json!(0));
    // A spec with hostile device positions and marks.
    let spec = json!({"version": 1, "units_per_foot": 12, "levels": [{"name": "L1", "elevation_ft": 0, "height_ft": 10, "ceiling_ft": 9}],
        "tags": [device(1e300, -1e300, "A", "LP1-3", "zz"), device(1.0, 1.0, &"Q".repeat(5000), &"-".repeat(3000), "1")]});
    s.execute("bspec.set", &json!({"replace": true, "spec": spec})).expect("spec");
    let _ = s.execute("model.build", &json!({}));
    let r = fill(&mut s);
    assert!(r["devices"].as_u64().is_some(), "{r}");
}

#[test]
fn the_text_readers() {
    assert_eq!(mount_height_in("DUPLEX RECEPTACLE +18\" AFF"), Some(18.0));
    assert_eq!(mount_height_in("MOUNT AT 48\" A.F.F."), Some(48.0));
    assert_eq!(mount_height_in("THERMOSTAT 4'-6\" AFF"), Some(54.0));
    assert_eq!(mount_height_in("SWITCH 4' AFF"), Some(48.0));
    assert_eq!(mount_height_in("12\" DUCT"), None, "no word saying it is a mounting height");
    assert_eq!(mount_height_in("+2000\" AFF"), None, "out of range");
    assert_eq!(panel_of("LP1-3"), Some("LP1".into()));
    assert_eq!(panel_of("2LA-12"), Some("2LA".into()));
    assert_eq!(panel_of("H1/7"), Some("H1".into()));
    assert_eq!(panel_of("12"), None);
    assert_eq!(panel_of("1-3"), None, "no letters: not a panel name");
    assert_eq!(split_switch_leg("A-1,a"), ("A-1".into(), Some("a".into())));
    assert_eq!(split_switch_leg("LP-3,5"), ("LP-3,5".into(), None), "two circuits, no leg");
    assert_eq!(split_switch_leg("D-7"), ("D-7".into(), None));
    for t in ["101", "101A", "D-12", "1-104B"] {
        assert!(door_number(t), "{t}");
    }
    for t in ["DOOR", "12'-6\"", "OFFICE 101", "ABCD12345"] {
        assert!(!door_number(t), "{t}");
    }
    assert_eq!(cfm_in("150 CFM"), Some(150.0));
    assert_eq!(cfm_in("12x12 / 1,200CFM"), Some(1200.0));
    assert_eq!(cfm_in("CFM: 200"), Some(200.0));
    assert_eq!(cfm_in("CFM"), None);
    let sheets: std::collections::BTreeSet<String> = ["E-201".to_string(), "A-301".to_string()].into();
    for t in ["G-12", "LP-3,5", "LP1-3", "2LA-12", "H1/7"] {
        assert!(cadaddy_engine::cmd::guided::circuit_text(t, &sheets), "{t}");
    }
    for t in ["E-201", "A-301", "LP1-3333", "12", "SEE NOTE 4"] {
        assert!(!cadaddy_engine::cmd::guided::circuit_text(t, &sheets), "{t}");
    }
}

/// The guided job on a made-up hotel's plans: every device it reads is an element with its
/// mounting height and location, the duct and pipe runs have size and system, rooms numbers.
#[test]
fn the_guided_job_fills_the_elements_properties() {
    let _g = serial();
    real_pdf_reader();
    let mut s = run_job("job", &["A-101", "E-201", "M-101", "P-101"]);
    let sum = s.execute("bim.props.summary", &json!({})).expect("summary");
    let k = &sum["kinds"];
    let read = s.execute("bspec.get", &json!({})).expect("spec")["spec"]["tags"]
        .as_array()
        .map_or(0, |t| t.iter().filter(|t| t["id"].as_str().is_some_and(|i| i.starts_with("device:")) && t["fields"]["symbol"].is_object()).count());
    assert!(read > 0, "the job reads devices: {sum:#}");
    assert_eq!(k["device"]["count"], json!(read), "one element per device read: {:#}", k["device"]);
    for key in ["mount_height", "location", "type_mark", "sheet"] {
        assert_eq!(k["device"]["filled"][key], json!(read), "{key}: {:#}", k["device"]);
    }
    let circuits = k["device"]["filled"]["circuit"].as_u64().unwrap_or(0);
    assert_eq!(k["device"]["filled"]["fed_from"].as_u64().unwrap_or(0), circuits, "every device with a circuit is fed from it");
    for kind in ["duct", "pipe"] {
        let n = k[kind]["count"].as_u64().unwrap_or(0);
        assert!(n > 0, "{kind}s built: {sum:#}");
        assert_eq!(k[kind]["filled"]["size"], json!(n), "{kind}: {:#}", k[kind]);
        assert_eq!(k[kind]["filled"]["system"], json!(n), "{kind}: {:#}", k[kind]);
    }
    assert!(k["room"]["filled"]["number"].as_u64().unwrap_or(0) > 0, "{:#}", k["room"]);
    assert!(k["wall"]["filled"]["type"].as_u64().unwrap_or(0) > 0, "{:#}", k["wall"]);
}

/// Two rooms sharing a 6" partition (x 19.75'–20.25'), a receptacle drawn on each face of it
/// and one on the outer wall: each takes the room on the face it serves, none is in no room.
#[test]
fn a_device_on_a_shared_wall_takes_the_room_on_its_face() {
    let _g = serial();
    let mut d = Drawing::new_imperial();
    let outer = [((0., 0.), (40.5, 0.)), ((40.5, 0.), (40.5, 10.)), ((40.5, 10.), (0., 10.)), ((0., 10.), (0., 0.))];
    let left = [((0.5, 0.5), (19.75, 0.5)), ((19.75, 0.5), (19.75, 9.5)), ((19.75, 9.5), (0.5, 9.5)), ((0.5, 9.5), (0.5, 0.5))];
    let right = [((20.25, 0.5), (40.0, 0.5)), ((40.0, 0.5), (40.0, 9.5)), ((40.0, 9.5), (20.25, 9.5)), ((20.25, 9.5), (20.25, 0.5))];
    for (a, b) in outer.into_iter().chain(left).chain(right) {
        line(&mut d, "A-WALL", a, b);
    }
    let mut s = Session::empty();
    s.open_drawing(d, "Shared Wall", None);
    text(&mut s, "A-ROOM-TAG", [110.0, 64.0], "OFFICE");
    text(&mut s, "A-ROOM-TAG", [116.0, 52.0], "101");
    text(&mut s, "A-ROOM-TAG", [350.0, 64.0], "STORAGE");
    text(&mut s, "A-ROOM-TAG", [356.0, 52.0], "102");
    let spec = json!({
        "version": 1, "units_per_foot": 12,
        "levels": [{"name": "L1", "elevation_ft": 0, "height_ft": 10, "ceiling_ft": 9, "base": [0, 0]}],
        "layers": [{"layer": "A-WALL", "role": "wall", "wall_type": "Partition"}, {"layer": "A-ROOM-TAG", "role": "room_tag"}],
        "wall_types": [{"name": "Partition", "thickness_in": 6}],
        "tags": [device(19.9, 5.0, "A", "", "1F0"), device(20.1, 5.0, "B", "", "1F1"), device(30.0, 0.35, "C", "", "1F2"),
                 device(10.0, 2.0, "D", "", "1F3"), device(10.0, 5.0, "E", "", "1F4")]
    });
    s.execute("bspec.set", &json!({"replace": true, "spec": spec})).expect("spec");
    s.execute("model.build", &json!({})).expect("build");
    s.execute("rooms.build", &json!({})).expect("rooms");
    let r = fill(&mut s);
    assert_eq!(device_of(&mut s, "A")["room"]["value"], "101", "the partition's west face: {r:#}");
    assert_eq!(device_of(&mut s, "B")["room"]["value"], "102", "its east face");
    assert_eq!(device_of(&mut s, "C")["room"]["value"], "102", "the outer wall's inside face");
    assert!(r["devices_in_no_room"].as_object().is_none_or(|m| m.is_empty()), "{r:#}");
    // Each wall device set in its wall, its box's face flush with the wall's face (its middle 1"
    // inside), where it was along the wall: A in the partition's west face (19.75'), D read 1.5'
    // into the room in the south wall's inside face (0.5'); E, a device of no known kind mid-room
    // 4.5' from any wall, stays for review.
    let xy = |el: &Value| -> (f64, f64) {
        let v: Vec<f64> = el["location"]["value"].as_str().unwrap_or("").split(',').filter_map(|p| p.trim().parse().ok()).collect();
        (v.first().copied().unwrap_or(f64::NAN), v.get(1).copied().unwrap_or(f64::NAN))
    };
    let (ax, ay) = xy(&device_of(&mut s, "A"));
    assert!((ax - (19.75 + 1.0 / 12.0)).abs() < 0.02 && (ay - 5.0).abs() < 0.02, "A at {ax}, {ay}");
    let (dx, dy) = xy(&device_of(&mut s, "D"));
    assert!((dx - 10.0).abs() < 0.02 && (dy - (0.5 - 1.0 / 12.0)).abs() < 0.02, "D at {dx}, {dy}");
    let a = device_of(&mut s, "A");
    assert!(a["host_wall"]["value"].as_str().is_some_and(|h| !h.is_empty()) && a["face_normal"]["value"].is_string(), "{a:#}");
    let e = device_of(&mut s, "E");
    assert_eq!(xy(&e), (10.0, 5.0));
    assert!(e["placement"]["value"].as_str().is_some_and(|p| p.starts_with("review")), "{e:#}");
}

/// A room's load center read beside its wall is hosted in it: recessed (its front flush with the
/// wall's room face, its back in the wall), turned with the wall, the wall's handle on it; filed
/// as electrical gear.
#[test]
fn a_recessed_room_panel_is_built_in_its_wall() {
    let _g = serial();
    let mut s = plan();
    let mut spec = s.execute("bspec.get", &json!({})).expect("spec")["spec"].clone();
    if let Some(eq) = spec["mep"]["equipment"].as_array_mut() {
        // Read 1' into office 101 off its south wall (inside face at y = 6").
        eq.push(
            json!({"tag": "G-101", "kind": "panel", "level": "L1", "min": [100.0, 18.0], "max": [114.0, 22.0], "bottom_ft": 4.5, "height_ft": 2.0,
                       "properties": {"MOUNTING": "recessed, front flush with the wall"}}),
        );
    }
    s.execute("bspec.set", &json!({"replace": true, "spec": spec})).expect("spec");
    s.execute("model.build", &json!({})).expect("build");
    let d = s.doc().expect("doc");
    let panel = d.model.iter().find(|e| e.common.xdata.iter().any(|t| t.value == "tag:G-101")).expect("the panel built");
    let b = cadaddy_engine::doc::entity_bounds(d, panel, 0);
    // Its 14" along the wall, its 4" deep into the wall (y 2"..6": the wall is 0..6").
    assert!((b.max.y - 6.0).abs() < 0.01 && (b.min.y - 2.0).abs() < 0.01, "in the wall: {b:?}");
    assert!((b.max.x - b.min.x - 14.0).abs() < 0.01, "along the wall: {b:?}");
    assert!(panel.common.xdata.iter().any(|t| t.value.starts_with("prop:HOST_WALL=")), "{:?}", panel.common.xdata);
}
