//! Q6158 devicefit: every device in its wall at the documents' height; nothing outside the
//! building unsaid. Made-up plans only.

use std::sync::{Mutex, MutexGuard};

use cadaddy_engine::Session;
use cadaddy_engine::doc::{Drawing, Entity, EntityKind, Handle, Line};
use cadaddy_engine::geom::Vec3;
use cadaddy_mepfix::generate_hotel;
use cadaddy_mepfix::hotel::HotelSize;
use serde_json::{Value, json};

static SERIAL: Mutex<()> = Mutex::new(());

fn serial() -> MutexGuard<'static, ()> {
    SERIAL.lock().unwrap_or_else(|e| e.into_inner())
}

fn line(d: &mut Drawing, layer: &str, a: (f64, f64), b: (f64, f64)) {
    let p = |(x, y): (f64, f64)| Vec3 { x: x * 12.0, y: y * 12.0, z: 0.0 };
    let h = Handle(d.handseed.max(0x100));
    d.handseed = h.0 + 1;
    let mut e = Entity::new(h, EntityKind::Line(Line { a: p(a), b: p(b) }));
    e.common.layer = layer.into();
    d.model.push(e);
}

fn text_on(s: &mut Session, layer: &str, at: [f64; 2], t: &str) {
    if s.execute("layer.new", &json!({"name": layer, "current": true})).is_err() {
        s.execute("layer.current", &json!({"name": layer})).expect("layer");
    }
    s.execute("text", &json!({"at": at, "text": t, "height": 6})).expect("text");
}

fn text(s: &mut Session, at: [f64; 2], t: &str) {
    text_on(s, "E-ANNO", at, t);
}

/// The two rooms' tags: OFFICE 101 (west), STORAGE 102 (east).
fn room_tags(s: &mut Session) {
    text_on(s, "A-ROOM-TAG", [110.0, 64.0], "OFFICE");
    text_on(s, "A-ROOM-TAG", [116.0, 52.0], "101");
    text_on(s, "A-ROOM-TAG", [350.0, 64.0], "STORAGE");
    text_on(s, "A-ROOM-TAG", [356.0, 52.0], "102");
}

/// A device the readers put in the spec at `(x, y)` feet: its type mark, kind, symbol and extra
/// fields.
fn device(x: f64, y: f64, mark: &str, kind: &str, symbol: &str, extra: Value) -> Value {
    let mut fields = json!({
        "type": {"value": mark, "source": "plan"},
        "device_type": {"value": format!("{kind} {mark}"), "source": "plan"},
        "sheet": {"value": "E-101", "source": "plan"},
        "symbol": {"value": symbol, "source": "plan"},
    });
    if let (Some(f), Some(x)) = (fields.as_object_mut(), extra.as_object()) {
        for (k, v) in x {
            f.insert(k.clone(), json!({"value": v, "source": "plan"}));
        }
    }
    json!({"id": format!("device:L1:{x:.1},{y:.1}:{mark}"), "kind": kind, "level": "L1", "at_ft": [x, y], "fields": fields})
}

/// Two rooms side by side, 6" walls: west 0.5–19.75', east 20.25–40', 0.5–9.5' deep; the
/// partition between them from 19.75' to 20.25'.
fn two_rooms() -> Drawing {
    let mut d = Drawing::new_imperial();
    let outer = [((0., 0.), (40.5, 0.)), ((40.5, 0.), (40.5, 10.)), ((40.5, 10.), (0., 10.)), ((0., 10.), (0., 0.))];
    let west = [((0.5, 0.5), (19.75, 0.5)), ((19.75, 0.5), (19.75, 9.5)), ((19.75, 9.5), (0.5, 9.5)), ((0.5, 9.5), (0.5, 0.5))];
    let east = [((20.25, 0.5), (40.0, 0.5)), ((40.0, 0.5), (40.0, 9.5)), ((40.0, 9.5), (20.25, 9.5)), ((20.25, 9.5), (20.25, 0.5))];
    for (a, b) in outer.into_iter().chain(west).chain(east) {
        line(&mut d, "A-WALL", a, b);
    }
    d
}

fn spec(tags: Vec<Value>, extra: Value) -> Value {
    let mut sp = json!({
        "version": 1, "units_per_foot": 12,
        "levels": [{"name": "L1", "elevation_ft": 0, "height_ft": 10, "ceiling_ft": 9, "base": [0, 0]}],
        "layers": [{"layer": "A-WALL", "role": "wall", "wall_type": "Partition"}, {"layer": "A-ROOM-TAG", "role": "room_tag"}],
        "wall_types": [{"name": "Partition", "thickness_in": 6}],
        "tags": tags,
    });
    if let (Some(o), Some(x)) = (sp.as_object_mut(), extra.as_object()) {
        o.extend(x.clone());
    }
    sp
}

fn build(s: &mut Session, sp: Value) {
    s.execute("bspec.set", &json!({"replace": true, "spec": sp})).expect("spec");
    s.execute("model.build", &json!({})).expect("build");
}

fn handle_of(s: &mut Session, mark: &str) -> String {
    let r = s.execute("bim.props.find", &json!({"kind": "device", "where": {"type_mark": mark}, "limit": 10})).expect("find");
    let hs: Vec<String> = r["handles"].as_array().expect("handles").iter().filter_map(|h| h.as_str().map(str::to_string)).collect();
    assert_eq!(hs.len(), 1, "one device {mark}: {hs:?}");
    hs[0].clone()
}

fn props(s: &mut Session, mark: &str) -> Value {
    let h = handle_of(s, mark);
    s.execute("bim.props.get", &json!({"handle": h})).expect("get")["elements"][0]["props"].clone()
}

/// A device box's plan extent and its bottom and top (inches).
fn extent(s: &mut Session, mark: &str) -> ([f64; 4], f64, f64) {
    let h = Handle::parse_hex(&handle_of(s, mark)).expect("hex");
    let d = s.doc().expect("doc");
    let e = d.model.get(h).expect("device");
    let EntityKind::Mesh(m) = &e.kind else { panic!("a mesh") };
    let mut b = [f64::INFINITY, f64::INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY];
    let (mut z0, mut z1) = (f64::INFINITY, f64::NEG_INFINITY);
    for v in &m.vertices {
        b = [b[0].min(v.x), b[1].min(v.y), b[2].max(v.x), b[3].max(v.y)];
        z0 = z0.min(v.z);
        z1 = z1.max(v.z);
    }
    (b, z0, z1)
}

#[test]
fn a_wall_device_is_set_in_its_wall_flush_with_the_face_of_the_room_it_serves() {
    let _g = serial();
    let mut s = Session::empty();
    s.open_drawing(two_rooms(), "Shared Wall", None);
    room_tags(&mut s);
    let tags = vec![
        // Read 2" west of the partition: in its west face.
        device(19.6, 5.0, "A", "receptacle", "1F0", json!({})),
        // Read inside the partition, near its east face: in its east face.
        device(20.2, 5.0, "B", "receptacle", "1F1", json!({})),
        // On the partition's centreline: the side its circuit's other devices are on (east).
        device(20.0, 3.0, "C", "receptacle", "1F2", json!({"circuit": "LP-1"})),
        device(30.0, 1.0, "D", "receptacle", "1F3", json!({"circuit": "LP-1"})),
        device(35.0, 9.0, "E", "receptacle", "1F4", json!({"circuit": "LP-1"})),
        // On the centreline, nothing says a side.
        device(20.0, 7.0, "F", "receptacle", "1F5", json!({})),
        // A receptacle read 4' into the west room: in the nearest wall, marked for review.
        device(10.0, 4.5, "G", "receptacle", "1F6", json!({})),
    ];
    build(&mut s, spec(tags, json!({})));
    let face_in = |x: f64| x * 12.0;
    // A: the box in the partition, its face on the west face (19.75') within 1/4".
    let (a, _, _) = extent(&mut s, "A");
    assert!((a[0] - face_in(19.75)).abs() <= 0.25 && a[2] <= face_in(20.25) + 1e-6, "A in the partition's west face: {a:?}");
    let pa = props(&mut s, "A");
    assert!(pa["host_wall"]["value"].as_str().is_some_and(|h| !h.is_empty()), "its host wall is stored: {pa:#}");
    assert_eq!(pa["host_face"]["value"], "-1.000,0.000", "{pa:#}");
    assert!(pa["placement"]["value"].as_str().is_some_and(|p| p.contains("flush") && p.contains("its symbol is drawn on")), "{pa:#}");
    // B: in the east face (20.25').
    let (b, _, _) = extent(&mut s, "B");
    assert!((b[2] - face_in(20.25)).abs() <= 0.25 && b[0] >= face_in(19.75) - 1e-6, "B in the east face: {b:?}");
    // C: the east face, by its circuit.
    let (c, _, _) = extent(&mut s, "C");
    assert!((c[2] - face_in(20.25)).abs() <= 0.25, "C on its circuit's side: {c:?}");
    assert!(props(&mut s, "C")["placement"]["value"].as_str().is_some_and(|p| p.contains("circuit's other devices")));
    // D: read 0.5' into the east room by the south wall: in its inside face (0.5'), at 30' along.
    let (dd, _, _) = extent(&mut s, "D");
    assert!((dd[3] - face_in(0.5)).abs() <= 0.25 && ((dd[0] + dd[2]) / 2.0 - face_in(30.0)).abs() <= 0.25, "D: {dd:?}");
    // F: on a face of the partition, said so.
    let (f, _, _) = extent(&mut s, "F");
    assert!((f[0] - face_in(19.75)).abs() <= 0.25 || (f[2] - face_in(20.25)).abs() <= 0.25, "F on a face: {f:?}");
    assert!(props(&mut s, "F")["placement"]["value"].as_str().is_some_and(|p| p.contains("centreline")));
    // G: more than 3' from a wall, in the nearest one all the same, for review.
    let pg = props(&mut s, "G");
    assert!(
        pg["host_wall"]["value"].is_string() && pg["placement"]["value"].as_str().is_some_and(|p| p.starts_with("review: in the nearest wall")),
        "{pg:#}"
    );
    // The room each serves is the one past its face.
    s.execute("rooms.build", &json!({})).expect("rooms");
    s.execute("bim.props.fill", &json!({})).expect("fill");
    let (ra, rb) = (props(&mut s, "A")["room"]["value"].clone(), props(&mut s, "B")["room"]["value"].clone());
    assert!(ra.is_string() && rb.is_string() && ra != rb, "A and B serve the rooms on their sides: {ra} {rb}");
    assert_eq!(props(&mut s, "C")["room"]["value"], rb, "C serves its circuit's room");
}

#[test]
fn a_device_whose_wall_is_merged_away_goes_in_the_wall_there_now() {
    let _g = serial();
    let mut s = Session::empty();
    s.open_drawing(two_rooms(), "Merged Wall", None);
    build(&mut s, spec(vec![device(19.6, 5.0, "A", "receptacle", "1F0", json!({}))], json!({})));
    let host = props(&mut s, "A")["host_wall"]["value"].as_str().expect("host").to_string();
    // The wall merged into a copy of itself (as the job's identity merge leaves one of two).
    let copy = {
        let d = s.doc_mut().expect("doc");
        let h = Handle::parse_hex(&host).expect("hex");
        let mut e = (**d.entity(h).expect("wall")).clone();
        e.handle = d.new_handle();
        let c = e.handle;
        d.model.push(e);
        d.model.remove(h);
        c
    };
    let r = s.execute("bim.props.fill", &json!({})).expect("fill");
    assert_eq!(r["reseated"], json!(1), "{r:#}");
    let pa = props(&mut s, "A");
    assert_eq!(pa["host_wall"]["value"], json!(copy.hex()), "{pa:#}");
    let (a, _, _) = extent(&mut s, "A");
    assert!((a[0] - 19.75 * 12.0).abs() <= 0.25, "still flush with the west face: {a:?}");
}

/// A notes list of device heights, a legend, a fire alarm symbol list and an AFF note by a
/// symbol, in the drawing; an accessibility sheet's list (read by the job) in the spec.
fn documented() -> Session {
    let mut s = Session::empty();
    s.open_drawing(two_rooms(), "Heights", None);
    room_tags(&mut s);
    // A symbol with its own AFF note beside it.
    let sym = s.execute("circle", &json!({"center": [60, 30], "radius": 3})).expect("circle")["handle"].as_str().expect("handle").to_string();
    text(&mut s, [60.0, 38.0], "42\" AFF");
    // The legend: a thermostat row with its height.
    text(&mut s, [900.0, 900.0], "ELECTRICAL LEGEND");
    text(&mut s, [880.0, 880.0], "T");
    text(&mut s, [920.0, 880.0], "THERMOSTAT, +46\" AFF");
    // A fire alarm symbol list: a pull station to its top, a sleeping room's detector.
    text(&mut s, [3000.0, 900.0], "FIRE ALARM DEVICE SYMBOL LIST");
    text(&mut s, [2950.0, 870.0], "P");
    text(&mut s, [2980.0, 870.0], "MANUAL FIRE ALARM PULL STATION");
    text(&mut s, [3200.0, 870.0], "4'-0\" TO TOP");
    text(&mut s, [2950.0, 850.0], "S");
    text(&mut s, [2957.0, 848.0], "R");
    text(&mut s, [2980.0, 850.0], "SLEEPING ROOM SMOKE DETECTOR WITH BASE SOUNDER");
    text(&mut s, [3200.0, 850.0], "CEILING OR WALL");
    // A general note listing heights.
    text(&mut s, [2000.0, 400.0], "9.WIRING DEVICE MOUNTED HEIGHTS (UNLESS NOTED):");
    for (i, (what, h)) in [("LIGHT SWITCH", "3'-10\" AFF TO TOP"), ("RECEPTACLES", "1'-4\" CENTERLINE")].iter().enumerate() {
        let y = 391.0 - 9.0 * i as f64;
        text(&mut s, [2012.0, y], "\u{2022}");
        text(&mut s, [2024.0, y], what);
        text(&mut s, [2138.0, y], h);
    }
    text(&mut s, [2000.0, 370.0], "10.LABEL EVERY PANEL DIRECTORY.");
    let tags = vec![
        device(5.0, 0.8, "REC", "receptacle", &sym, json!({})),
        device(8.0, 0.8, "REC2", "receptacle", "1F1", json!({"mount_height_in": "38", "mount_height_note": "38\" AFF on A-501"})),
        device(11.0, 0.8, "REC3", "receptacle", "1F2", json!({})),
        device(14.0, 0.8, "S", "switch", "1F3", json!({})),
        device(17.0, 0.8, "T", "device", "1F4", json!({})),
        device(3.0, 9.2, "Q9", "device", "1F5", json!({})),
        device(25.0, 0.8, "P", "fire_alarm_device", "1F6", json!({})),
        device(30.0, 5.0, "R", "fire_alarm_device", "1F7", json!({})),
        device(22.0, 9.0, "P-1", "plumbing_fixture", "1F8", json!({"fixture": "WATER CLOSET"})),
        device(26.0, 9.0, "P-1H", "plumbing_fixture", "1F9", json!({"fixture": "WATER CLOSET"})),
        device(34.0, 9.2, "P-2", "plumbing_fixture", "1FA", json!({"fixture": "LAVATORY"})),
        device(38.0, 9.0, "FD", "plumbing_fixture", "1FB", json!({"fixture": "FLOOR DRAIN"})),
    ];
    let notes = json!({"mount_notes": [
        {"sheet": "G-001", "subject": "WATER CLOSETS", "inches": 15.5, "ada_inches": 18.0, "datum": "top",
         "text": "WATER CLOSETS (TO TOP OF SEAT): STANDARD = 15\" TO 16\". ADA (TO TOP OF SEAT) = 17\" TO 19\"."},
        {"sheet": "G-001", "subject": "LAVATORIES", "inches": 35.0, "ada_inches": 34.0, "datum": "top",
         "text": "LAVATORIES (TO RIM): STANDARD = 35\" MAX. ADA = 34\" MAX."}
    ]});
    build(&mut s, spec(tags, notes));
    s
}

#[test]
fn heights_come_from_the_documents_in_order_with_their_source() {
    let _g = serial();
    let mut s = documented();
    let h = |s: &mut Session, mark: &str| -> (Value, Value, Value) {
        let p = props(s, mark);
        (p["mount_height"]["value"].clone(), p["mount_height"]["source"]["kind"].clone(), p["mount_height_from"]["value"].clone())
    };
    // Its own AFF note by its symbol, before the general note's 16".
    assert_eq!(h(&mut s, "REC"), (json!(42.0), json!("note"), json!("own_note")));
    // Its typical room's enlarged plan note.
    assert_eq!(h(&mut s, "REC2"), (json!(38.0), json!("note"), json!("enlarged_plan_note")));
    // The general notes' list: a receptacle to its middle, a switch to its top.
    assert_eq!(h(&mut s, "REC3"), (json!(16.0), json!("note"), json!("general_notes")));
    assert_eq!(h(&mut s, "S"), (json!(46.0), json!("note"), json!("general_notes")));
    let (_, z0, z1) = extent(&mut s, "S");
    assert!((z1 - 46.0).abs() < 1e-6 && (z0 - 42.0).abs() < 1e-6, "the switch's top at 46\": {z0}..{z1}");
    assert_eq!(props(&mut s, "S")["mount_datum"]["value"], "top");
    let (_, z0, z1) = extent(&mut s, "REC3");
    assert!(((z0 + z1) / 2.0 - 16.0).abs() < 1e-6, "the receptacle's middle at 16\"");
    // The legend's row.
    assert_eq!(h(&mut s, "T"), (json!(46.0), json!("legend"), json!("legend")));
    // A pull station by its symbol list's row, to its top.
    assert_eq!(h(&mut s, "P"), (json!(48.0), json!("legend"), json!("legend")));
    // No document: the rules data's usual height, marked a default.
    let q9 = props(&mut s, "Q9");
    assert_eq!(q9["mount_height"]["source"]["kind"], "default", "{q9:#}");
    assert_eq!(q9["mount_height_from"]["value"], "default");
    assert!(q9["mount_height"]["source"]["doc"].as_str().is_some_and(|d| d.starts_with("rules data")), "{q9:#}");
    assert!(q9["mount_height"]["note"].as_str().is_some_and(|n| n.contains("not printed")), "{q9:#}");
}

#[test]
fn ceiling_devices_sit_at_the_ceiling_and_floor_fixtures_on_the_floor() {
    let _g = serial();
    let mut s = documented();
    // A sleeping room's detector (its legend row: a smoke detector) at the 9' ceiling.
    let r = props(&mut s, "R");
    assert_eq!(r["mounting"]["value"], "ceiling", "{r:#}");
    assert_eq!(r["mount_height"]["value"], json!(108.0), "{r:#}");
    let (_, _, z1) = extent(&mut s, "R");
    assert!((z1 - 108.0).abs() < 1e-6, "its top at the ceiling: {z1}");
    // A water closet on the floor up to its seat (the accessibility list), an accessible one's
    // to the ADA height; a floor drain in the floor.
    let (_, z0, z1) = extent(&mut s, "P-1");
    assert!(z0.abs() < 1e-6 && (z1 - 15.5).abs() < 1e-6, "P-1 from the floor to its seat: {z0}..{z1}");
    let p1 = props(&mut s, "P-1");
    assert_eq!((p1["mounting"]["value"].clone(), p1["mount_height"]["source"]["kind"].clone()), (json!("floor"), json!("note")), "{p1:#}");
    let (_, _, z1) = extent(&mut s, "P-1H");
    assert!((z1 - 18.0).abs() < 1e-6, "the accessible one's seat at 18\": {z1}");
    let (_, z0, _) = extent(&mut s, "FD");
    assert!(z0.abs() < 1e-6, "the floor drain at the floor");
    assert_eq!(props(&mut s, "FD")["mount_height_from"]["value"], "floor");
    // A lavatory on its wall, its rim at the list's 35".
    let (_, _, z1) = extent(&mut s, "P-2");
    assert!((z1 - 35.0).abs() < 1e-6, "the lavatory's rim at 35\": {z1}");
    assert_eq!(props(&mut s, "P-2")["mounting"]["value"], "wall");
}

#[test]
fn g_height_compares_a_height_to_its_top_at_the_devices_top() {
    let _g = serial();
    let mut s = documented();
    let all: Vec<Value> = {
        let mut out = Vec::new();
        let mut cursor = 0u64;
        loop {
            let r = s.execute("validate.prescreen", &json!({"all": true, "limit": 50, "cursor": cursor, "rules": ["G-HEIGHT"]})).expect("prescreen");
            out.extend(r["findings"].as_array().cloned().unwrap_or_default());
            match r["next"]["cursor"].as_u64() {
                Some(c) => cursor = c,
                None => break,
            }
        }
        out
    };
    let sw: Vec<&Value> = all.iter().filter(|f| f["mark"] == "S").collect();
    assert!(!sw.is_empty(), "{all:?}");
    for f in sw {
        assert_eq!(f["verdict"], "pass", "{f:#}");
        assert_eq!(f["measured"]["to"], "top", "{f:#}");
        assert!((f["measured"]["aff_in"].as_f64().unwrap_or(0.0) - 46.0).abs() < 0.01, "{f:#}");
    }
}

#[test]
fn a_device_outside_its_levels_footprint_is_flagged_and_listed() {
    let _g = serial();
    let mut s = Session::empty();
    s.open_drawing(two_rooms(), "Outside", None);
    room_tags(&mut s);
    let tags = vec![device(10.0, 0.8, "A", "receptacle", "1F0", json!({})), device(80.0, 5.0, "B", "receptacle", "1F1", json!({}))];
    let r = {
        s.execute("bspec.set", &json!({"replace": true, "spec": spec(tags, json!({}))})).expect("spec");
        s.execute("model.build", &json!({})).expect("build")
    };
    let db = &r["deviceBuild"];
    assert_eq!(db["misplaced"], json!(1), "{db:#}");
    assert!(db["misplaced_first"][0]["why"].as_str().is_some_and(|w| w.contains("outside L1's footprint")), "{db:#}");
    let b = props(&mut s, "B");
    assert!(b["misplaced"]["value"].as_str().is_some_and(|w| w.contains("ft outside")), "{b:#}");
    assert!(props(&mut s, "A")["misplaced"].is_null());
    // Validate lists every element outside its footprint, and that it was flagged.
    let p = s.execute("validate.prescreen", &json!({"limit": 1})).expect("prescreen");
    assert_eq!(p["outside_floors"]["count"], json!(1), "{p:#}");
    assert_eq!(p["outside_floors"]["flagged"], json!(1), "{p:#}");
    assert_eq!(p["outside_floors"]["by_part"]["device"], json!(1), "{p:#}");
}

// ─── The job ────────────────────────────────────────────────────────────────

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

#[test]
fn the_job_says_where_each_devices_height_came_from_and_what_it_kept_out() {
    let _g = serial();
    real_pdf_reader();
    let mut set = generate_hotel(42, HotelSize::Small).expect("generate hotel");
    set.truth.sheets.retain(|s| ["A-101", "E-201"].contains(&s.number.as_str()));
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos());
    let dir = std::env::temp_dir().join(format!("cadaddy_devicefit_{}_{nanos}", std::process::id()));
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
    assert_eq!(st["done"], json!(true), "{st}");
    let p = &st["placement"];
    let from = p["devices"]["heights_from"].as_object().expect("heights_from");
    assert!(from.get("own_note").and_then(Value::as_u64).is_some_and(|n| n > 0), "the plan's AFF notes by its switches: {p:#}");
    assert!(p["left_out"].is_array(), "what no registration placed, listed: {p:#}");
    assert!(p["misplaced"]["count"].is_u64(), "what lies outside the footprint, counted: {p:#}");
    let report = s.execute("job.report", &json!({})).expect("report");
    assert_eq!(report["placement"], *p, "the report says the same");
    let _ = std::fs::remove_dir_all(&dir);
}
