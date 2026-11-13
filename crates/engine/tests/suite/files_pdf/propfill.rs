//! Q6002 propfill: the guided job's readers fill the elements' properties.

use crate::helper::serial;
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
                           "properties": {"CFM": "2000", "MODEL": "XB-12"}, "sources": {"schedule": "M-601"}},
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
fn a_switch_takes_its_wiring_device_sections_product_record() {
    let mut s = plan();
    fill(&mut s);
    let dir = std::env::temp_dir().join(format!("cadaddy-products-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let spec = dir.join("26 27 26 - Wiring Devices.txt");
    std::fs::write(
        &spec,
        "SECTION 262726 - WIRING DEVICES\nPART 2 - PRODUCTS\n2.4 TOGGLE SWITCHES\nB. Switches, 120/277 V, 20 A:\n1. Single Pole:\n\
         a. Manufacturers: Subject to compliance with requirements, provide products by one of the following:\n1) Hubbell.\n2) Leviton.\n\
         2.8 FINISHES\nA. Device Color:\n1. Wiring Devices Connected to Normal Power System: white unless otherwise indicated.\nPART 3 - EXECUTION\n",
    )
    .unwrap();
    let r = s.execute("bim.products", &json!({"specs": [dir.to_string_lossy()]})).unwrap();
    assert!(r["elements"].as_u64().unwrap_or(0) >= 1, "{r:#}");
    let sw = device_of(&mut s, "S");
    assert_eq!(sw["color"]["value"], "white", "{sw:#}");
    assert_eq!(sw["color"]["source"]["kind"], "spec");
    assert_eq!(sw["rating"]["value"], "120/277 V, 20 A");
    assert!(sw["approved_makers"]["value"].as_str().is_some_and(|m| m.contains("Hubbell") && m.contains("listed makers only")), "{sw:#}");
    let product: Value = serde_json::from_str(sw["product"]["value"].as_str().unwrap_or("null")).unwrap();
    assert!(product["required"]["section"].as_str().is_some_and(|x| x.starts_with("26 27 26")), "{product:#}");
    assert_eq!(product["approved"]["none"], "no submittal for this kind");
    assert_eq!(product["resolved"]["configuration"], "single pole");
    // A kind no section speaks for and no submittal covers is listed with why.
    assert!(r["no_product_data"].as_object().is_some_and(|m| !m.is_empty()), "{r:#}");
    let _ = std::fs::remove_dir_all(&dir);
}

/// A project manual whose sections are headed `SECTION 03 3000` with the title on the next
/// line, none of them in the product divisions, and whose Division 26 points to a drawing sheet
/// for its specification (Q6295): every section is read, and the sheet is read as Division 26's
/// specification (the switch takes its product record from it).
#[test]
fn a_manual_reads_every_division_and_the_sheet_it_points_a_division_to() {
    let mut s = plan();
    fill(&mut s);
    let dir = std::env::temp_dir().join(format!("cadaddy-products-manual-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let manual = dir.join("Project Manual.txt");
    std::fs::write(
        &manual,
        "PROJECT A\nTABLE OF CONTENTS\n2.01 DIVISION 03 -- CONCRETE\nA. 03 3000 - Cast-in-Place Concrete\n\
         2.02 DIVISION 22 -- PLUMBING\nA. Refer Sheet Q8 for Plumbing Specifications\n\
         2.03 DIVISION 26 -- ELECTRICAL\nA. Refer Sheet Q-9 for Electrical Specifications\n2.04 DIVISION 27 -- COMMUNICATIONS\n\
         --- page 2 of 4 ---\nSECTION 03 3000\nCAST-IN-PLACE CONCRETE\nPART 1 GENERAL\n1.01 SUMMARY\nA. Footings.\n\
         --- page 3 of 4 ---\nSECTION 04 2000\nUNIT MASONRY\nPART 1 GENERAL\nA. See Section 03 3000 - Cast-in-Place Concrete.\n\
         --- page 4 of 4 ---\nSECTION 09 9000\nPAINTING AND COATING\nPART 1 GENERAL\n",
    )
    .unwrap();
    // The sheet: a specification printed on the drawings, read as text.
    let sheet = dir.join("sheet-q9.txt");
    std::fs::write(
        &sheet,
        "ELECTRICAL SPECIFICATIONS\nPART 2 - PRODUCTS\n2.4 TOGGLE SWITCHES\nB. Switches, 120/277 V, 20 A:\n1. Single Pole:\n\
         a. Manufacturers: Subject to compliance with requirements, provide products by one of the following:\n1) Hubbell.\n2) Leviton.\nPART 3 - EXECUTION\n",
    )
    .unwrap();
    let r = s
        .execute(
            "bim.products",
            &json!({"specs": [manual.to_string_lossy()], "sheets": [{"number": "Q9", "file": sheet.to_string_lossy(), "page": 1}]}),
        )
        .unwrap();
    let read: Vec<&str> = r["sections_read"].as_array().unwrap().iter().filter_map(Value::as_str).collect();
    assert!(read.contains(&"03 30 00 CAST-IN-PLACE CONCRETE"), "every division's section, titled from the next line: {r:#}");
    assert!(read.contains(&"04 20 00 UNIT MASONRY") && read.contains(&"09 90 00 PAINTING AND COATING"), "{r:#}");
    assert!(!read.iter().any(|x| x.starts_with("03 30 00 -")), "a reference in a sentence is no header: {read:?}");
    assert!(read.contains(&"26 ELECTRICAL (sheet Q-9)"), "the sheet is Division 26's specification: {r:#}");
    assert_eq!(r["product_sections"], 1, "{r:#}");
    let sheets = r["division_sheets"].as_array().unwrap();
    assert_eq!(sheets.len(), 2, "{r:#}");
    let e = sheets.iter().find(|d| d["division"] == "26").unwrap();
    assert_eq!((e["sheet"].as_str(), e["read"].as_bool(), e["requirements"].as_u64()), (Some("Q-9"), Some(true), Some(1)), "{r:#}");
    let p = sheets.iter().find(|d| d["division"] == "22").unwrap();
    assert_eq!(p["read"], false, "a sheet the set doesn't give isn't read: {r:#}");
    assert!(p["why"].as_str().is_some_and(|w| w.contains("not in the print set")), "{r:#}");
    let sw = device_of(&mut s, "S");
    let product: Value = serde_json::from_str(sw["product"]["value"].as_str().unwrap_or("null")).unwrap();
    assert_eq!(product["required"]["section"], "26 ELECTRICAL (sheet Q-9)", "{product:#}");
    assert!(sw["approved_makers"]["value"].as_str().is_some_and(|m| m.contains("Hubbell")), "{sw:#}");
    let _ = std::fs::remove_dir_all(&dir);
}

/// A device in no room stands in a corridor the plans trace: that is its room (the takeoff
/// counts the corridor's receptacles by it).
#[test]
fn a_device_in_a_corridor_is_in_the_corridor() {
    let mut s = plan();
    let mut sp = s.execute("bspec.get", &json!({})).expect("spec")["spec"].clone();
    if let Some(t) = sp["tags"].as_array_mut() {
        t.push(device(80.0, 5.0, "C", "", "1F9"));
    }
    sp["circulation"] = json!([{"kind": "corridor", "level": "L1", "name": "CORRIDOR", "at": [960, 60], "rects": [[[900, 0], [1100, 120]]]}]);
    s.execute("bspec.set", &json!({"replace": true, "spec": sp})).expect("spec");
    s.execute("model.build", &json!({})).expect("build");
    let r = fill(&mut s);
    let c = device_of(&mut s, "C");
    assert_eq!(c["room"]["value"], "CORRIDOR", "{c:#}");
    assert_eq!(r["devices_in_no_room"], json!({}), "{r:#}");
}

/// A general note's panelboard height to top puts each scheduled panel on a wall at its centre
/// (the top less half its box), from the note; a person's height stays.
#[test]
fn a_notes_panelboard_height_to_top_is_each_wall_panels() {
    let mut s = plan();
    let mut sp = s.execute("bspec.get", &json!({})).expect("spec")["spec"].clone();
    sp["mep"]["equipment"][1]["bottom_ft"] = json!(3.0);
    sp["mep"]["equipment"][1]["height_ft"] = json!(3.5);
    s.execute("bspec.set", &json!({"replace": true, "spec": sp})).expect("spec");
    s.execute("model.build", &json!({})).expect("build");
    let r = s
        .execute("bim.props.fill", &json!({"panel_top_in": 78.0, "panel_top_doc": "E-1 general note: PANELBOARDS 6'-6\" AFF TO TOP"}))
        .expect("fill");
    assert_eq!(r["panels_heighted"], json!(1), "{r:#}");
    let lp = find(&mut s, "equipment", json!({"type_mark": "LP1"}));
    let el = get(&mut s, &lp[0]);
    let h = &el["mount_height"];
    // A 42" box whose top is at 78": its centre at 57".
    assert_eq!(h["value"], json!(57.0), "{el:#}");
    assert_eq!(h["source"]["kind"], "note");
    // One of a typical panel's (the guest rooms' `LP1-201` of the schedule's `LP1`) too.
    let mut sp = s.execute("bspec.get", &json!({})).expect("spec")["spec"].clone();
    sp["mep"]["equipment"][1]["tag"] = json!("LP1-201");
    sp["mep"]["equipment"][1]["circuits"] = json!([]);
    sp["mep"]["panel_schedules"] = json!([{"panel": "LP1", "circuits": []}]);
    s.execute("bspec.set", &json!({"replace": true, "spec": sp})).expect("spec");
    s.execute("model.build", &json!({})).expect("build");
    let r = s.execute("bim.props.fill", &json!({"panel_top_in": 78.0})).expect("fill");
    assert_eq!(r["panels_heighted"], json!(1), "{r:#}");
}

/// A typical room's unit keeps its devices to what the build stamped and booked into it: a read
/// device standing in it is in it as a model element, but its spec tag isn't given the unit.
#[test]
fn a_units_room_is_not_written_onto_unbooked_devices_tags() {
    let mut s = plan();
    s.execute("bspec.set", &json!({"spec": {"unit_instances": [{"number": "102", "type": "Q", "hand": "normal"}]}})).expect("units");
    let r = fill(&mut s);
    assert_eq!(device_of(&mut s, "FA")["room"]["value"], "102", "the element stands in it");
    assert_eq!(r["device_tags_roomed"], json!(2), "the two outside the unit: {r:#}");
    let sp = s.execute("bspec.get", &json!({})).expect("spec");
    let rooms: Vec<String> =
        sp["spec"]["tags"].as_array().into_iter().flatten().filter_map(|t| t["fields"]["room"]["value"].as_str().map(str::to_string)).collect();
    assert!(!rooms.iter().any(|r| r == "102"), "{rooms:?}");
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
    // bim.element's host is the wall the build put it on, measured.
    let ha = find(&mut s, "device", json!({"type_mark": "A"}));
    let el = s.execute("bim.element", &json!({"handle": ha[0]})).unwrap();
    assert_eq!(el["host"]["wall"], a["host_wall"]["value"], "{el:#}");
    assert!(el["host"]["stored_wall"].is_null(), "the same wall: {el:#}");
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
    // The spec's read devices carry their rooms too (the takeoff counts by room).
    assert_eq!(r["device_tags_roomed"], json!(4), "{r:#}");
    let sp = s.execute("bspec.get", &json!({})).expect("spec");
    let rooms: Vec<String> = sp["spec"]["tags"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|t| t["id"].as_str().is_some_and(|i| i.starts_with("device:")))
        .filter_map(|t| t["fields"]["room"]["value"].as_str().map(str::to_string))
        .collect();
    assert_eq!(rooms.len(), 4, "{rooms:?}");
    assert!(rooms.iter().any(|r| r == "101") && rooms.iter().any(|r| r == "102"), "{rooms:?}");
    // No circuit printed: none presented.
    let j = device_of(&mut s, "J");
    assert!(j["circuit"].is_null() && j["fed_from"].is_null(), "{j:#}");
}

/// A receptacle the print or a sink calls GFCI on a GFCI breaker goes on the bill of materials as
/// a standard one: the take-off follows the code rules' decision. A receptacle in a guest room is
/// bought tamper-resistant.
#[test]
fn the_take_off_buys_what_the_code_rules_decided() {
    let mut s = plan();
    let got = s.execute("bspec.get", &json!({})).unwrap();
    let mut tags = got["spec"]["tags"].as_array().unwrap().clone();
    let a = tags.iter_mut().find(|t| t["fields"]["type"]["value"] == "A").unwrap();
    a["kind"] = json!("receptacle");
    a["fields"]["device_type"] = json!({"value": "GFCI receptacle", "source": "rule"});
    let sched = json!({"tags": tags, "mep": {"panel_schedules": [{"panel": "LP1", "source": "E-601", "legend": {"GF": "GROUND FAULT INTERRUPTER", "AF": "ARC FAULT"},
        "circuits": [{"number": "3", "description": "RECEPTACLES OFFICE 101", "breaker": "20", "poles": 1, "protection": ["AF", "GF"], "source": "E-601"}]}]}});
    s.execute("bspec.set", &json!({"spec": sched})).expect("schedules");
    fill(&mut s);
    let ha = find(&mut s, "device", json!({"type_mark": "A"}));
    s.execute("bim.props.set", &json!({"handle": ha[0], "key": "kind", "value": "receptacle"})).unwrap();
    s.execute("bim.coderules", &json!({})).unwrap();
    // Its line lists its element (fix-bom-device-lines-ignore-element-edits).
    let id = ha[0].clone();
    let item = |s: &mut Session| -> String {
        let b = s.execute("bim.bom", &json!({})).unwrap();
        let on = |l: &&Value| l["category"] == "devices" && l["elements"].as_array().is_some_and(|e| e.iter().any(|x| x == id.as_str()));
        b["lines"].as_array().unwrap().iter().find(on).unwrap()["item"].as_str().unwrap().to_string()
    };
    assert_eq!(item(&mut s), "duplex receptacle");
    s.execute("bspec.set", &json!({"spec": {"unit_instances": [{"number": "101", "type": "Q", "hand": "normal"}]}})).unwrap();
    s.execute("bim.coderules", &json!({})).unwrap();
    assert_eq!(item(&mut s), "duplex receptacle, tamper-resistant");
}

/// The project says what was built: every breaker of a panel AF/GF except its PTAC's, over a
/// schedule printing AF. Receptacles under them are standard; the print stays in the note.
#[test]
fn breakers_as_built_outrank_the_printed_schedule() {
    let mut s = plan();
    let sched = json!({"mep": {"panel_schedules": [{"panel": "LP1", "source": "E-601", "legend": {"GF": "GROUND FAULT INTERRUPTER", "AF": "ARC FAULT"},
        "circuits": [{"number": "3", "description": "RECEPTACLES OFFICE 101", "breaker": "20", "poles": 1, "protection": ["AF"], "source": "E-601"},
                     {"number": "5", "description": "PTAC-1", "breaker": "20", "poles": 2, "protection": [], "source": "E-601"},
                     {"number": "7", "description": "SPACE ONLY", "breaker": "", "poles": 1, "protection": [], "source": "E-601"}]}]}});
    s.execute("bspec.set", &json!({"spec": sched})).expect("schedules");
    fill(&mut s);
    assert_eq!(device_of(&mut s, "A")["gfci_breaker"]["value"], "no", "as printed");
    s.execute(
        "project.rules.set",
        &json!({"domain": "custom", "key": "breaker_protection.lp1", "value": "AF/GF except PTAC", "tier": "field_verified", "source": "field report 2026-11-05", "reason": "seen in the panels"}),
    )
    .unwrap();
    fill(&mut s);
    let a = device_of(&mut s, "A");
    assert_eq!(a["gfci_breaker"]["value"], "yes", "{a:#}");
    assert_eq!(a["breaker_type"]["value"], "GFCI + AFCI (dual function)");
    assert_eq!(a["breaker_type"]["source"]["doc"], "field report 2026-11-05");
    assert!(a["breaker_type"]["note"].as_str().is_some_and(|n| n.contains("prints AF")), "{a:#}");
    let ha = find(&mut s, "device", json!({"type_mark": "A"}));
    s.execute("bim.props.set", &json!({"handle": ha[0], "key": "kind", "value": "receptacle"})).unwrap();
    s.execute("bim.coderules", &json!({})).unwrap();
    let a = device_of(&mut s, "A");
    assert_eq!(a["receptacle_type"]["value"], "standard");
    // The breaker as built is a person's word; what the rule decides from it is the rule's.
    assert_eq!(a["gfci"]["value"], "breaker", "{a:#}");
    for k in ["gfci", "protected_by", "receptacle_type"] {
        assert_eq!(a[k]["source"]["kind"], "rule", "{k}: {a:#}");
    }
    assert!(a["gfci"]["source"]["doc"].as_str().is_some_and(|d| d.contains("field report 2026-11-05")), "the breaker's source cited: {a:#}");
    // The print's GFCI tag on that breaker: kept, redundant protection reported (allowed), nothing
    // corrected (fix-coderules-drops-printed-gfci).
    s.execute("bim.props.set", &json!({"handle": ha[0], "key": "type_mark", "value": "GFCI"})).unwrap();
    let r = s.execute("bim.coderules", &json!({})).unwrap();
    assert_eq!(r["gfci"]["redundant_protection"], 1, "{r:#}");
    let hs = find(&mut s, "device", json!({"type_mark": "GFCI"}));
    let a = get(&mut s, &hs[0]);
    assert_eq!(a["receptacle_type"]["value"], "GFCI", "{a:#}");
    assert_eq!(a["gfci"]["value"], "device", "{a:#}");
    assert!(a["corrected_value"].is_null() && a["design_value"].is_null(), "{a:#}");
    // The panel's schedule says so too: what was built, and what was printed.
    let r = s.execute("panels.schedule", &json!({"panel": "lp1"})).unwrap();
    let c = |n: &str| r["panels"][0]["circuits"].as_array().unwrap().iter().find(|c| c["number"] == n).cloned().unwrap();
    assert_eq!(c("3")["protection"]["ground_fault"], true, "{r:#}");
    assert_eq!(c("3")["protection"]["codes"], json!(["AF"]));
    assert_eq!(c("3")["protection"]["as_built"]["source"], "field report 2026-11-05");
    assert!(c("5")["protection"]["as_built"].is_null(), "{r:#}");
    assert!(c("7")["protection"]["as_built"].is_null(), "a space has no breaker: {r:#}");
}

/// A receptacle the drawings tag GFCI on a dual-function breaker stays a GFCI device: redundant
/// protection is allowed and reported for value engineering, never corrected to standard; the
/// values the rule derives are the rule's, never a person's (fix-coderules-drops-printed-gfci).
#[test]
fn a_drawn_gfci_receptacle_on_a_gfci_breaker_stays_gfci() {
    let mut s = plan();
    let sched = json!({"mep": {"panel_schedules": [{"panel": "LP1", "source": "E-601", "legend": {"GF": "GROUND FAULT INTERRUPTER", "AF": "ARC FAULT"},
        "circuits": [{"number": "3", "description": "RECEPTACLES OFFICE 101", "breaker": "20", "poles": 1, "protection": ["AF", "GF"], "source": "E-601"}]}]}});
    s.execute("bspec.set", &json!({"spec": sched})).expect("schedules");
    fill(&mut s);
    let ha = find(&mut s, "device", json!({"type_mark": "A"}));
    let h = ha[0].clone();
    s.execute("bim.props.set", &json!({"handle": h, "key": "kind", "value": "receptacle"})).unwrap();
    s.execute("bim.props.set", &json!({"handle": h, "key": "type_mark", "value": "GFCI"})).unwrap();
    for run in 0..2 {
        let r = s.execute("bim.coderules", &json!({})).unwrap();
        assert_eq!(r["gfci"]["redundant_protection"], 1, "run {run}: {r:#}");
        assert!(r["gfci"]["breaker"].is_null(), "not built standard: {r:#}");
        assert_eq!(r["corrections"], 0, "{r:#}");
        let red = &r["redundant_protection"][0];
        assert_eq!(red["allowed"], true, "{r:#}");
        assert!(red["handle"].as_str().is_some_and(|x| x.eq_ignore_ascii_case(&h)), "{r:#}");
        assert!(red["note"].as_str().is_some_and(|n| n.contains("redundant protection (allowed)")), "{r:#}");
        let a = get(&mut s, &h);
        assert_eq!(a["receptacle_type"]["value"], "GFCI", "the drawn device stays: {a:#}");
        assert_eq!(a["gfci"]["value"], "device", "{a:#}");
        assert!(a["protected_by"]["value"].as_str().is_some_and(|p| p.contains("breaker")), "{a:#}");
        for k in ["design_value", "corrected_value", "correction_reason", "correction_rule"] {
            assert!(a[k].is_null(), "nothing corrected ({k}): {a:#}");
        }
        for k in ["gfci", "protected_by", "receptacle_type"] {
            assert_ne!(a[k]["source"]["kind"], "user", "no person set {k}: {a:#}");
        }
        assert!(a["gfci"]["source"]["doc"].as_str().is_some_and(|d| d.contains("NEC 210.8") && d.contains("E-601")), "rule and row: {a:#}");
    }
    // Validate agrees: a pass, redundant protection allowed, not double protection to remove.
    let v = s.execute("validate.prescreen", &json!({"rules": ["G-PRODUCT-CODE"], "all": true, "limit": 50})).unwrap();
    let f = v["findings"]
        .as_array()
        .and_then(|fs| fs.iter().find(|f| f["handle"].as_str().is_some_and(|x| x.eq_ignore_ascii_case(&h)) && f["check"] == "gfci"))
        .cloned()
        .unwrap_or_else(|| panic!("{v:#}"));
    assert_eq!(f["verdict"], "pass", "{f:#}");
    assert!(f["issue"].as_str().is_some_and(|i| i.contains("redundant protection (allowed)")), "{f:#}");
}

#[test]
fn a_devices_breaker_type_is_its_printed_schedule_rows() {
    let mut s = plan();
    // The schedule as printed: circuit 3 a dual-function breaker (AF/GF, the legend's GF a
    // ground fault interrupter), 5 an AFCI one.
    let sched = json!({"mep": {"panel_schedules": [{"panel": "LP1", "source": "E-601", "legend": {"GF": "GROUND FAULT INTERRUPTER", "AF": "ARC FAULT"},
        "circuits": [{"number": "3", "description": "RECEPTACLES OFFICE 101", "breaker": "20", "poles": 1, "protection": ["AF", "GF"], "source": "E-601"},
                     {"number": "5", "description": "LIGHTS", "breaker": "20", "poles": 1, "protection": ["AF"], "source": "E-601"}]}]}});
    s.execute("bspec.set", &json!({"spec": sched})).expect("schedules");
    fill(&mut s);
    let a = device_of(&mut s, "A");
    assert_eq!(a["breaker_type"]["value"], "GFCI + AFCI (dual function)", "{a:#}");
    assert_eq!(a["gfci_breaker"]["value"], "yes");
    assert_eq!(a["breaker_type"]["source"]["kind"], "schedule");
    assert_eq!(a["breaker_type"]["source"]["doc"], "E-601 panel LP1 circuit 3");
    // The breaker itself still from the panel's own row.
    assert_eq!(a["breaker"]["value"], "20A");
    // The schedule as data: each circuit's breaker type and the model's devices on it; as CSV;
    // as a CADaddy table.
    let csv = std::env::temp_dir().join(format!("cadaddy-panels-{}.csv", std::process::id()));
    let r = s.execute("panels.schedule", &json!({"panel": "lp1", "csv": csv.to_string_lossy(), "place": {"panel": "LP1", "at": [0, -100]}})).unwrap();
    let c3 = r["panels"][0]["circuits"].as_array().unwrap().iter().find(|c| c["number"] == "3").cloned().unwrap();
    assert_eq!(c3["protection"]["ground_fault"], true, "{r:#}");
    assert_eq!(c3["protection"]["meaning"][1], "GF: GROUND FAULT INTERRUPTER");
    assert_eq!(c3["breaker_amps"], json!(20.0));
    assert!(c3["devices"]["count"].as_u64().unwrap_or(0) >= 1, "{c3:#}");
    assert!(std::fs::read_to_string(&csv).unwrap().contains("LP1,,E-601,3,RECEPTACLES OFFICE 101,20,1,,,AF/GF,true,true,"));
    let _ = std::fs::remove_file(&csv);
    assert!(!r["placed"].is_null(), "{r:#}");
    s.execute("undo", &json!({})).unwrap();
    // The code rules: under a GFCI breaker it is a standard receptacle, protected by its breaker;
    // the office is no guest room (no TR rule), and the documents don't say TR.
    let ha = find(&mut s, "device", json!({"type_mark": "A"}));
    s.execute("bim.props.set", &json!({"handle": ha[0], "key": "kind", "value": "receptacle"})).unwrap();
    let r = s.execute("bim.coderules", &json!({})).unwrap();
    assert_eq!(r["gfci"]["breaker"], 1, "{r:#}");
    let a = device_of(&mut s, "A");
    assert_eq!(a["gfci"]["value"], "breaker", "{a:#}");
    assert_eq!(a["receptacle_type"]["value"], "standard");
    assert_eq!(a["receptacle_type"]["source"]["kind"], "rule");
    // (The print doesn't tag it GFCI, so nothing was corrected.)
    assert!(a["corrected_value"].is_null(), "{a:#}");
    assert!(a["tamper_resistant"].is_null(), "{a:#}");
    assert_eq!(a["tr_documented"]["value"], "not stated in the documents");
    // As a guest room (a typical unit fitted into it), every receptacle is TR.
    s.execute("bspec.set", &json!({"spec": {"unit_instances": [{"number": "101", "type": "Q", "hand": "normal"}]}})).unwrap();
    let r = s.execute("bim.coderules", &json!({})).unwrap();
    assert!(r["tamper_resistant"].as_u64().unwrap_or(0) >= 1, "{r:#}");
    let a = device_of(&mut s, "A");
    assert_eq!(a["tamper_resistant"]["value"], "yes", "{a:#}");
    assert!(a["tamper_resistant"]["source"]["doc"].as_str().is_some_and(|d| d.contains("406.12") && d.contains("101")), "{a:#}");
    // Its product follows: the section's tamper-resistant receptacle, where the documents say TR.
    let dir = std::env::temp_dir().join(format!("cadaddy-products-tr-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("26 27 26 - Wiring Devices.txt"),
        "SECTION 262726 - WIRING DEVICES\nPART 2 - PRODUCTS\n2.2 STRAIGHT-BLADE RECEPTACLES\nA. Convenience Receptacles: 125 V, 20 A; comply with NEMA WD 1.\n\
         1. Manufacturers: Subject to compliance with requirements, provide products by one of the following:\na. Hubbell.\nb. Leviton.\n\
         B. Tamper-Resistant Convenience Receptacles: 125 V, 20 A; comply with NEMA WD 1.\n\
         1. Manufacturers: Subject to compliance with requirements, provide products by one of the following:\na. Hubbell.\nb. Leviton.\nPART 3 - EXECUTION\n",
    )
    .unwrap();
    s.execute("bim.products", &json!({"specs": [dir.to_string_lossy()]})).unwrap();
    let a = device_of(&mut s, "A");
    let product: Value = serde_json::from_str(a["product"]["value"].as_str().unwrap_or("null")).unwrap();
    assert!(product["required"]["product"].as_str().is_some_and(|p| p.to_ascii_uppercase().contains("TAMPER")), "{product:#}");
    assert!(product["resolved"]["features"].as_array().is_some_and(|f| f.iter().any(|x| x == "TR")), "{product:#}");
    assert!(a["tr_documented"]["value"].as_str().is_some_and(|d| d.starts_with("26 27 26") && d.to_ascii_uppercase().contains("TAMPER")), "{a:#}");
    assert_eq!(a["tr_documented"]["source"]["kind"], "spec");
    let _ = std::fs::remove_dir_all(&dir);
    // Beside a sink on an AFCI-only breaker, the untagged receptacle needs GFCI: the circuit's
    // first (here its only one) is the GFCI device.
    let sched = json!({"mep": {"panel_schedules": [{"panel": "LP1", "source": "E-601", "legend": {"GF": "GROUND FAULT INTERRUPTER", "AF": "ARC FAULT"},
        "circuits": [{"number": "3", "description": "RECEPTACLES OFFICE 101", "breaker": "20", "poles": 1, "protection": ["AF"], "source": "E-601"}]}]}});
    s.execute("bspec.set", &json!({"spec": sched})).unwrap();
    fill(&mut s);
    let a = device_of(&mut s, "A");
    assert_eq!(a["gfci_breaker"]["value"], "no", "{a:#}");
    let at: Vec<f64> = a["location"]["value"].as_str().unwrap().split(',').map(|v| v.trim().parse().unwrap()).collect();
    let sink = find(&mut s, "fixture", json!({}));
    assert!(!sink.is_empty(), "the plan's diffuser");
    let level = a["level"]["value"].clone();
    for (k, v) in [("type", json!("SINK")), ("location", json!(format!("{:.2}, {:.2}, 3.00", at[0] + 4.0, at[1]))), ("level", level)] {
        s.execute("bim.props.set", &json!({"handle": sink[0], "key": k, "value": v})).unwrap();
    }
    let r = s.execute("bim.coderules", &json!({})).unwrap();
    let a = device_of(&mut s, "A");
    let sk = get(&mut s, &sink[0]);
    assert_eq!(a["gfci"]["value"], "device", "{r:#} {a:#} SINK {sk:#}");
    assert_eq!(a["receptacle_type"]["value"], "GFCI");
    assert!(a["receptacle_type"]["source"]["doc"].as_str().is_some_and(|d| d.contains("needs GFCI") && d.contains("sink")), "{a:#}");
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
fn a_sconces_height_measured_for_it_stands_and_a_receptacles_note_by_it_does_not() {
    let mut s = plan();
    // Two wall luminaires by their submittals: one with the height its room type's elevations
    // draw the wall luminaires at (5'-0"), one with a 44" note its typical plan prints by it (a
    // receptacle's beside it).
    let mut spec = s.execute("bspec.get", &json!({})).expect("spec")["spec"].clone();
    if let Some(t) = spec["tags"].as_array_mut() {
        let mut w1 = device(15.0, 9.2, "W1", "", "1FD");
        w1["fields"]["mount"] = json!({"value": "wall", "source": "submittal"});
        w1["fields"]["mount_height_in"] = json!({"value": "60", "source": "note"});
        w1["fields"]["mount_height_note"] = json!({
            "value": "60\" AFF to its middle: the type's interior elevations draw its wall luminaires there (A-2 SUITE - BED WALL)",
            "source": "note"
        });
        t.push(w1);
        let mut w2 = device(20.0, 9.2, "W2", "", "1FE");
        w2["fields"]["mount"] = json!({"value": "wall", "source": "submittal"});
        w2["fields"]["mount_height_in"] = json!({"value": "44", "source": "note"});
        w2["fields"]["mount_height_note"] = json!({"value": "44\" AFF on A-2", "source": "note"});
        t.push(w2);
    }
    s.execute("bspec.set", &json!({"replace": true, "spec": spec})).expect("spec");
    s.execute("model.build", &json!({})).expect("build");
    let w1 = device_of(&mut s, "W1");
    assert_eq!(w1["mount_height"]["value"], json!(60.0), "{w1:#}");
    let w2 = device_of(&mut s, "W2");
    assert_eq!(w2["mount_height"]["value"], json!(90.0), "the wall luminaire rule, not the receptacle's note: {w2:#}");
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
            // The route height was estimated: a default, never a reading. The build raises it
            // into the plenum, 3" over the level's 9' ceiling (M-TERMINAL-CEILING).
            assert_eq!(p["elevation"]["source"]["kind"], "default", "{p:#}");
            assert_eq!(p["elevation"]["value"], json!(9.25));
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
    // Its schedule row's model, from the schedule (Q6313).
    assert_eq!(a["model"]["value"], "XB-12", "{a:#}");
    assert_eq!(a["model"]["source"]["kind"], "schedule");
    assert_eq!(a["model"]["source"]["doc"], "M-601");
}

/// A door made like the build makes them in office 101's south wall (nothing printed by it);
/// its handle and plan middle.
fn door_in_south_wall(s: &mut Session) -> (Handle, Vec2) {
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
    (door, at)
}

fn door_schedule_findings(s: &mut Session) -> Vec<Value> {
    let r = s.execute("bim.props.check", &json!({"checks": ["door_schedule"]})).expect("check");
    r["findings"].as_array().cloned().unwrap_or_default()
}

/// No door schedule and no number printed: the door takes the number of the room it serves,
/// marked as a rule's with its basis, and the missing schedule is a finding.
#[test]
fn a_door_nothing_numbers_takes_its_rooms_number() {
    let mut s = plan();
    let (door, _) = door_in_south_wall(&mut s);
    let r = fill(&mut s);
    let dp = get(&mut s, &door.hex());
    assert_eq!(dp["number"]["value"], "101", "{dp:#}");
    assert_eq!(dp["number"]["source"]["kind"], "rule", "{dp:#}");
    assert!(dp["number"]["note"].as_str().unwrap_or("").contains("only door of room 101"), "{dp:#}");
    assert_eq!(r["doors"]["numbered_by_room"], json!(1), "{r}");
    let f = door_schedule_findings(&mut s);
    assert_eq!(f.len(), 1, "{f:?}");
    assert!(f[0]["message"].as_str().unwrap_or("").contains("no door schedule"), "{f:?}");
}

/// A door schedule's tag by the door names its row: the door takes the row's number, type,
/// frame, fire rating, hardware set and size, each from the schedule.
#[test]
fn a_door_takes_its_door_schedule_row() {
    let mut s = plan();
    let (door, at) = door_in_south_wall(&mut s);
    let schedule = json!({
        "rows": [{"kind": "door", "mark": "7", "width_in": 34.0, "height_in": 84.0, "type": "B", "frame": "HM", "fire_rating": "20 MIN", "hardware": "HW-2"},
                 {"kind": "door", "mark": "8", "width_in": 36.0}],
        "tags": [{"mark": "7", "at": [at.x + 6.0, at.y + 30.0]}]
    });
    s.execute("bspec.set", &json!({"spec": {"schedule": schedule}})).expect("schedule");
    fill(&mut s);
    let dp = get(&mut s, &door.hex());
    assert_eq!(dp["number"]["value"], "7", "{dp:#}");
    for (key, want) in
        [("type", json!("B")), ("frame", json!("HM")), ("fire_rating", json!("20 MIN")), ("hardware_set", json!("HW-2")), ("width", json!(34.0))]
    {
        assert_eq!(dp[key]["value"], want, "{key}: {dp:#}");
        assert_eq!(dp[key]["source"]["kind"], "schedule", "{key}: {dp:#}");
    }
    assert!(door_schedule_findings(&mut s).is_empty());
}

/// A door made like the build makes them in office 101's south wall, from `x0` to `x1` (inches);
/// its handle and plan middle.
fn door_from_to(s: &mut Session, x0: f64, x1: f64) -> (Handle, Vec2) {
    let (door, _) = door_in_south_wall(s);
    let d = s.doc_mut().expect("doc");
    let mut e = (**d.entity(door).expect("door")).clone();
    let EntityKind::Mesh(m) = &mut e.kind else { panic!("the build's wall is a mesh") };
    let (lo, hi) = m.vertices.iter().fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), v| (lo.min(v.x), hi.max(v.x)));
    for v in &mut m.vertices {
        v.x = x0 + (v.x - lo) / (hi - lo) * (x1 - x0);
    }
    assert!(d.model.replace(e), "the door is in the model");
    let at = cadaddy_engine::doc::entity_bounds(s.doc().expect("doc"), s.doc().expect("doc").entity(door).expect("door"), 0).center();
    (door, at)
}

/// Q6454: a schedule tag names one opening. Of two doors within its reach the nearer takes its
/// row; the other, with nothing printed by it, is numbered from its room and keeps its own
/// width.
#[test]
fn a_schedule_tag_names_one_door_the_nearest() {
    let mut s = plan();
    // Door A 36" wide, middle 12" from the tag; door B 30" wide, 48" from it.
    let (a, at_a) = door_from_to(&mut s, 42.0, 78.0);
    let (b, at_b) = door_from_to(&mut s, 105.0, 135.0);
    assert!((at_b.x - at_a.x - 60.0).abs() < 1e-6, "{at_a:?} {at_b:?}");
    let schedule = json!({
        "rows": [{"kind": "door", "mark": "7", "width_in": 36.0, "hardware": "HW-2"},
                 {"kind": "door", "mark": "8", "width_in": 32.0, "hardware": "HW-3"}],
        "tags": [{"mark": "7", "at": [at_a.x + 12.0, at_a.y]}]
    });
    s.execute("bspec.set", &json!({"spec": {"schedule": schedule}})).expect("schedule");
    let r = fill(&mut s);
    let ap = get(&mut s, &a.hex());
    assert_eq!(ap["number"]["value"], "7", "{ap:#}");
    assert_eq!(ap["hardware_set"]["value"], "HW-2", "{ap:#}");
    assert_eq!(ap["width"]["value"], json!(36.0), "{ap:#}");
    let bp = get(&mut s, &b.hex());
    assert_eq!(bp["number"]["value"], "101", "the room's only unnumbered door: {bp:#}");
    assert_eq!(bp["number"]["source"]["kind"], "rule", "{bp:#}");
    assert!(bp["hardware_set"].is_null(), "no row of its own: {bp:#}");
    assert_eq!(bp["width"]["value"], json!(30.0), "{bp:#}");
    assert_eq!(bp["width"]["source"]["kind"], "computed", "{bp:#}");
    assert_eq!(r["doors"]["numbered_by_tag"], json!(1), "{r}");
    assert_eq!(r["doors"]["with_schedule_row"], json!(1), "{r}");
    assert_eq!(r["doors"]["numbered_by_room"], json!(1), "{r}");
}

/// Q6454: a schedule tag of another level names none of this level's doors, however near.
#[test]
fn a_schedule_tag_of_another_level_names_no_door() {
    let mut s = plan();
    let (b, at_b) = door_from_to(&mut s, 105.0, 135.0);
    let schedule = json!({
        "rows": [{"kind": "door", "mark": "8", "width_in": 32.0, "hardware": "HW-3"}],
        "tags": [{"mark": "8", "level": "L2", "at": [at_b.x, at_b.y + 6.0]}]
    });
    s.execute("bspec.set", &json!({"spec": {"schedule": schedule}})).expect("schedule");
    fill(&mut s);
    let bp = get(&mut s, &b.hex());
    assert_eq!(bp["number"]["value"], "101", "{bp:#}");
    assert!(bp["hardware_set"].is_null(), "{bp:#}");
}

/// Q6454: a number printed between two doors numbers the nearer one only, and its schedule tag
/// (the same mark at the same place) is not taken again by the other.
#[test]
fn a_printed_door_number_names_one_door_the_nearest() {
    let mut s = plan();
    // The number 12" from door A's middle, 30" from door B's: within both doors' reach.
    let (a, at_a) = door_from_to(&mut s, 42.0, 78.0);
    let (b, at_b) = door_from_to(&mut s, 87.0, 117.0);
    let at = [at_a.x + 12.0, at_a.y];
    assert!(Vec2::new(at[0], at[1]).dist(at_b) <= 36.0);
    text(&mut s, "A-ANNO", at, "7");
    let schedule = json!({"rows": [{"kind": "door", "mark": "7", "width_in": 36.0, "hardware": "HW-2"}], "tags": [{"mark": "7", "at": at}]});
    s.execute("bspec.set", &json!({"spec": {"schedule": schedule}})).expect("schedule");
    fill(&mut s);
    let ap = get(&mut s, &a.hex());
    assert_eq!(ap["number"]["value"], "7", "{ap:#}");
    assert_eq!(ap["number"]["source"]["kind"], "print", "{ap:#}");
    assert_eq!(ap["hardware_set"]["value"], "HW-2", "{ap:#}");
    let bp = get(&mut s, &b.hex());
    assert_eq!(bp["number"]["value"], "101", "{bp:#}");
    assert!(bp["hardware_set"].is_null(), "{bp:#}");
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

/// Two rooms inside 6" outer walls (x 0–40.5', y 0–10') split by a 6" partition; receptacles read
/// on the outer walls' centrelines, where nothing (no symbol side, no circuit) says which face
/// they serve, and one on the partition's.
fn centreline_plan(extra: &[Value]) -> Session {
    let mut d = Drawing::new_imperial();
    let outer = [((0., 0.), (40.5, 0.)), ((40.5, 0.), (40.5, 10.)), ((40.5, 10.), (0., 10.)), ((0., 10.), (0., 0.))];
    let left = [((0.5, 0.5), (19.75, 0.5)), ((19.75, 0.5), (19.75, 9.5)), ((19.75, 9.5), (0.5, 9.5)), ((0.5, 9.5), (0.5, 0.5))];
    let right = [((20.25, 0.5), (40.0, 0.5)), ((40.0, 0.5), (40.0, 9.5)), ((40.0, 9.5), (20.25, 9.5)), ((20.25, 9.5), (20.25, 0.5))];
    for (a, b) in outer.into_iter().chain(left).chain(right) {
        line(&mut d, "A-WALL", a, b);
    }
    let mut s = Session::empty();
    s.open_drawing(d, "Centreline Devices", None);
    text(&mut s, "A-ROOM-TAG", [110.0, 64.0], "OFFICE");
    text(&mut s, "A-ROOM-TAG", [116.0, 52.0], "101");
    text(&mut s, "A-ROOM-TAG", [350.0, 64.0], "STORAGE");
    text(&mut s, "A-ROOM-TAG", [356.0, 52.0], "102");
    let mut tags = vec![
        device(10.0, 0.25, "S", "", "1F0"),
        device(30.0, 9.75, "N", "", "1F1"),
        device(0.25, 5.0, "W", "", "1F2"),
        device(40.25, 5.0, "X", "", "1F3"),
        device(20.0, 3.0, "P", "", "1F4"),
    ];
    tags.extend(extra.iter().cloned());
    let spec = json!({
        "version": 1, "units_per_foot": 12,
        "levels": [{"name": "L1", "elevation_ft": 0, "height_ft": 10, "ceiling_ft": 9, "base": [0, 0]}],
        "layers": [{"layer": "A-WALL", "role": "wall", "wall_type": "Partition"}, {"layer": "A-ROOM-TAG", "role": "room_tag"}],
        "wall_types": [{"name": "Partition", "thickness_in": 6}],
        "tags": tags
    });
    s.execute("bspec.set", &json!({"replace": true, "spec": spec})).expect("spec");
    s.execute("model.build", &json!({})).expect("build");
    s.execute("rooms.build", &json!({})).expect("rooms");
    s
}

/// A receptacle on an outer wall's centreline faces the room inside, never the outside past the
/// wall's other face (Q6387): nothing said a side, so the side a room is on is the one it serves,
/// said so on its placement; each is in its room, none in no room.
#[test]
fn a_device_on_an_outer_walls_centreline_faces_the_room_inside() {
    let _g = serial();
    let mut s = centreline_plan(&[]);
    let r = fill(&mut s);
    for (mark, room) in [("S", "101"), ("N", "102"), ("W", "101"), ("X", "102")] {
        let el = device_of(&mut s, mark);
        assert_eq!(el["room"]["value"], room, "{mark}: {el:#}\n{r:#}");
        let placed = el["placement"]["value"].as_str().unwrap_or("");
        assert!(placed.contains("the side a room or corridor is on"), "{mark}: {placed}");
        assert!(el["room_review"].is_null(), "{mark}: {el:#}");
    }
    let p = device_of(&mut s, "P");
    assert!(matches!(p["room"]["value"].as_str(), Some("101" | "102")), "the partition's: {p:#}");
    assert!(r["devices_in_no_room"].as_object().is_none_or(|m| m.is_empty()), "{r:#}");
    assert!(r["devices_in_no_room_listed"].as_array().is_some_and(Vec::is_empty), "{r:#}");
}

/// A device whose symbol the plan draws past an outer wall's outside face (an outside receptacle)
/// stays on that face, as drawn, and is in no room: a finding with its reason on the device and
/// in the fill's report, never '(no room)' silently (Q6387).
#[test]
fn a_device_on_no_rooms_face_is_a_finding_with_its_reason() {
    use cadaddy_engine::cmd::propfill::{NO_ROOM_BEHIND, NO_ROOM_OFF_WALL, ROOM_REVIEW};
    let _g = serial();
    // O: 0.6' south of the south wall's outer face; F: a device of no known kind 5' out in the
    // open, on no wall.
    let mut s = centreline_plan(&[device(10.0, -0.6, "O", "", "1F5"), device(30.0, -5.0, "F", "", "1F6")]);
    let r = fill(&mut s);
    let o = device_of(&mut s, "O");
    assert!(o["room"]["value"].is_null(), "outside: {o:#}");
    assert!(o["face_normal"]["value"].as_str().is_some_and(|n| n.ends_with("-1.000")), "the drawn, outside face: {o:#}");
    let review = o[ROOM_REVIEW]["value"].as_str().unwrap_or("");
    assert!(review.starts_with("review: in no room") && review.contains("room 101 is behind its wall"), "{review}");
    let f = device_of(&mut s, "F");
    assert!(f[ROOM_REVIEW]["value"].as_str().is_some_and(|t| t.contains(NO_ROOM_OFF_WALL)), "{f:#}");
    assert_eq!(r["devices_in_no_room"], json!({"L1": 2}), "{r:#}");
    assert_eq!(r["devices_in_no_room_why"], json!({NO_ROOM_BEHIND: 1, NO_ROOM_OFF_WALL: 1}), "{r:#}");
    let listed = r["devices_in_no_room_listed"].as_array().cloned().unwrap_or_default();
    assert_eq!(listed.len(), 2, "{r:#}");
    assert!(listed.iter().all(|x| x["handle"].is_string() && x["why"].is_string() && x["at_ft"].is_array()), "{r:#}");
    // Listed where the plan has it.
    let o_at = listed.iter().find(|x| x["why"].as_str().is_some_and(|w| w.contains("behind"))).and_then(|x| x["at_ft"].as_array().cloned());
    assert!(o_at.is_some_and(|a| a.first().and_then(Value::as_f64).is_some_and(|x| (x - 10.0).abs() < 0.5)), "{r:#}");
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

/// The built element tagged `tag`.
fn tagged<'a>(d: &'a Drawing, tag: &str) -> &'a Entity {
    let want = format!("tag:{tag}");
    d.model.iter().find(|e| e.common.xdata.iter().any(|t| t.value == want)).map(|e| e.as_ref()).expect("built")
}

fn prop_of(e: &Entity, key: &str) -> Option<String> {
    let head = format!("prop:{key}=");
    e.common.xdata.iter().find_map(|t| t.value.strip_prefix(&head).map(str::to_string))
}

/// A through-wall unit (a PTAC) whose plan symbol stands 8" in front of an outside wall is built
/// through that wall (Q6546, M-THROUGH-WALL): hosted in it, its sleeve flush with the outside face
/// and its cabinet projecting into the room, a sleeve and an outside grille built for the
/// takeoff, the wall cut for the sleeve, its drawn place kept beside the seated one, and G-HOST
/// judges it in its wall. One drawn where no outside wall is near is a review item, not a box
/// floating in the room silently.
#[test]
fn a_ptac_drawn_in_front_of_an_outside_wall_is_built_through_it() {
    let _g = serial();
    let mut s = plan();
    let mut spec = s.execute("bspec.get", &json!({})).expect("spec")["spec"].clone();
    if let Some(eq) = spec["mep"]["equipment"].as_array_mut() {
        // Office 101's south wall is 0..6" (outside face y 0); the symbol 42" x 12", 8" in front of it.
        eq.push(
            json!({"tag": "U-1", "kind": "ptac", "level": "L1", "min": [60.0, 14.0], "max": [102.0, 26.0], "bottom_ft": 0.3333, "height_ft": 1.3333}),
        );
        // In the middle of office 102: no wall within reach.
        eq.push(json!({"tag": "U-2", "kind": "ptac", "level": "L1", "min": [460.0, 50.0], "max": [502.0, 62.0], "bottom_ft": 0.3333, "height_ft": 1.3333}));
    }
    s.execute("bspec.set", &json!({"replace": true, "spec": spec})).expect("spec");
    s.execute("model.build", &json!({})).expect("build");
    let d = s.doc().expect("doc");
    let unit = tagged(d, "U-1");
    let b = cadaddy_engine::doc::entity_bounds(d, unit, 0);
    // From the outside face (y 0) 21" into the room (the rule's depth, no product data): through
    // the 6" wall, its cabinet 15" past the room face; along the wall as drawn.
    assert!(b.min.y.abs() < 0.01 && (b.max.y - 21.0).abs() < 0.01, "through the wall: {b:?}");
    assert!((b.min.x - 60.0).abs() < 0.01 && (b.max.x - 102.0).abs() < 0.01, "along the wall as drawn: {b:?}");
    let host = prop_of(unit, "HOST_WALL").expect("hosted");
    let wall = d.model.iter().find(|e| e.handle.hex() == host).expect("its host wall is built");
    assert_eq!(made_as(wall).as_deref(), Some("wall"));
    assert!(prop_of(unit, "PLACEMENT").is_some_and(|p| p.starts_with("derived:") && p.contains("M-THROUGH-WALL")), "{:?}", unit.common.xdata);
    assert!(unit.common.xdata.iter().any(|t| t.value == "status:derived"), "its place is derived: {:?}", unit.common.xdata);
    // Its drawn place beside the seated one.
    let ps = cadaddy_engine::doc::props::read(&unit.common.xdata);
    assert_eq!(ps.get("design:location").map(|p| p.text()), Some("6.75, 1.67".to_string()), "{ps:?}");
    assert_eq!(ps.get("location").map(|p| p.text()), Some("6.75, 0.88".to_string()), "{ps:?}");
    // Its sleeve and grille: their own elements, in the wall.
    for part in ["U-1 SLEEVE", "U-1 GRILLE"] {
        let p = tagged(d, part);
        let pb = cadaddy_engine::doc::entity_bounds(d, p, 0);
        assert!(pb.min.y.abs() < 0.01 && pb.max.y <= 13.76, "{part} in the wall: {pb:?}");
        assert_eq!(prop_of(p, "HOST_WALL").as_deref(), Some(host.as_str()), "{part}");
        assert!(p.common.xdata.iter().any(|t| t.value == "status:derived"), "{part}: {:?}", p.common.xdata);
    }
    // The wall cut for the sleeve: no wall at the sleeve's middle at the unit's height, wall beside it.
    let EntityKind::Mesh(m) = &wall.kind else { panic!("a wall mesh") };
    // Inside the wall's solid: an odd number of its flat faces over the point (a vertical ray).
    let solid_at = |x: f64, y: f64, z: f64| {
        let over = m
            .faces
            .iter()
            .filter(|f| {
                let vs: Vec<Vec3> = f.v.iter().filter_map(|i| m.vertices.get(*i as usize).copied()).collect();
                let flat = vs.len() >= 3 && vs.iter().all(|v| (v.z - vs[0].z).abs() < 1e-9);
                let (x0, x1) = vs.iter().fold((f64::INFINITY, f64::NEG_INFINITY), |(a, b), v| (a.min(v.x), b.max(v.x)));
                let (y0, y1) = vs.iter().fold((f64::INFINITY, f64::NEG_INFINITY), |(a, b), v| (a.min(v.y), b.max(v.y)));
                flat && vs[0].z > z && x > x0 && x < x1 && y > y0 && y < y1
            })
            .count();
        over % 2 == 1
    };
    assert!(!solid_at(81.0, 3.0, 12.0), "the sleeve's opening is cut");
    assert!(solid_at(30.0, 3.0, 12.0) && solid_at(81.0, 3.0, 2.0) && solid_at(81.0, 3.0, 60.0), "the wall beside, under and over it stays");
    // No outside wall near: built as drawn, a review item.
    let u2 = tagged(d, "U-2");
    assert!(prop_of(u2, "PLACEMENT").is_some_and(|p| p.starts_with("review:") && p.contains("no outside wall")), "{:?}", u2.common.xdata);
    assert!(prop_of(u2, "HOST_WALL").is_none());
    // G-HOST: the unit through its wall passes.
    let r = s.execute("validate.prescreen", &json!({"rules": ["G-HOST"], "all": true, "limit": 50})).expect("prescreen");
    let host_hex = unit_handle(&mut s, "U-1");
    let f = r["findings"].as_array().into_iter().flatten().find(|f| f["handle"].as_str().is_some_and(|h| h.eq_ignore_ascii_case(&host_hex)));
    let f = f.unwrap_or_else(|| panic!("a G-HOST finding for the unit: {r:#}"));
    assert_eq!(f["verdict"], "pass", "{f:#}");
    assert!(f["issue"].as_str().is_some_and(|i| i.contains("goes through")), "{f:#}");
}

fn unit_handle(s: &mut Session, tag: &str) -> String {
    tagged(s.doc().expect("doc"), tag).handle.hex()
}

#[test]
fn a_product_no_section_speaks_for_cites_the_submittal_its_maker_comes_from() {
    use cadaddy_engine::doc::props::{self, Prop, Source, SourceKind};
    let mut s = plan();
    fill(&mut s);
    // A made-up submittal's maker on receptacle A; the section read speaks for switches only.
    let a = find(&mut s, "device", json!({"type_mark": "A"}));
    let h = cadaddy_engine::doc::Handle::parse_hex(&a[0]).unwrap();
    assert!(s.doc_mut().unwrap().model.modify(h, |e| {
        let mut store = props::read(&e.common.xdata);
        store.set("manufacturer", Prop::new("MAKER ONE", Source::from_doc(SourceKind::Submittal, "Submittal 26 27 26-1 p.2")));
        e.common.xdata = props::write(&e.common.xdata, &store).into();
    }));
    let dir = std::env::temp_dir().join(format!("cadaddy-products-sub-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("26 27 26 - Wiring Devices.txt"),
        "SECTION 262726 - WIRING DEVICES\nPART 2 - PRODUCTS\n2.4 TOGGLE SWITCHES\nB. Switches, 120/277 V, 20 A:\n1. Single Pole:\n\
         a. Manufacturers: Subject to compliance with requirements, provide products by one of the following:\n1) Maker Two.\n\
         PART 3 - EXECUTION\n",
    )
    .unwrap();
    s.execute("bim.products", &json!({"specs": [dir.to_string_lossy()]})).unwrap();
    let a = device_of(&mut s, "A");
    assert_eq!(a["product"]["source"]["kind"], "submittal", "{a:#}");
    assert_eq!(a["product"]["source"]["doc"], "Submittal 26 27 26-1 p.2", "{a:#}");
    assert!(a["approved_makers"].is_null(), "no section's makers: {a:#}");
    let _ = std::fs::remove_dir_all(&dir);
}

/// A device a typical plan stamped into a unit, drawn on the line of the next room's wall (an
/// entry's receptacle on the corridor's wall line), is fed from its unit's own panel.
#[test]
fn a_stamped_device_on_a_neighbours_wall_line_is_fed_from_its_units_panel() {
    let mut s = plan();
    let mut sp = s.execute("bspec.get", &json!({})).expect("spec")["spec"].clone();
    sp["tags"][0]["fields"]["room"] = json!({"value": "102", "source": "rule"});
    sp["mep"]["equipment"][1]["tag"] = json!("LP1-102");
    sp["mep"]["equipment"][1]["circuits"] = json!([]);
    s.execute("bspec.set", &json!({"replace": true, "spec": sp})).expect("spec");
    s.execute("model.build", &json!({})).expect("build");
    fill(&mut s);
    let a = device_of(&mut s, "A");
    assert_eq!(a["room"]["value"], "101", "it stands in 101: {a:#}");
    let panel = find(&mut s, "equipment", json!({"type_mark": "LP1-102"}));
    assert_eq!(a["fed_from_handle"]["value"], json!(panel[0]), "its unit's panel: {a:#}");
}

/// Q6354: a luminaire built into casework (its plan draws only its junction box) stands in the
/// casework at the height its casework detail gives, else at the rules' casework height: never
/// hung from the ceiling, even drawn clear of every wall (a bar's or an island's casework).
#[test]
fn a_casework_luminaire_takes_its_details_height_and_is_never_hung_from_the_ceiling() {
    let mut s = plan();
    let mut spec = s.execute("bspec.get", &json!({})).expect("spec")["spec"].clone();
    if let Some(t) = spec["tags"].as_array_mut() {
        for (x, mark, symbol, height) in [(8.0, "X3", "1FA", Some("42")), (12.0, "X4", "1FB", None)] {
            let mut l = device(x, 5.0, mark, "K-4,b", symbol);
            l["fields"]["device_type"] = json!({"value": format!("luminaire {mark}"), "source": "plan"});
            l["fields"]["device_kind"] = json!({"value": "luminaire", "source": "submittal"});
            l["fields"]["mount"] = json!({"value": "casework", "source": "plan"});
            l["fields"]["connection"] = json!({"value": "E-1 draws a junction box (J) tagged with luminaire type", "source": "plan"});
            if let Some(h) = height {
                l["fields"]["mount_height_in"] = json!({"value": h, "source": "note"});
                l["fields"]["mount_height_note"] = json!({"value": "A-50 casework detail, luminaire X3: 3'-6\" AFF", "source": "note"});
            }
            t.push(l);
        }
    }
    s.execute("bspec.set", &json!({"replace": true, "spec": spec})).expect("spec");
    s.execute("model.build", &json!({})).expect("build");
    fill(&mut s);
    let luminaire = |s: &mut Session, mark: &str| {
        let hs = find(s, "light_fixture", json!({"type_mark": mark}));
        assert_eq!(hs.len(), 1, "one luminaire {mark}: {hs:?}");
        get(s, &hs[0])
    };
    let x3 = luminaire(&mut s, "X3");
    assert_eq!(x3["mount_height"]["value"], json!(42.0), "its casework detail's height: {x3:#}");
    let x4 = luminaire(&mut s, "X4");
    assert_eq!(x4["mount_height"]["value"], json!(54.0), "the rules' casework height, not the ceiling: {x4:#}");
    assert_eq!(x4["mount_height"]["source"]["kind"], "default", "never presented as read: {x4:#}");
}

/// Office 101 under a 7'-11" ceiling and storage 102 under a 9'-0" one, a ceiling exhaust fan in
/// each at its ceiling, and each fan's connection as the job leaves it: at the fan in plan, at
/// the one height (`96`) the job fixed when it read it.
fn fans_and_connections(s: &mut Session, ceilings: [f64; 2], fan_x: [f64; 2]) {
    let mut spec = s.execute("bspec.get", &json!({})).expect("spec")["spec"].clone();
    spec["rooms"] = json!([
        {"name": "OFFICE", "number": "101", "level": "L1", "ceiling_ft": ceilings[0] / 12.0, "ceiling_source": "room RCP: test"},
        {"name": "STORAGE", "number": "102", "level": "L1", "ceiling_ft": ceilings[1] / 12.0, "ceiling_source": "room RCP: test"}
    ]);
    let fan = |tag: &str, x: f64, bottom_in: f64| {
        json!({"tag": tag, "kind": "exhaust_fan", "level": "L1", "min": [x, 50.0], "max": [x + 12.0, 62.0], "bottom_ft": bottom_in / 12.0,
               "height_ft": 10.0 / 12.0, "title": "EXHAUST FAN", "systems": ["EA"], "sources": {"sheet": "M-101"}})
    };
    let eq = spec["mep"]["equipment"].as_array_mut().expect("equipment");
    eq.retain(|e| e["kind"] != "exhaust_fan");
    eq.push(fan("EF", fan_x[0], ceilings[0]));
    eq.push(fan("EF", fan_x[1], ceilings[1]));
    let tags = spec["tags"].as_array_mut().expect("tags");
    tags.retain(|t| t["kind"] != "equipment_connection");
    for (x, sym) in [(fan_x[0] + 6.0, "1E0"), (fan_x[1] + 6.0, "1E1")] {
        let mut c = device(x / 12.0, 56.0 / 12.0, "EF", "LP1-7", sym);
        c["kind"] = json!("equipment_connection");
        c["fields"]["mount_height_in"] = json!({"value": "96", "source": "note"});
        c["fields"]["mount_height_note"] = json!({"value": "connection to EF in the room's ceiling (M-101)", "source": "note"});
        tags.push(c);
    }
    s.execute("bspec.set", &json!({"replace": true, "spec": spec})).expect("spec");
    s.execute("model.build", &json!({})).expect("build");
}

/// Each fan connection's box (bottom and top inches over the floor) and plan middle (inches), by
/// its plan x.
fn connections(s: &mut Session) -> Vec<(f64, f64, [f64; 2], Value)> {
    let mut out: Vec<(f64, f64, [f64; 2], Value)> = find(s, "device", json!({"type_mark": "EF"}))
        .iter()
        .map(|h| {
            let p = get(s, h);
            let mid = p["mount_height"]["value"].as_f64().unwrap_or_else(|| panic!("height: {p:#}"));
            let loc: Vec<f64> = p["location"]["value"].as_str().unwrap_or_default().split(',').filter_map(|v| v.trim().parse().ok()).collect();
            let (x, y) = (loc.first().copied().unwrap_or(f64::NAN), loc.get(1).copied().unwrap_or(f64::NAN));
            (mid - 2.0, mid + 2.0, [x * 12.0, y * 12.0], p)
        })
        .collect();
    out.sort_by(|a, b| a.2[0].total_cmp(&b.2[0]));
    out
}

/// The ceiling built over a plan point (inches over the floor): a luminaire hung there.
fn ceiling_at(s: &mut Session, x_ft: f64, sym: &str) -> f64 {
    let mut spec = s.execute("bspec.get", &json!({})).expect("spec")["spec"].clone();
    let mut lum = device(x_ft, 3.0, "QX", "", sym);
    lum["fields"]["mount"] = json!({"value": "ceiling", "source": "submittal"});
    spec["tags"].as_array_mut().expect("tags").push(lum);
    s.execute("bspec.set", &json!({"replace": true, "spec": spec})).expect("spec");
    s.execute("model.build", &json!({})).expect("build");
    let q = device_of(s, "QX");
    let mut spec = s.execute("bspec.get", &json!({})).expect("spec")["spec"].clone();
    spec["tags"].as_array_mut().expect("tags").retain(|t| t["fields"]["type"]["value"] != "QX");
    s.execute("bspec.set", &json!({"replace": true, "spec": spec})).expect("spec");
    q["mount_height"]["value"].as_f64().unwrap_or_else(|| panic!("{q:#}"))
}

#[test]
fn a_ceiling_fans_connection_is_above_the_ceiling_at_its_fan_and_follows_it() {
    let _g = serial();
    let mut s = plan();
    fans_and_connections(&mut s, [95.0, 108.0], [100.0, 460.0]);
    // The rooms' ceilings as built (what a connection must be above).
    let built = [ceiling_at(&mut s, 106.0 / 12.0, "1E8"), ceiling_at(&mut s, 466.0 / 12.0, "1E9")];
    assert_eq!(built, [95.0, 108.0], "the rooms' own ceilings are built");
    s.execute("model.build", &json!({})).expect("build");
    let check = |got: &[(f64, f64, [f64; 2], Value)], ceilings: [f64; 2], fan_x: [f64; 2]| {
        assert_eq!(got.len(), 2, "{got:?}");
        for ((bottom, top, at, p), (ceiling, x)) in got.iter().zip(ceilings.iter().zip(fan_x)) {
            // On the hidden side of the ceiling, at its fan's housing (10" tall from the ceiling).
            assert!(*bottom >= *ceiling, "above the {ceiling} in ceiling, not exposed under it: {bottom} to {top}: {p:#}");
            assert!(*bottom <= ceiling + 10.0 && *top >= *ceiling, "beside the housing's height: {bottom} to {top}: {p:#}");
            let off = Vec2::new(at[0].clamp(x, x + 12.0), at[1].clamp(50.0, 62.0)).dist(Vec2::new(at[0], at[1]));
            assert!(off <= 12.0, "within 12 in of its fan in plan: {off} in at {at:?}: {p:#}");
            assert_eq!(p["mount_height_from"]["value"], "equipment", "{p:#}");
            assert!(
                p["mount_height"]["source"]["doc"].as_str().is_some_and(|d| d.contains("E-CONN-CEILING-EQUIP"))
                    || p["mount_height"]["note"].as_str().is_some_and(|n| n.contains("E-CONN-CEILING-EQUIP")),
                "the rule its basis: {p:#}"
            );
            // The job's fixed height kept as the design value beside the built one.
            assert_eq!(p["design:mount_height"]["value"], json!("96"), "{p:#}");
            assert!(p["correction:mount_height"]["value"].as_str().is_some_and(|w| w.contains("E-CONN-CEILING-EQUIP")), "{p:#}");
        }
    };
    let got = connections(&mut s);
    check(&got, [95.0, 108.0], [100.0, 460.0]);
    // The ceiling of 102 raised and the office's fan moved: each connection follows on the next build.
    fans_and_connections(&mut s, [95.0, 117.0], [130.0, 460.0]);
    let moved = connections(&mut s);
    check(&moved, [95.0, 117.0], [130.0, 460.0]);
    assert!(moved[1].0 > got[1].0 + 8.0 && moved[0].2[0] > got[0].2[0] + 20.0, "{got:?} {moved:?}");
}

// ─── Air terminals (Q6318) ──────────────────────────────────────────────────

/// [`plan`] with its spec changed by `edit`, built again and filled.
fn plan_edited(edit: impl FnOnce(&mut Value)) -> Session {
    let mut s = plan();
    let mut sp = s.execute("bspec.get", &json!({})).expect("spec")["spec"].clone();
    edit(&mut sp);
    s.execute("bspec.set", &json!({"replace": true, "spec": sp})).expect("spec");
    s.execute("model.build", &json!({})).expect("build");
    fill(&mut s);
    s
}

/// The one air terminal whose type mark is `mark`.
fn terminal_of(s: &mut Session, mark: &str) -> Value {
    let hs = find(s, "fixture", json!({"type_mark": mark}));
    assert_eq!(hs.len(), 1, "one terminal {mark}: {hs:?}");
    get(s, &hs[0])
}

/// A diffuser takes what its plan tag says (its type mark and airflow, over an airflow printed
/// nearby), its type's row of the air device schedule, and the unit its duct leads to; the air
/// schedule lists it under that unit, not as served by none.
#[test]
fn an_air_terminal_takes_its_tags_type_and_airflow_its_schedule_row_and_its_unit() {
    let _g = serial();
    let mut s = plan_edited(|sp| {
        sp["mep"]["terminals"][0]["tag"] = json!("SD-4");
        // A mark the schedule doesn't list (another tag read beside a symbol).
        sp["mep"]["terminals"].as_array_mut().expect("terminals").push(
            json!({"system": "SA", "level": "L1", "at": [60, 60], "size_in": [12, 12], "bottom_ft": 8, "terminal_kind": "supply_diffuser", "tag": "X-01"}),
        );
        sp["mep"]["terminals"][0]["cfm"] = json!(260.0);
        // The air handler at the run's end, on its system.
        sp["mep"]["equipment"][0]["min"] = json!([1800, 520]);
        sp["mep"]["equipment"][0]["max"] = json!([1860, 560]);
        sp["mep"]["equipment"][0]["systems"] = json!(["SA"]);
        sp["mep"]["air_device_schedule"] = json!([
            {"tag": "SD-1", "properties": {"MODEL": "OTHER"}, "source": "M-601"},
            {"tag": "SD-4", "properties": {"MANUFACTURER": "MAKER A", "MODEL": "CONE-9", "FACE SIZE (IN)": "24\"x24\"", "MOUNTING LOCATION": "CEILING"}, "source": "M-601"}
        ]);
    });
    let t = terminal_of(&mut s, "SD-4");
    assert_eq!(t["type_mark"]["source"]["kind"], "print", "{t:#}");
    assert!(t["type_mark"].get("note").is_none(), "the schedule lists SD-4: {t:#}");
    assert_eq!(t["cfm"]["value"], json!(260.0), "its tag's airflow over the 150 CFM printed by it: {t:#}");
    assert_eq!(t["model"]["value"], "CONE-9", "{t:#}");
    assert_eq!(t["model"]["source"]["kind"], "schedule");
    assert_eq!(t["manufacturer"]["value"], "MAKER A");
    assert_eq!(t["mounting"]["value"], "CEILING");
    assert_eq!(t["face_size"]["value"], "24\"x24\"");
    assert_eq!(t["served_by"]["value"], "AHU-1", "{t:#}");
    assert_eq!(t["served_by"]["source"]["kind"], "computed");
    assert!(t["served_by"]["note"].as_str().is_some_and(|n| n.contains("duct network")), "{t:#}");
    let x = terminal_of(&mut s, "X-01");
    assert!(x["type_mark"]["note"].as_str().is_some_and(|n| n.contains("not a mark of the air device schedule")), "{x:#}");
    let air = s.execute("bim.schedule", &json!({"kind": "air"})).expect("air schedule");
    let rows: Vec<Value> = air["rows"].as_array().cloned().unwrap_or_default().into_iter().filter(|r| r["type_mark"] == "SD-4").collect();
    assert_eq!(rows.len(), 1, "{air:#}");
    assert_eq!(rows[0]["unit"], "AHU-1", "{air:#}");
    assert_eq!(rows[0]["type_mark"], "SD-4");
    assert_eq!(rows[0]["cfm"], "260");
}

/// With no duct to a unit: an outdoor air unit whose schedule row says it serves the room's
/// kind of room serves its supply diffuser (derived), a room's own exhaust fan its exhaust
/// grille (inferred, by the trade practice rule); a supply diffuser no unit can be said to
/// serve stays unserved, and an exhaust fan never serves a supply diffuser.
#[test]
fn a_terminal_no_duct_reaches_is_served_by_its_schedules_unit_or_its_rooms_own() {
    let _g = serial();
    let mut s = plan_edited(|sp| {
        let systems = sp["mep"]["systems"].as_array_mut().expect("systems");
        systems.push(json!({"name": "EA", "layer": "M-EXH", "kind": "duct", "size_in": [6, 6], "bottom_ft": 9}));
        let terms = sp["mep"]["terminals"].as_array_mut().expect("terminals");
        // Office 101 (x 0–20'): an exhaust grille and a supply diffuser; storage 102 (x 30–50'):
        // a supply diffuser.
        terms.push(json!({"system": "EA", "level": "L1", "at": [60, 60], "size_in": [8, 8], "bottom_ft": 8, "terminal_kind": "exhaust_grille", "tag": "EG-1"}));
        terms.push(json!({"system": "SA", "level": "L1", "at": [180, 60], "size_in": [8, 8], "bottom_ft": 8, "terminal_kind": "supply_diffuser", "tag": "SD-7"}));
        terms.push(json!({"system": "SA", "level": "L1", "at": [480, 60], "size_in": [8, 8], "bottom_ft": 8, "terminal_kind": "supply_diffuser", "tag": "SD-1"}));
        let eq = sp["mep"]["equipment"].as_array_mut().expect("equipment");
        eq.push(json!({"tag": "EF-1", "kind": "exhaust_fan", "level": "L1", "min": [120, 80], "max": [140, 100], "height_ft": 1, "systems": ["EA"]}));
        eq.push(json!({"tag": "OAU-3", "kind": "outdoor_air_unit", "level": "L1", "min": [2400, 400], "max": [2460, 440], "height_ft": 4,
                       "properties": {"CFM": "900", "AREA SERVED": "STORAGE ROOMS"}, "sources": {"schedule": "M-601"}}));
    });
    let sd1 = terminal_of(&mut s, "SD-1");
    assert_eq!(sd1["served_by"]["value"], "OAU-3", "{sd1:#}");
    assert_eq!(sd1["served_by"]["source"]["kind"], "computed");
    assert!(sd1["served_by"]["note"].as_str().is_some_and(|n| n.contains("schedule row") && n.contains("STORAGE")), "{sd1:#}");
    let eg = terminal_of(&mut s, "EG-1");
    assert_eq!(eg["served_by"]["value"], "EF-1", "{eg:#}");
    assert_eq!(eg["served_by"]["source"]["kind"], "rule");
    assert!(eg["served_by"]["note"].as_str().is_some_and(|n| n.starts_with("M-TERMINAL-ROOM-UNIT")), "{eg:#}");
    let sd7 = terminal_of(&mut s, "SD-7");
    assert!(sd7.get("served_by").is_none(), "no supply unit for office 101: {sd7:#}");
    // With no air device schedule in the set, a mark is taken as printed.
    assert!(sd7["type_mark"].get("note").is_none(), "{sd7:#}");
    let r = s.execute("bim.props.check", &json!({"checks": ["terminal_unserved"]})).expect("check");
    let unserved: Vec<String> = r["findings"].as_array().into_iter().flatten().filter_map(|f| f["message"].as_str().map(str::to_string)).collect();
    assert!(unserved.iter().any(|m| m.contains("SD-7")) && !unserved.iter().any(|m| m.contains("SD-1") || m.contains("EG-1")), "{unserved:?}");
}

// ─── Through-wall units (Q6586) ─────────────────────────────────────────────

/// Two 12' × 20' guest rooms side by side (101 x 0–12', 102 x 12.5–24.5'), a 7 7/8" exterior
/// wall on the south and 6" walls elsewhere; in each room a through-wall unit (42" × 21.6",
/// centred in the exterior wall's depth, 4" off the floor), a room panel of the typical panel U
/// and a receptacle on it. Room 102 also has the unit's receptacle drawn (`unit_receptacle`):
/// a device tagged with the unit's tag on the unit's 2-pole circuit. A condensing unit stands
/// out on the site, in no room. Built, rooms traced, filled.
fn unit_rooms(unit_receptacle: bool) -> (Session, Value) {
    let mut d = Drawing::new_imperial();
    let ext = 7.875 / 12.0;
    // Outside faces.
    for (a, b) in [((-0.5, -ext), (25.0, -ext)), ((25.0, -ext), (25.0, 20.5)), ((25.0, 20.5), (-0.5, 20.5)), ((-0.5, 20.5), (-0.5, -ext))] {
        line(&mut d, "A-WALL", a, b);
    }
    // Inside faces of each room.
    for x0 in [0.0, 12.5] {
        let x1 = x0 + 12.0;
        for (a, b) in [((x0, 0.0), (x1, 0.0)), ((x1, 0.0), (x1, 20.0)), ((x1, 20.0), (x0, 20.0)), ((x0, 20.0), (x0, 0.0))] {
            line(&mut d, "A-WALL", a, b);
        }
    }
    let mut s = Session::empty();
    s.open_drawing(d, "Sample Floor Plan", None);
    text(&mut s, "A-ROOM-TAG", [60.0, 130.0], "GUEST");
    text(&mut s, "A-ROOM-TAG", [66.0, 118.0], "101");
    text(&mut s, "A-ROOM-TAG", [210.0, 130.0], "GUEST");
    text(&mut s, "A-ROOM-TAG", [216.0, 118.0], "102");
    // The unit: 42" wide along the wall, 21.6" deep across it, centred in the wall (y −7 7/8"–0).
    let unit = |x_mid: f64| {
        let yc = -7.875 / 2.0;
        json!({"tag": "PTAC-1", "kind": "ptac", "level": "L1", "min": [x_mid - 21.0, yc - 10.8], "max": [x_mid + 21.0, yc + 10.8],
               "bottom_ft": 4.0 / 12.0, "height_ft": 16.0 / 12.0, "systems": ["M-DUCT"], "sources": {"schedule": "M-601"}})
    };
    let panel = |tag: &str, room: &str, x: f64| {
        json!({"tag": tag, "kind": "panel", "level": "L1", "min": [x, 230.0], "max": [x + 14.0, 234.0], "height_ft": 2, "bottom_ft": 4,
               "properties": {"PANEL": "U", "ROOM": room}})
    };
    let mut tags = vec![device(3.0, 19.5, "A", "U-1", "1F0"), device(15.5, 19.5, "A", "U-1", "1F1")];
    if unit_receptacle {
        // Under the unit, in its sub-base: on the 2-pole circuit the schedule gives the unit.
        let mut r = device(20.5, 0.4, "PTAC", "U-13,15", "1F2");
        r["fields"]["device_type"] = json!({"value": "receptacle", "source": "plan"});
        tags.push(r);
    }
    let spec = json!({
        "version": 1, "units_per_foot": 12,
        "levels": [{"name": "L1", "elevation_ft": 0, "height_ft": 10, "ceiling_ft": 9, "base": [0, 0]}],
        "layers": [{"layer": "A-WALL", "role": "wall", "wall_type": "Exterior"}, {"layer": "A-ROOM-TAG", "role": "room_tag"}],
        "wall_types": [{"name": "Exterior", "thickness_in": 7.875}],
        "tags": tags,
        "mep": {
            "systems": [{"name": "M-DUCT", "layer": "M-DUCT", "kind": "duct", "size_in": [12, 8], "bottom_ft": 9}],
            "equipment": [
                unit(72.0), unit(222.0),
                panel("U-101", "101", 20.0), panel("U-102", "102", 170.0),
                {"tag": "CU-1", "kind": "condensing_unit", "level": "L1", "min": [480.0, 60.0], "max": [516.0, 96.0], "height_ft": 3}
            ],
            "panel_schedules": [{"panel": "U", "typical_of": 2, "source": "E-601", "circuits": [
                {"number": "1", "description": "ROOM RCPT", "breaker": "20", "poles": 1, "source": "E-601"},
                {"number": "13", "description": "PTAC", "breaker": "20", "poles": 2, "source": "E-601"}
            ]}]
        }
    });
    s.execute("bspec.set", &json!({"replace": true, "spec": spec})).expect("spec");
    s.execute("model.build", &json!({"route": false})).expect("build");
    s.execute("rooms.build", &json!({})).expect("rooms");
    let r = fill(&mut s);
    (s, r)
}

/// The built equipment tagged `tag`, by its west edge.
fn equipment_by_x(s: &mut Session, tag: &str) -> Vec<String> {
    let d = s.doc().expect("doc");
    let mut v: Vec<(f64, String)> = d
        .model
        .iter()
        .filter(|e| cadaddy_engine::cmd::bimlook::made_tag(e, "tag") == Some(tag))
        .map(|e| (cadaddy_engine::doc::entity_bounds(d, e, 0).min.x, e.handle.hex()))
        .collect();
    v.sort_by(|a, b| a.0.total_cmp(&b.0));
    v.into_iter().map(|(_, h)| h).collect()
}

/// A unit set through an exterior wall (its middle in the wall, outside every room) takes the
/// room on its service face, the room's own panel's circuit for it and that panel as its feed:
/// from its receptacle in the room when the plans draw one, else from the room panel's schedule
/// row for its kind. Equipment left in no room is a warning with its reason, never silent.
#[test]
fn a_unit_through_an_exterior_wall_takes_the_room_on_its_service_face_and_its_circuit() {
    let _g = serial();
    for drawn in [false, true] {
        let (mut s, r) = unit_rooms(drawn);
        let units = equipment_by_x(&mut s, "PTAC-1");
        assert_eq!(units.len(), 2, "{r:#}");
        let missing = find(&mut s, "equipment", json!({"room": "missing"}));
        assert!(units.iter().all(|u| !missing.contains(u)), "drawn {drawn}: units in no room {missing:?}: {r:#}");
        for (h, room) in units.iter().zip(["101", "102"]) {
            let p = get(&mut s, h);
            assert_eq!(p["room"]["value"], room, "{p:#}");
            // Its room on its service face: the note says so while its middle is in the wall; a
            // unit seated through the wall (Q6546) has its cabinet, and its middle, in that room.
            let seated = p["location"]["note"].as_str().is_some_and(|v| v.starts_with("M-THROUGH-WALL"));
            assert!(seated || p["room"]["note"].as_str().is_some_and(|n| n.contains("service face")), "{p:#}");
            assert_eq!(p["circuit"]["value"], "U-13,15", "drawn {drawn}: {p:#}");
            assert_eq!(p["panel"]["value"], "U", "{p:#}");
            let feed = p["fed_from_handle"]["value"].as_str().unwrap_or_default().to_string();
            let fp = get(&mut s, &feed);
            assert!(
                !feed.is_empty() && equipment_by_x(&mut s, &format!("U-{room}")) == vec![feed.clone()],
                "fed from its room's panel: {p:#} {fp:#}"
            );
        }
        // Room 102's unit: its receptacle's circuit when drawn, else the schedule row's.
        let p = get(&mut s, &units[1]);
        let how = p["circuit"]["note"].as_str().unwrap_or_default().to_string();
        assert_eq!(how.contains("receptacle") || how.contains("connection"), drawn, "{p:#}");
        if !drawn {
            assert_eq!(p["circuit"]["source"]["kind"], "schedule", "{p:#}");
        }
        // The condensing unit outside is in no room: a warning with its reason.
        let cu = equipment_by_x(&mut s, "CU-1");
        let listed = r["equipment_in_no_room_listed"].as_array().cloned().unwrap_or_default();
        assert!(
            listed.iter().any(|x| cu.first().is_some_and(|h| x["handle"] == h.as_str()) && x["why"].as_str().is_some_and(|w| !w.is_empty())),
            "{r:#}"
        );
        assert!(!listed.iter().any(|x| units.iter().any(|u| x["handle"] == u.as_str())), "{r:#}");
        assert_eq!(r["equipment_in_no_room"]["L1"], 1, "{r:#}");
    }
}
