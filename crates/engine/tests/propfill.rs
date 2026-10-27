//! Q6002 propfill: the guided job's readers fill the elements' properties.

use std::sync::{Mutex, MutexGuard};

use cadaddy_engine::Session;
use cadaddy_engine::cmd::propfill::{
    airflow_number, cfm_in, device_class, device_type_of, door_number, level_key, mount_height_in, panel_of, room_number_in, terminal_type_tag,
};
use cadaddy_engine::cmd::wallgen::made_as;
use cadaddy_engine::doc::{Attrib, AttribModes, AttribProps, Block, Drawing, Entity, EntityKind, Handle, Insert, Line, RawTag};
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
    plan_from(on_e_sheet, 0)
}

/// [`plan_with`] with `filler` short lines between the walls and the texts (a print set imported
/// sheet by sheet: the later sheets' texts come after many objects).
fn plan_from(on_e_sheet: bool, filler: usize) -> Session {
    let mut d = Drawing::new_imperial();
    room(&mut d, 0.0);
    room(&mut d, 30.0);
    for i in 0..filler {
        let y = -40.0 - (i % 100) as f64 * 0.01;
        line(&mut d, "G-ANNO", (-50.0, y), (-49.9, y));
    }
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
    // 5', 0.8' on the plan, 20" up.
    assert_eq!(a["location"]["value"], "5.00, 0.80, 1.67", "{a:#}");
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
    // Devices read on the plan itself stay where they are.
    assert_eq!(device_of(&mut s, "A")["location"]["value"], "5.00, 0.80, 1.67");
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
    // 3.5' from the door's centre (a tag in front of the swing), nearer than the room's tag.
    text(&mut s, "A-ANNO", [at.x + 24.0, at.y - 34.0], "101A");
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
    for t in ["A", "CD-1", "SR2", "RG-1"] {
        assert!(terminal_type_tag(t), "{t}");
    }
    for t in ["", "12", "ABCD-1", "CD-123", "CD 1"] {
        assert!(!terminal_type_tag(t), "{t}");
    }
    for (t, ty, class) in [
        ("DUPLEX RECEPTACLE, +18\" AFF", "duplex receptacle", "duplex_receptacle"),
        ("GFCI DUPLEX RECEPTACLE", "GFCI receptacle", "gfci"),
        ("SINGLE POLE SWITCH", "switch", "switch"),
        ("3-WAY SWITCH", "3-way switch", "switch"),
        ("2X4 LED TROFFER", "light fixture", "light"),
        ("DATA OUTLET, CAT6", "data/voice outlet", "data"),
        ("FIRE ALARM HORN/STROBE", "fire alarm horn/strobe", "fire_alarm"),
        ("SMOKE DETECTOR", "smoke detector", "fire_alarm"),
        ("CEILING EXHAUST FAN", "exhaust fan", "ef"),
        ("FUSED DISCONNECT SWITCH", "disconnect switch", "disconnect_switch"),
    ] {
        assert_eq!(device_type_of(t), Some(ty), "{t}");
        assert_eq!(device_class(ty), class, "{ty}");
    }
    for t in ["", "A", "SEE NOTE 4", "SWITCHBOARD SB-1", "LIGHTNING PROTECTION"] {
        assert_eq!(device_type_of(t), None, "{t}");
    }
    for l in ["Level 1", "LEVEL 1", "1ST FLOOR", "FIRST FLOOR PLAN", "L1", "1"] {
        assert_eq!(level_key(l), "#1", "{l}");
    }
    assert_eq!(level_key("Level 2"), "#2");
    assert_eq!(level_key("ROOF"), "R");
    assert_eq!(level_key("Mezzanine"), "MEZZANINE");
    assert_eq!(airflow_number("150"), Some(150.0));
    assert_eq!(airflow_number("1,200"), Some(1200.0));
    for t in ["10", "9000", "150 CFM", "12x12", "1A"] {
        assert_eq!(airflow_number(t), None, "{t}");
    }
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

/// Probe (run by hand: `--ignored --nocapture`): the made-up full hotel's property summary after
/// the guided job, beside the set's own counts.
#[test]
#[ignore]
fn probe_full_hotel_summary() {
    let _g = serial();
    real_pdf_reader();
    let set = generate_hotel(42, HotelSize::Full).expect("generate hotel");
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos());
    let dir = std::env::temp_dir().join(format!("cadaddy_propfill_probe_{}_{nanos}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("folder");
    let pages: Vec<(Vec<u8>, String, String)> = set
        .truth
        .sheets
        .iter()
        .map(|sh| (set.file(&sh.pdf).expect("sheet pdf").to_vec(), format!("{} {}", sh.number, sh.title), String::new()))
        .collect();
    std::fs::write(dir.join("Full Set.pdf"), cadaddy_io::pdf_merge::merge(&pages).expect("merge")).expect("write set");
    let mut s = Session::new();
    let st = s.execute("job.start", &json!({"pdfs": dir.display().to_string(), "goal": "takeoff", "auto": true, "checkpoint": false})).expect("job");
    let _ = std::fs::remove_dir_all(&dir);
    eprintln!("done {}; the set: {} fixtures, {} terminals", st["done"], set.truth.fixtures.len(), set.truth.terminals.len());
    let spec = s.execute("bspec.get", &json!({})).expect("spec")["spec"].clone();
    let mut by_type: std::collections::BTreeMap<String, usize> = Default::default();
    for t in spec["tags"].as_array().into_iter().flatten().filter(|t| t["fields"]["symbol"].is_object()) {
        *by_type.entry(format!("{} ({})", t["fields"]["device_type"]["value"], t["fields"]["device_type"]["source"])).or_default() += 1;
    }
    eprintln!("devices by type: {by_type:?}");
    let build = s.execute("job.explain", &json!({"stage": "bim_build"})).expect("explain")["result"].clone();
    eprintln!("fill diagnostics: {}", build["props"]["diagnostics"]);
    let sum = s.execute("bim.props.summary", &json!({})).expect("summary");
    for k in ["device", "door", "room", "wall", "fixture", "equipment", "duct", "pipe"] {
        eprintln!("{k}: count {} filled {}", sum["kinds"][k]["count"], sum["kinds"][k]["filled"]);
    }
}

/// [`plan_with`] on a set of sheets side by side: the floor plan A-101, then a mechanical plan
/// M-101 and a plumbing plan P-101 of the same size. The model is built again with a diffuser
/// and an air handler read on M-101 (moved onto the floor plan, as the readers do), a device on
/// office 101's wall; the trades' tags are printed on their own sheets.
fn trade_sheets() -> Session {
    let mut s = plan_with(true);
    let mut spec = s.execute("bspec.get", &json!({})).expect("spec")["spec"].clone();
    if let Some(sh) = spec["sheets"].as_array_mut() {
        sh.push(json!({"number": "M-101", "discipline": "M", "level": "L1", "source": "model", "rect": [6000, -120, 7320, 600]}));
        sh.push(json!({"number": "P-101", "discipline": "P", "level": "L1", "source": "model", "rect": [9000, -120, 10320, 600]}));
    }
    // A diffuser in office 101 (10', 5' on the plan); an air handler with no schedule row in the
    // spec (its row comes with the fill's `schedules`).
    if let Some(t) = spec["mep"]["terminals"].as_array_mut() {
        t.push(json!({"system": "SA", "level": "L1", "at": [120, 60], "size_in": [12, 12], "bottom_ft": 8, "terminal_kind": "supply_diffuser"}));
    }
    if let Some(e) = spec["mep"]["equipment"].as_array_mut() {
        e.push(
            json!({"tag": "AHU-2", "kind": "ahu", "level": "L1", "min": [380, 20], "max": [440, 60], "height_ft": 5, "sources": {"sheet": "M-101"}}),
        );
    }
    // A receptacle drawn on office 101's south wall, outside the room's outline.
    if let Some(t) = spec["tags"].as_array_mut() {
        t.push(device(8.0, 0.2, "W", "LP1-7", "1F5"));
    }
    s.execute("bspec.set", &json!({"replace": true, "spec": spec})).expect("spec");
    s.execute("model.build", &json!({})).expect("build");
    s.execute("rooms.build", &json!({})).expect("rooms");
    // The diffuser's tag on M-101: its type over its airflow, beside it (10', 5' → 6240", 60").
    text(&mut s, "M-ANNO", [6260.0, 66.0], "CD-1");
    text(&mut s, "M-ANNO", [6260.0, 56.0], "225");
    // P-101: a water closet tagged by its mark at 5', 2.5' and a lavatory by its letters at 15', 2.5';
    // the plan's legend names the mark (not a fixture).
    text(&mut s, "P-ANNO", [9180.0, 30.0], "WC-1");
    text(&mut s, "P-ANNO", [9300.0, 30.0], "LAV");
    text(&mut s, "P-ANNO", [10000.0, 500.0], "WC-1");
    text(&mut s, "P-ANNO", [10040.0, 500.0], "WATER CLOSET, FLUSH VALVE");
    // The floor plan tags the same water closet again.
    text(&mut s, "A-ANNO", [62.0, 31.0], "WC-1");
    s
}

/// The schedule rows the guided job reads off the M and P schedule sheets.
fn schedule_rows() -> Value {
    json!([
        {"tag": "WC-1", "sheet": "P-601", "title": "PLUMBING FIXTURE SCHEDULE",
         "row": {"TAG": "WC-1", "FIXTURE": "WATER CLOSET, FLUSH VALVE", "CW": "1\"", "HW": "-", "SAN": "3\"", "V": "2\""}},
        {"tag": "LAV-1", "sheet": "P-601", "title": "PLUMBING FIXTURE SCHEDULE",
         "row": {"TAG": "LAV-1", "FIXTURE": "LAVATORY", "CW": "1/2\"", "HW": "1/2\"", "SAN": "1-1/2\"", "V": "1-1/2\""}},
        {"tag": "AHU-2", "sheet": "M-601", "title": "AIR HANDLING UNIT SCHEDULE", "row": {"TAG": "AHU-2", "CFM": "3000", "VOLTS": "460/3/60"}},
        {"tag": "CD-1", "sheet": "M-601", "title": "AIR DEVICE SCHEDULE", "row": {"TAG": "CD-1", "NECK (IN)": "10"}}
    ])
}

/// What the trades printed on their own sheets reaches the elements moved onto the floor plan:
/// a terminal's airflow from its tag on the mechanical plan, an air handler's capacity from its
/// schedule row, plumbing fixtures placed at their tags with the schedule's connection sizes
/// (once each, a legend's entry none), and a device on a room's wall is in that room.
#[test]
fn the_trades_sheets_give_airflow_capacity_fixtures_and_rooms() {
    let mut s = trade_sheets();
    let r = s.execute("bim.props.fill", &json!({"schedules": schedule_rows()})).expect("fill");
    assert_eq!(r["fixtures_placed"], json!(2), "{r:#}");

    let with_cfm: Vec<Value> = find(&mut s, "fixture", json!({"cfm": "exists"})).iter().map(|h| get(&mut s, h)).collect();
    let cd =
        with_cfm.iter().find(|p| p["cfm"]["value"] == json!(225.0)).unwrap_or_else(|| panic!("the diffuser's airflow from M-101: {with_cfm:#?}"));
    assert_eq!(cd["cfm"]["source"]["kind"], "print", "{cd:#}");
    assert_eq!(cd["cfm"]["source"]["doc"], "M-101", "read on the mechanical plan: {cd:#}");
    assert_eq!(cd["room"]["value"], "101", "{cd:#}");

    let ahu = find(&mut s, "equipment", json!({"type_mark": "AHU-2"}));
    assert_eq!(ahu.len(), 1);
    let a = get(&mut s, &ahu[0]);
    assert_eq!(a["capacity"]["value"], "3000", "{a:#}");
    assert_eq!(a["capacity"]["unit"], "cfm");
    assert_eq!(a["capacity"]["source"]["kind"], "schedule");
    assert!(a["capacity"]["source"]["doc"].as_str().is_some_and(|d| d.contains("M-601")), "{a:#}");

    let wc = find(&mut s, "fixture", json!({"type_mark": "WC-1"}));
    assert_eq!(wc.len(), 1, "one water closet: the floor plan's tag is the same one, the legend's is none");
    let w = get(&mut s, &wc[0]);
    assert_eq!(w["type"]["value"], "WATER CLOSET, FLUSH VALVE");
    assert_eq!(w["cw_size"]["value"], "1\"", "{w:#}");
    assert_eq!(w["waste_size"]["value"], "3\"");
    assert_eq!(w["vent_size"]["value"], "2\"");
    assert!(w["hw_size"].is_null(), "no hot water: {w:#}");
    assert_eq!(w["connections"]["source"]["kind"], "schedule");
    assert!(w["connections"]["value"].as_str().is_some_and(|c| c.contains("CW 1\"") && c.contains("SAN 3\"")), "{w:#}");
    assert_eq!(w["type_mark"]["source"]["kind"], "print");
    assert_eq!(w["sheet"]["value"], "P-101");
    assert_eq!(w["room"]["value"], "101", "moved onto the floor plan, into its room: {w:#}");
    let lav = find(&mut s, "fixture", json!({"type_mark": "LAV-1"}));
    assert_eq!(lav.len(), 1, "the lavatory tagged by its letters");
    assert_eq!(get(&mut s, &lav[0])["hw_size"]["value"], "1/2\"");

    let dev = device_of(&mut s, "W");
    assert_eq!(dev["room"]["value"], "101", "{dev:#}");
    assert_eq!(dev["room"]["note"], "the nearest room: the device is on its wall");

    // Filling again places nothing twice.
    let again = s.execute("bim.props.fill", &json!({"schedules": schedule_rows()})).expect("fill");
    assert_eq!(again["fixtures_placed"], json!(0), "{again:#}");
    assert_eq!(find(&mut s, "fixture", json!({"type_mark": "WC-1"})).len(), 1);
    // Hostile schedules: not a list, rows without a tag or a row: nothing placed, no failure.
    for bad in [json!("x"), json!([{"tag": ""}, {"row": {"CW": "1"}}, 5])] {
        let r = s.execute("bim.props.fill", &json!({"schedules": bad})).expect("fill");
        assert_eq!(r["fixtures_placed"], json!(0));
    }
}

/// The guided job on a made-up hotel's second floor with the M and P schedule sheets: the
/// plumbing fixtures are elements with their connection sizes and the air handler has its
/// capacity, both from the schedules.
#[test]
fn the_guided_job_reads_the_schedules_onto_fixtures_and_equipment() {
    let _g = serial();
    real_pdf_reader();
    let mut s = run_job("sched", &["A-101", "M-101", "A-102", "P-102", "M-601", "P-601"]);
    let build = s.execute("job.explain", &json!({"stage": "bim_build"})).expect("explain");
    let read = &build["result"]["schedules_read"];
    assert_eq!(read["sheets"], json!(["M-601", "P-601"]), "the schedule sheets the job read: {read:#}");
    assert!(read["rows"].as_u64().unwrap_or(0) >= 4, "{read:#}");
    assert!(build["result"]["props"]["diagnostics"]["schedule_rows"].as_u64().unwrap_or(0) >= 4, "{:#}", build["result"]["props"]);
    let sum = s.execute("bim.props.summary", &json!({})).expect("summary");
    let k = &sum["kinds"];
    let fixtures = k["fixture"]["filled"]["connections"].as_u64().unwrap_or(0);
    assert!(fixtures > 0, "fixtures with connections: {:#}", k["fixture"]);
    assert_eq!(k["fixture"]["filled"]["cw_size"].as_u64(), Some(fixtures), "{:#}", k["fixture"]);
    assert!(k["equipment"]["filled"]["capacity"].as_u64().unwrap_or(0) > 0, "{:#}", k["equipment"]);
}

/// A room element whose tag carries no number takes the room number printed inside it (`print`,
/// with where), and the devices standing in it get that room; the fill says what it went on.
#[test]
fn a_room_without_a_numbered_tag_takes_the_number_printed_inside_it() {
    let mut s = plan();
    // Office 101's room element, its tag's number taken off.
    let room = {
        let d = s.doc().expect("doc");
        d.model
            .iter()
            .filter(|e| made_as(e).as_deref() == Some("room"))
            .find(|e| e.common.xdata.iter().any(|t| t.value == "number:101"))
            .map(|e| e.handle)
            .expect("office 101's room element")
    };
    s.doc_mut()
        .expect("doc")
        .modify_entity(room, |e| {
            e.common.xdata = e.common.xdata.iter().filter(|t| !t.value.starts_with("number:")).cloned().collect::<Vec<_>>().into()
        })
        .expect("untag");
    assert!(get(&mut s, &room.hex())["number"].is_null(), "no number before the fill");
    let r = fill(&mut s);
    assert_eq!(r["diagnostics"]["rooms_numbered_from_text"], json!(1), "{r:#}");
    assert!(r["diagnostics"]["rooms"].as_u64().unwrap_or(0) >= 2, "{r:#}");
    assert!(r["diagnostics"]["element_levels"]["L1"].as_u64().unwrap_or(0) >= 4, "{r:#}");
    let p = get(&mut s, &room.hex());
    assert_eq!(p["number"]["value"], "101", "{p:#}");
    assert_eq!(p["number"]["source"]["kind"], "print");
    assert_eq!(p["number"]["note"], "the room number printed inside the room");
    assert_eq!(device_of(&mut s, "A")["room"]["value"], "101", "the device in it gets its number");
    let sum = s.execute("bim.props.summary", &json!({})).expect("summary");
    assert_eq!(sum["kinds"]["room"]["filled"]["number"], json!(2), "{:#}", sum["kinds"]["room"]);
}

#[test]
fn texts_past_the_first_two_hundred_thousand_objects_are_read() {
    // Q6002 round 3: the fill and the build's room tracing read the plan's texts from the first
    // 200,000 objects only; a whole print set is more, so its later sheets gave no room numbers.
    let mut s = plan_from(false, 200_000);
    let rooms: Vec<String> = {
        let d = s.doc().expect("doc");
        assert!(d.model.len() > 200_000, "the texts come after the first 200,000 objects");
        d.model.iter().filter(|e| made_as(e).as_deref() == Some("room")).map(|e| e.handle.hex()).collect()
    };
    assert_eq!(rooms.len(), 2, "the build traces both rooms from their tags: {rooms:?}");
    let r = fill(&mut s);
    assert!(r["diagnostics"]["drawing_objects"].as_u64().unwrap_or(0) > 200_000, "{r:#}");
    assert_eq!(r["diagnostics"]["room_elements"], json!(2), "{r:#}");
    let numbers: Vec<Value> = rooms.iter().map(|h| get(&mut s, h)["number"]["value"].clone()).collect();
    assert!(numbers.contains(&json!("101")) && numbers.contains(&json!("102")), "{numbers:?}");
    assert_eq!(device_of(&mut s, "A")["room"]["value"], "101");
    let terminals = find(&mut s, "fixture", json!({"cfm": "exists"}));
    assert_eq!(terminals.len(), 1, "the diffuser's airflow, printed after the filler: {r:#}");
    assert_eq!(get(&mut s, &terminals[0])["cfm"]["value"], json!(150.0));
    let sum = s.execute("bim.props.summary", &json!({})).expect("summary");
    assert_eq!(sum["kinds"]["room"]["filled"]["number"], json!(2), "{:#}", sum["kinds"]["room"]);
}

#[test]
fn a_terminal_tag_held_in_a_symbol_gives_its_airflow_from_the_schedule() {
    // Q6002 round 3: the PDF import puts a tag drawn beside a repeated symbol in the symbol's
    // TAG attribute (at the origin), so `CD-1` is no text of its own; with no airflow printed,
    // the schedule's CD-1 row gives it.
    let mut s = plan();
    let style = {
        let d = s.doc_mut().expect("doc");
        let cfm =
            d.model.iter().find(|e| matches!(&e.kind, EntityKind::Text(t) if t.value == "150 CFM")).map(|e| e.handle).expect("the printed airflow");
        let style = d.model.iter().find_map(|e| if let EntityKind::Text(t) = &e.kind { Some(t.clone()) } else { None }).expect("a text");
        d.remove_entity(cfm);
        let mut b = Block::new("PDF-SYM-1");
        let h = d.new_handle();
        b.entities.push(Entity::new(h, EntityKind::Line(Line { a: Vec3::new(-4.0, 0.0, 0.0), b: Vec3::new(4.0, 0.0, 0.0) })));
        d.blocks.insert("PDF-SYM-1".into(), std::sync::Arc::new(b));
        let tag = Attrib {
            tag: "TAG".into(),
            text: cadaddy_engine::doc::Text { value: "CD-1".into(), insert: Vec3::new(0.0, 0.0, 0.0), align_pt: None, ..style.clone() },
            invisible: false,
            constant: false,
            prompt: String::new(),
            props: AttribProps::default(),
            multiline: None,
            modes: AttribModes::default(),
        };
        let ins = Insert {
            block: "PDF-SYM-1".into(),
            insert: Vec3::new(1500.0, 520.0, 0.0),
            scale: Vec3::new(1.0, 1.0, 1.0),
            rotation: 0.0,
            attribs: vec![tag],
            cols: 1,
            rows: 1,
            col_spacing: 0.0,
            row_spacing: 0.0,
            clip: None,
        };
        let h = d.new_handle();
        let mut e = Entity::new(h, EntityKind::Insert(ins));
        e.common.layer = "M-ANNO".into();
        d.model.push(e);
        style
    };
    assert_eq!(style.height, 6.0);
    let rows = json!([{"tag": "CD-1", "sheet": "M-601", "title": "AIR DEVICE SCHEDULE",
                       "row": {"TYPE": "CD-1", "NECK SIZE": "8\"", "CFM": "225"}}]);
    let r = s.execute("bim.props.fill", &json!({"schedules": rows})).expect("fill");
    let terminals = find(&mut s, "fixture", json!({"cfm": "exists"}));
    assert_eq!(terminals.len(), 1, "{r:#}");
    let t = get(&mut s, &terminals[0]);
    assert_eq!(t["cfm"]["value"], json!(225.0), "{t:#}");
    assert_eq!(t["cfm"]["source"]["kind"], "schedule");
    assert_eq!(t["cfm"]["source"]["doc"], "AIR DEVICE SCHEDULE M-601");
    // No schedule row: the tag alone gives no airflow.
    let mut bare = plan();
    let cfm = bare.doc().expect("doc").model.iter().find(|e| matches!(&e.kind, EntityKind::Text(t) if t.value == "150 CFM")).map(|e| e.handle);
    bare.doc_mut().expect("doc").remove_entity(cfm.expect("the printed airflow"));
    fill(&mut bare);
    assert!(find(&mut bare, "fixture", json!({"cfm": "exists"})).is_empty());
}

// ─── Round 4: rooms numbered from the job's rooms reading ───────────────────

/// Office 101's room element with its tag's number taken off, and the printed `101` gone.
fn office_without_its_number(s: &mut Session) -> Handle {
    let (room, printed) = {
        let d = s.doc().expect("doc");
        let room = d
            .model
            .iter()
            .filter(|e| made_as(e).as_deref() == Some("room"))
            .find(|e| e.common.xdata.iter().any(|t| t.value == "number:101"))
            .map(|e| e.handle)
            .expect("office 101's room element");
        let printed: Vec<Handle> =
            d.model.iter().filter(|e| matches!(&e.kind, EntityKind::Text(t) if t.value.trim() == "101")).map(|e| e.handle).collect();
        (room, printed)
    };
    assert_eq!(printed.len(), 1, "one printed 101");
    let d = s.doc_mut().expect("doc");
    d.modify_entity(room, |e| e.common.xdata = e.common.xdata.iter().filter(|t| !t.value.starts_with("number:")).cloned().collect::<Vec<_>>().into())
        .expect("untag");
    for h in printed {
        d.remove_entity(h);
    }
    room
}

#[test]
fn room_numbers_in_a_one_line_tag() {
    assert_eq!(room_number_in("101").as_deref(), Some("101"));
    assert_eq!(room_number_in(" 101A ").as_deref(), Some("101A"));
    assert_eq!(room_number_in("OFFICE 101").as_deref(), Some("101"));
    assert_eq!(room_number_in("101 STORAGE").as_deref(), Some("101"));
    assert_eq!(room_number_in("ROOMS 101 TO 103"), None, "two numbers name no one room");
    assert_eq!(room_number_in("OFFICE"), None);
    assert_eq!(room_number_in("LEVEL 2"), None, "one digit is no room number");
    assert_eq!(room_number_in("12 34"), None, "no name word");
    assert_eq!(room_number_in("1/2 HR RATED"), None);
}

#[test]
fn a_room_tagged_name_and_number_on_one_line_takes_its_number() {
    // Q6002 round 4: a print that sets a room's name and number on one line ("OFFICE 101") gave
    // no number: only a text that is a number alone counted.
    let mut s = plan();
    let room = office_without_its_number(&mut s);
    text(&mut s, "A-ANNO", [60.0, 30.0], "OFFICE 101");
    let r = fill(&mut s);
    assert_eq!(r["diagnostics"]["rooms_numbered_from_text"], json!(1), "{r:#}");
    let p = get(&mut s, &room.hex());
    assert_eq!(p["number"]["value"], "101", "{p:#}");
    assert_eq!(p["number"]["source"]["kind"], "print");
    assert_eq!(device_of(&mut s, "A")["room"]["value"], "101");
}

#[test]
fn a_room_with_no_number_printed_inside_takes_the_number_the_rooms_reading_gave() {
    // Q6002 round 4: on the real set the job's rooms stage reads ~200 numbered rooms, yet the
    // model's rooms had no number: the fill only looked for a number text inside each room. Now
    // the job passes its reading and a room takes the number paired with the label inside it.
    let mut s = plan();
    let room = office_without_its_number(&mut s);
    let r = fill(&mut s);
    assert_eq!(r["diagnostics"]["rooms_numbered_from_text"], json!(0), "nothing printed: {r:#}");
    assert!(get(&mut s, &room.hex())["number"].is_null(), "no number without the reading");
    let rooms = json!([
        // The label inside office 101 (plan coordinates), as the rooms stage read it.
        {"number": "101", "name": "OFFICE", "level": "LEVEL 1", "sheet": "A-101", "at": [110.0, 64.0]},
        // Read on another level: never office 101's number.
        {"number": "201", "name": "OFFICE", "level": "LEVEL 2", "sheet": "A-102", "at": [112.0, 60.0]}
    ]);
    let r = s.execute("bim.props.fill", &json!({"rooms": rooms})).expect("fill");
    assert_eq!(r["diagnostics"]["rooms_numbered_from_reading"], json!(1), "{r:#}");
    assert_eq!(r["diagnostics"]["read_rooms"], json!(2), "{r:#}");
    let p = get(&mut s, &room.hex());
    assert_eq!(p["number"]["value"], "101", "{p:#}");
    assert_eq!(p["number"]["source"]["kind"], "print");
    assert_eq!(p["number"]["note"], "the room number the rooms reading paired with this room's label");
    assert_eq!(device_of(&mut s, "A")["room"]["value"], "101", "the device in it gets its number");
    let sum = s.execute("bim.props.summary", &json!({})).expect("summary");
    assert_eq!(sum["kinds"]["room"]["filled"]["number"], json!(2), "{:#}", sum["kinds"]["room"]);
}

#[test]
fn a_numbered_room_the_build_did_not_trace_becomes_a_room_of_the_model() {
    // Q6002 round 4: the model's room elements come only from tags inside built walls (~69 on
    // the real set against ~200 read). A numbered room the job read and the model lacks becomes
    // a room of the model, and the devices in it take its number.
    let mut s = plan();
    let storage = {
        let d = s.doc().expect("doc");
        d.model
            .iter()
            .filter(|e| made_as(e).as_deref() == Some("room"))
            .find(|e| e.common.xdata.iter().any(|t| t.value == "number:102"))
            .map(|e| e.handle)
            .expect("storage 102's room element")
    };
    s.doc_mut().expect("doc").remove_entity(storage);
    s.execute("rooms.build", &json!({})).expect("rooms");
    let r = fill(&mut s);
    assert!(device_of(&mut s, "FA")["room"].is_null(), "no room for the device in 102 yet: {r:#}");
    let sum = s.execute("bim.props.summary", &json!({})).expect("summary");
    assert_eq!(sum["kinds"]["room"]["filled"]["number"], json!(1), "{:#}", sum["kinds"]["room"]);
    let rooms = json!([
        // Office 101 is in the model already: not added again.
        {"number": "101", "name": "OFFICE", "level": "L1", "sheet": "A-101", "at": [110.0, 64.0],
         "outline": [[6.0, 6.0], [234.0, 6.0], [234.0, 114.0], [6.0, 114.0]]},
        // The sheet names the level its own way: the room is on the model's L1.
        {"number": "102", "name": "STORAGE", "level": "LEVEL 1", "sheet": "A-101", "at": [470.0, 64.0],
         "outline": [[366.0, 6.0], [594.0, 6.0], [594.0, 114.0], [366.0, 114.0]]},
        // A label's box, not a room (under 16 sf): left out.
        {"number": "103", "name": "CLOSET", "level": "L1", "sheet": "A-101", "at": [700.0, 64.0],
         "outline": [[690.0, 60.0], [710.0, 60.0], [710.0, 70.0], [690.0, 70.0]]}
    ]);
    let r = s.execute("bim.props.fill", &json!({"rooms": rooms})).expect("fill");
    assert_eq!(r["diagnostics"]["read_rooms_added"], json!(1), "{r:#}");
    assert_eq!(device_of(&mut s, "FA")["room"]["value"], "102", "the device in 102 gets its number");
    let added = find(&mut s, "room", json!({"number": "102"}));
    assert_eq!(added.len(), 1, "{added:?}");
    let p = get(&mut s, &added[0]);
    assert_eq!(p["number"]["source"]["kind"], "print", "{p:#}");
    assert_eq!(p["level"]["value"], "L1", "{p:#}");
    let sum = s.execute("bim.props.summary", &json!({})).expect("summary");
    assert_eq!(sum["kinds"]["room"]["filled"]["number"], json!(2), "{:#}", sum["kinds"]["room"]);
    assert_eq!(sum["kinds"]["room"]["count"], json!(2), "{:#}", sum["kinds"]["room"]);
    // Filled again: nothing added twice.
    let r = s.execute("bim.props.fill", &json!({"rooms": rooms})).expect("fill");
    assert_eq!(r["diagnostics"]["read_rooms_added"], json!(0), "{r:#}");
    assert_eq!(find(&mut s, "room", json!({"number": "102"})).len(), 1);
}

#[test]
fn the_guided_job_gives_the_fill_its_rooms_reading() {
    // Q6002 round 4: the bim_build stage passes the rooms stage's reading (numbers, levels and
    // outlines) to the fill; the stage's own result stays without outlines.
    let _g = serial();
    real_pdf_reader();
    let mut s = run_job("reading", &["A-101"]);
    let rooms = s.execute("job.explain", &json!({"stage": "rooms"})).expect("explain")["result"].clone();
    let list = rooms["rooms"].as_array().cloned().unwrap_or_default();
    assert!(!list.is_empty(), "{rooms:#}");
    assert!(list.iter().all(|r| r.get("outline").is_none()), "no outlines in the stage's result");
    let build = s.execute("job.explain", &json!({"stage": "bim_build"})).expect("explain")["result"].clone();
    let diag = &build["props"]["diagnostics"];
    let read = list.iter().filter(|r| r["from_model"] != json!(true)).count() as u64;
    assert_eq!(diag["read_rooms"].as_u64(), Some(read), "{diag:#}");
    assert!(diag["read_rooms_with_outline"].as_u64().unwrap_or(0) > 0, "{diag:#}");
    assert!(diag["read_rooms_numbered"].as_u64().unwrap_or(0) > 0, "{diag:#}");
    let sum = s.execute("bim.props.summary", &json!({})).expect("summary");
    assert!(sum["kinds"]["room"]["filled"]["number"].as_u64().unwrap_or(0) > 0, "{:#}", sum["kinds"]["room"]);
}

/// A door the way the build makes one, over the built wall nearest `at` (drawing units).
fn door_at(s: &mut Session, at: Vec2) -> Handle {
    let wall = {
        let d = s.doc().expect("doc");
        d.model
            .iter()
            .filter(|e| made_as(e).as_deref() == Some("wall"))
            .map(|e| (cadaddy_engine::doc::entity_bounds(d, e, 0).center().dist(at), e.handle))
            .min_by(|a, b| a.0.total_cmp(&b.0))
            .expect("a wall")
            .1
    };
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
}

#[test]
fn a_door_with_no_tag_takes_the_number_of_the_one_numbered_room_it_opens_into() {
    // Q6002 round 5: on the real set 44 of 313 doors had a number; most print no tag beside
    // them. Doors are numbered after the room they open into, so a door with no tag between one
    // numbered room and the outside takes that number, marked as the convention's (`rule`).
    let mut s = plan();
    let into_101 = door_at(&mut s, Vec2::new(0.0, 60.0));
    // The doors stand in the rooms' outer walls, away from the room tags (a room tag within 5'
    // of a door reads as its number). Two untagged doors into storage 102: which one is 102 is not known, so neither is.
    let a_102 = door_at(&mut s, Vec2::new(360.0, 60.0));
    let b_102 = door_at(&mut s, Vec2::new(600.0, 60.0));
    let r = fill(&mut s);
    assert_eq!(r["diagnostics"]["doors_untagged"], json!(3), "{r:#}");
    assert_eq!(r["diagnostics"]["doors_numbered_by_room"], json!(1), "{r:#}");
    let p = get(&mut s, &into_101.hex());
    assert_eq!(p["number"]["value"], "101", "{p:#}");
    assert_eq!(p["number"]["source"]["kind"], "rule", "never presented as read: {p:#}");
    assert!(p["number"]["note"].as_str().is_some_and(|n| n.contains("opens into")), "{p:#}");
    assert!(get(&mut s, &a_102.hex())["number"].is_null());
    assert!(get(&mut s, &b_102.hex())["number"].is_null());
    // A second fill gives nothing again, and a tag printed at the door wins over the rule.
    let r = fill(&mut s);
    assert_eq!(r["diagnostics"]["doors_numbered_by_room"], json!(0), "{r:#}");
    let at = cadaddy_engine::doc::entity_bounds(s.doc().expect("doc"), s.doc().expect("doc").entity(into_101).expect("door"), 0).center();
    text(&mut s, "A-ANNO", [at.x + 24.0, at.y - 34.0], "101A");
    fill(&mut s);
    let p = get(&mut s, &into_101.hex());
    assert_eq!(p["number"]["value"], "101A", "{p:#}");
    assert_eq!(p["number"]["source"]["kind"], "print");
}

#[test]
fn a_door_whose_room_number_a_tagged_door_has_keeps_none() {
    // The tag says which door is 101: another untagged door into 101 is not given it again.
    let mut s = plan();
    let tagged = door_at(&mut s, Vec2::new(0.0, 60.0));
    let other = door_at(&mut s, Vec2::new(240.0, 60.0));
    let at = cadaddy_engine::doc::entity_bounds(s.doc().expect("doc"), s.doc().expect("doc").entity(tagged).expect("door"), 0).center();
    text(&mut s, "A-ANNO", [at.x + 24.0, at.y - 34.0], "101");
    let r = fill(&mut s);
    assert_eq!(get(&mut s, &tagged.hex())["number"]["source"]["kind"], "print", "{r:#}");
    assert!(get(&mut s, &other.hex())["number"].is_null(), "{r:#}");
    assert_eq!(r["diagnostics"]["doors_numbered_by_room"], json!(0), "{r:#}");
}

#[test]
fn a_device_on_a_sheet_whose_plan_sits_elsewhere_is_put_in_its_room_by_the_shared_numbers() {
    // Q6002 round 5: on the real set 83 of 1,055 devices had a room. A trade sheet the same size
    // as the architectural one, with its plan 10' further right on the paper, puts its devices
    // 10' off by the sheet frame. The room and door numbers both sheets print register it.
    let mut s = plan_with(true);
    let mut spec: Value = serde_json::from_str(s.doc().expect("doc").building_spec.as_deref().expect("spec")).expect("json");
    // Office 101 at 15', 5' on the plan; E-101 draws it 10' right of where the frame puts it.
    let mut c = device(285.0, 5.0, "C", "LP1-7", "1F5");
    c["id"] = json!("device:L1:3420.0,60.0");
    c["at_ft"] = json!([285.0, 5.0]);
    spec["tags"].as_array_mut().expect("tags").push(c);
    s.execute("bspec.set", &json!({"replace": true, "spec": spec})).expect("spec");
    s.execute("model.build", &json!({})).expect("build");
    s.execute("rooms.build", &json!({})).expect("rooms");
    // Before the numbers are printed on E-101: the frame puts C between the offices, in neither.
    fill(&mut s);
    assert!(device_of(&mut s, "C")["room"].is_null(), "{:#}", device_of(&mut s, "C"));
    // Numbers both sheets print: E-101's 3240" right of the plan's (3120" of frame, 120" more).
    text(&mut s, "A-ANNO", [300.0, 100.0], "105");
    for (x, y, t) in [(116.0, 52.0, "101"), (476.0, 52.0, "102"), (300.0, 100.0, "105")] {
        text(&mut s, "E-ANNO", [x + 3240.0, y], t);
    }
    let r = fill(&mut s);
    assert_eq!(r["diagnostics"]["sheets_registered_by_tags"], json!(1), "{r:#}");
    let p = device_of(&mut s, "C");
    assert_eq!(p["room"]["value"], "101", "{p:#}");
    assert!(p["room"]["note"].as_str().is_some_and(|n| n.contains("door and room numbers")), "{p:#}");
    assert!(r["diagnostics"]["devices_roomed_by_registration"].as_u64().unwrap_or(0) >= 1, "{r:#}");
    // B, on E-101 too, is where the frame put it: the registration agrees and keeps office 101.
    assert_eq!(device_of(&mut s, "B")["room"]["value"], "101");
}
