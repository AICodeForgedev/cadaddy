//! Q6174 walllayout: named wall faces, devices placed relative to a face, constraints to casework
//! and appliances, faceplates, the field layout. Made-up plans only.

use std::sync::{Mutex, MutexGuard};

use cadaddy_engine::Session;
use cadaddy_engine::doc::{Drawing, Entity, EntityKind, Handle, Line};
use cadaddy_engine::geom::Vec3;
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

/// Two rooms side by side, 6" walls: 1001 west 0.5–19.75', 1002 east 20.25–40', 0.5–9.5' deep.
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

fn room_tags(s: &mut Session) {
    text_on(s, "A-ROOM-TAG", [110.0, 64.0], "OFFICE");
    text_on(s, "A-ROOM-TAG", [116.0, 52.0], "1001");
    text_on(s, "A-ROOM-TAG", [350.0, 64.0], "STORAGE");
    text_on(s, "A-ROOM-TAG", [356.0, 52.0], "1002");
}

fn device(x: f64, y: f64, mark: &str, kind: &str, symbol: &str, extra: Value) -> Value {
    let mut fields = json!({
        "type": {"value": mark, "source": "plan"},
        "device_type": {"value": format!("{kind} {mark}"), "source": "plan"},
        "sheet": {"value": "E-901", "source": "plan"},
        "symbol": {"value": symbol, "source": "plan"},
    });
    if let (Some(f), Some(x)) = (fields.as_object_mut(), extra.as_object()) {
        for (k, v) in x {
            f.insert(k.clone(), json!({"value": v, "source": "plan"}));
        }
    }
    json!({"id": format!("device:L1:{x:.2},{y:.2}:{mark}"), "kind": kind, "level": "L1", "at_ft": [x, y], "fields": fields})
}

fn spec(tags: Vec<Value>) -> Value {
    json!({
        "version": 1, "units_per_foot": 12,
        "levels": [{"name": "L1", "elevation_ft": 0, "height_ft": 10, "ceiling_ft": 9, "base": [0, 0]}],
        "layers": [{"layer": "A-WALL", "role": "wall", "wall_type": "Partition"}, {"layer": "A-ROOM-TAG", "role": "room_tag"}],
        "wall_types": [{"name": "Partition", "thickness_in": 6}],
        "tags": tags,
    })
}

/// A 66" AFF note on the device (a made-up elevation).
fn at66() -> Value {
    json!({"mount_height_in": "66", "mount_height_note": "66\" AFF on A-900"})
}

/// The made-up job: the two rooms, devices, rooms traced and properties filled.
fn job(extra_objects: bool) -> Session {
    let mut s = Session::empty();
    s.open_drawing(two_rooms(), "Layout Rooms", None);
    room_tags(&mut s);
    let tags = vec![
        // 1001: a receptacle on the south face 15" from the east face; one in the partition.
        device(18.5, 0.6, "A", "receptacle", "1F0", json!({})),
        device(19.6, 5.0, "B", "receptacle", "1F1", json!({})),
        // Two switches side by side (1.8") at one height: one 2-gang plate.
        device(10.0, 9.4, "S", "switch", "1F2", json!({})),
        device(10.15, 9.4, "S2", "switch", "1F3", json!({})),
        // A third read at the first one's spot (read twice): one gang, said stacked.
        device(10.01, 9.4, "S3", "switch", "1FC", json!({})),
        // 1002: a refrigerator receptacle behind it, and one beside it (exposed).
        device(26.25, 0.6, "R", "receptacle", "1F4", json!({})),
        device(28.0, 0.6, "X", "receptacle", "1F5", json!({})),
        // A microwave receptacle centred in an upper cabinet, and one 4" off its centre.
        device(34.25, 0.6, "M", "receptacle", "1F6", at66()),
        device(34.25 + 4.0 / 12.0, 0.6, "M2", "receptacle", "1F7", at66()),
        // Jumbo and standard plates centred in narrow cabinets.
        device(37.0, 0.6, "J", "receptacle", "1F8", {
            let mut v = at66();
            v["device_type"] = json!("receptacle J, JUMBO PLATE");
            v
        }),
        device(38.5, 0.6, "K", "receptacle", "1F9", at66()),
        // A dishwasher outlet in the sink base, and one behind the dishwasher.
        device(13.25, 9.4, "D1", "receptacle", "1FA", json!({})),
        device(15.5, 9.4, "D2", "receptacle", "1FB", json!({})),
        // A receptacle its own words say serves a refrigerator, inside REF-1's footprint.
        device(26.25 + 4.0 / 12.0, 0.6, "F", "receptacle", "1FD", json!({"device_type": "receptacle F, REFRIGERATOR"})),
        // 1001 north: two receptacles 2.9" apart at one height (their 2.75" plates + gap would
        // overlap: one box) and two 3.5" apart (two plates, clear).
        device(5.0, 9.4, "G1", "receptacle", "1FE", json!({})),
        device(5.0 + 2.9 / 12.0, 9.4, "G2", "receptacle", "1FF", json!({})),
        device(7.0, 9.4, "G3", "receptacle", "200", json!({})),
        device(7.0 + 3.5 / 12.0, 9.4, "G4", "receptacle", "201", json!({})),
    ];
    s.execute("bspec.set", &json!({"replace": true, "spec": spec(tags)})).expect("spec");
    s.execute("model.build", &json!({})).expect("build");
    s.execute("rooms.build", &json!({})).expect("rooms");
    s.execute("bim.props.fill", &json!({})).expect("fill");
    if extra_objects {
        let obj = |s: &mut Session, kind: &str, tag: &str, b: [f64; 6]| {
            s.execute("bim.layout.object", &json!({"kind": kind, "tag": tag, "level": "L1", "box": b, "source": "made-up submittal"}))
                .expect("object");
        };
        // 1002's south wall (face at y = 6"): a refrigerator 30" wide at 25'–27.5'.
        obj(&mut s, "refrigerator", "REF-1", [300.0, 6.0, 0.0, 330.0, 36.0, 70.0]);
        // An upper cabinet 30" wide centred on 34.25'.
        obj(&mut s, "upper_cabinet", "UPPER-3", [396.0, 6.0, 54.0, 426.0, 18.0, 84.0]);
        // Two narrow cabinets 3 1/4" wide centred on 37' and 38.5'.
        obj(&mut s, "upper_cabinet", "CAB-N1", [444.0 - 1.625, 6.0, 54.0, 444.0 + 1.625, 18.0, 84.0]);
        obj(&mut s, "upper_cabinet", "CAB-N2", [462.0 - 1.625, 6.0, 54.0, 462.0 + 1.625, 18.0, 84.0]);
        // 1001's north wall (face at y = 114"): a sink base 12'–14.5', a dishwasher 14.5'–16.5'.
        obj(&mut s, "sink_base", "SB-1", [144.0, 90.0, 0.0, 174.0, 114.0, 34.5]);
        obj(&mut s, "dishwasher", "DW-1", [174.0, 90.0, 0.0, 198.0, 114.0, 34.0]);
    }
    s
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

fn num(p: &Value, key: &str) -> f64 {
    p[key]["value"].as_f64().unwrap_or_else(|| panic!("{key} in {p:#}"))
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
fn a_room_has_four_named_faces_and_a_demising_wall_two_names() {
    let _g = serial();
    let mut s = job(false);
    let r = s.execute("bim.faces", &json!({"room": "1001"})).expect("faces");
    let names: Vec<&str> = r["faces"].as_array().expect("faces").iter().filter_map(|f| f["name"].as_str()).collect();
    for n in ["1001/N", "1001/S", "1001/E", "1001/W"] {
        assert!(names.contains(&n), "{n} in {names:?}: {r:#}");
    }
    let face =
        |r: &Value, n: &str| r["faces"].as_array().and_then(|a| a.iter().find(|f| f["name"] == n).cloned()).unwrap_or_else(|| panic!("{n}: {r:#}"));
    let e101 = face(&r, "1001/E");
    let r2 = s.execute("bim.faces", &json!({"room": "1002"})).expect("faces");
    let w102 = face(&r2, "1002/W");
    assert_eq!(e101["wall"], w102["wall"], "the partition is 1001/E on one face and 1002/W on the other");
    assert_eq!(e101["wall_type"], "Partition");
    let s101 = face(&r, "1001/S");
    assert_eq!((s101["prev"].as_str(), s101["next"].as_str()), (Some("1001/W"), Some("1001/E")), "corner order: {s101:#}");
    assert!((s101["length_in"].as_f64().unwrap() - 231.0).abs() < 0.5, "{s101:#}");
}

#[test]
fn a_device_is_placed_from_its_faces_and_follows_its_wall() {
    let _g = serial();
    let mut s = job(false);
    let r = s.execute("bim.layout.rehost", &json!({})).expect("rehost");
    assert!(r["devices"]["on_face"].as_u64().is_some_and(|n| n >= 10), "{r:#}");
    assert!(r["fields"].as_array().is_some_and(|f| f.len() == 6), "{r:#}");
    // A: on 1001/S, 18" AFF to its centre, 15" from 1001/E (measured from the plan: derived), flush.
    let a = props(&mut s, "A");
    assert_eq!(a["host_face"]["value"], "1001/S", "{a:#}");
    assert_eq!(a["height_ref"]["value"], "centre");
    assert!((num(&a, "height_aff") - 18.0).abs() < 1e-6, "{a:#}");
    assert_eq!(a["offset_from"]["value"], "1001/E", "{a:#}");
    assert!((num(&a, "offset_along") - 15.0).abs() <= 0.25, "{a:#}");
    assert!(a["offset_along"]["note"].as_str().is_some_and(|n| n.contains("derived: measured from the plan")), "{a:#}");
    assert!(num(&a, "offset_depth").abs() <= 0.25, "flush: {a:#}");
    assert_eq!(a["layout_solve"]["value"], "pending framing", "no studs yet: {a:#}");
    // B: in the partition's west face (1001/E).
    let b = props(&mut s, "B");
    assert_eq!(b["host_face"]["value"], "1001/E", "{b:#}");
    let (_, az0, az1) = extent(&mut s, "A");
    // Move the partition 1 ft east: A keeps 15" from 1001/E, B stays in its face, heights hold.
    let faces = s.execute("bim.faces", &json!({"room": "1001"})).expect("faces");
    let wall = faces["faces"].as_array().unwrap().iter().find(|f| f["name"] == "1001/E").unwrap()["wall"].as_str().unwrap().to_string();
    s.execute("move", &json!({"handles": [wall], "delta": [12.0, 0.0]})).expect("move");
    let f = s.execute("bim.layout.follow", &json!({})).expect("follow");
    assert!(f["moved"].as_u64().is_some_and(|n| n >= 2), "{f:#}");
    let (a2, az0b, az1b) = extent(&mut s, "A");
    let ac = (a2[0] + a2[2]) / 2.0;
    assert!((ac - (20.75 * 12.0 - 15.0)).abs() <= 0.25, "A 15\" from the moved face: {a2:?}");
    assert!((az0 - az0b).abs() < 1e-6 && (az1 - az1b).abs() < 1e-6, "its height never changes");
    let (b2, _, _) = extent(&mut s, "B");
    // Its front on the moved face (20.75'), its box in the wall behind it.
    assert!((b2[0] - 20.75 * 12.0).abs() <= 0.25 && b2[2] > b2[0], "B's front on the moved face: {b2:?}");
}

#[test]
fn the_layout_lists_each_face_in_field_terms_and_draws_a_sheet() {
    let _g = serial();
    let mut s = job(false);
    let r = s.execute("bim.layout", &json!({"room": "1001", "draw": true})).expect("layout");
    let text = r["text"].as_str().expect("text");
    let south = text.lines().find(|l| l.starts_with("1001/S")).unwrap_or_else(|| panic!("{text}"));
    assert!(south.contains("REC (A) 18 in AFF ctr") && south.contains("15 in from 1001/E"), "{south}");
    assert!(south.contains("band ±6 in") && south.contains("pending framing") && south.contains("plate standard"), "{south}");
    assert!(text.lines().any(|l| l.starts_with("1001/N")), "{text}");
    assert!(r["sheet"]["entities"].as_u64().is_some_and(|n| n > 10) && r["sheet"]["dimensions"].as_u64().is_some_and(|n| n >= 2), "{r:#}");
    let name = r["sheet"]["layout"].as_str().unwrap().to_string();
    let lays = s.execute("layout.list", &json!({})).expect("layouts");
    assert!(lays.to_string().contains(&name), "{lays}");
}

#[test]
fn two_switches_side_by_side_share_one_two_gang_plate() {
    let _g = serial();
    let mut s = job(false);
    let r = s.execute("bim.layout.rehost", &json!({})).expect("rehost");
    assert!(r["plates"]["ganged"].as_u64().is_some_and(|n| n >= 1), "{r:#}");
    let (a, b) = (props(&mut s, "S"), props(&mut s, "S2"));
    assert_eq!(a["plate_group"]["value"], b["plate_group"]["value"], "{a:#}\n{b:#}");
    assert!(a["faceplate"]["value"].as_str().is_some_and(|p| p.contains("2-gang")), "{a:#}");
    assert!(r["plates"]["stacked"].as_u64().is_some_and(|n| n >= 1), "{r:#}");
    assert_eq!(props(&mut s, "S3")["plate_group"]["value"], a["plate_group"]["value"]);
    // One plate: no overlap finding between them; its plate check passes.
    let fs = layout_findings(&mut s, "S");
    let plate = verdict(&fs, "plate");
    assert!(!plate.is_empty() && plate.iter().all(|v| v.starts_with("pass")), "{plate:?}");
}

/// The G-LAYOUT findings of a device by check.
fn layout_findings(s: &mut Session, mark: &str) -> Vec<Value> {
    let h = handle_of(s, mark);
    let mut out = Vec::new();
    let mut cursor = 0;
    loop {
        let r = s.execute("validate.prescreen", &json!({"all": true, "limit": 50, "cursor": cursor, "rules": ["G-LAYOUT"]})).expect("prescreen");
        out.extend(r["findings"].as_array().unwrap().iter().filter(|f| f["handle"] == h.as_str()).cloned());
        match r["next"]["cursor"].as_u64() {
            Some(n) if n > cursor => cursor = n,
            _ => break,
        }
    }
    out
}

fn verdict(fs: &[Value], check: &str) -> Vec<String> {
    fs.iter()
        .filter(|f| f["check"] == check)
        .map(|f| format!("{} {}", f["verdict"].as_str().unwrap_or(""), f["issue"].as_str().unwrap_or("")))
        .collect()
}

#[test]
fn appliance_and_cabinet_constraints_by_their_plates_edges() {
    let _g = serial();
    let mut s = job(true);
    let r = s.execute("bim.layout.rehost", &json!({})).expect("rehost");
    assert!(r["objects"].as_u64() == Some(6), "{r:#}");
    // The refrigerator's receptacle: concealed behind it (rules data, derived) — passes.
    let rp = props(&mut s, "R");
    assert!(rp["constraint_1"]["value"].as_str().is_some_and(|c| c.contains("concealed behind REF-1")), "{rp:#}");
    assert_eq!(rp["constraint_1"]["source"]["kind"], "rule", "{rp:#}");
    let fr = layout_findings(&mut s, "R");
    assert!(verdict(&fr, "constraint").iter().all(|v| v.starts_with("pass")), "{fr:#?}");
    // The one beside it is exposed: its plate's east edge past the refrigerator's footprint.
    let fx = layout_findings(&mut s, "X");
    let vx = verdict(&fx, "constraint");
    assert!(vx.iter().any(|v| v.starts_with("fail") && v.contains("past REF-1") && v.contains("east edge")), "{vx:?}");
    // Microwave receptacle in UPPER-3: tied by an elevation (made-up) ±2": centred passes, 4" off fails.
    for m in ["M", "M2"] {
        let h = handle_of(&mut s, m);
        s.execute(
            "bim.layout.tie",
            &json!({"handle": h, "ref": "UPPER-3", "plus_minus_in": 2, "inside": true, "source": "interior elevation (made-up)"}),
        )
        .expect("constrain");
    }
    let vm = verdict(&layout_findings(&mut s, "M"), "constraint");
    assert!(!vm.is_empty() && vm.iter().all(|v| v.starts_with("pass")), "{vm:?}");
    let vm2 = verdict(&layout_findings(&mut s, "M2"), "constraint");
    assert!(vm2.iter().any(|v| v.starts_with("fail") && v.contains("off UPPER-3's centre")), "{vm2:?}");
    // A jumbo plate centred in a 3 1/4" cabinet crosses its edges; a standard plate fits.
    assert!(props(&mut s, "J")["faceplate"]["value"].as_str().is_some_and(|p| p.starts_with("jumbo")));
    let vj = verdict(&layout_findings(&mut s, "J"), "constraint");
    assert!(vj.iter().any(|v| v.starts_with("fail") && v.contains("plate") && v.contains("past CAB-N1")), "{vj:?}");
    let vk = verdict(&layout_findings(&mut s, "K"), "constraint");
    assert!(!vk.is_empty() && vk.iter().all(|v| v.starts_with("pass")), "{vk:?}");
    // The dishwasher outlet in the sink base beside it passes; one behind the dishwasher fails.
    let v1 = verdict(&layout_findings(&mut s, "D1"), "constraint");
    assert!(!v1.is_empty() && v1.iter().all(|v| v.starts_with("pass")), "{v1:?}");
    let v2 = verdict(&layout_findings(&mut s, "D2"), "constraint");
    assert!(v2.iter().any(|v| v.starts_with("fail") && v.contains("SB-1")), "{v2:?}");
    // The cabinet moves 6": the microwave receptacle tied to it follows; its height holds.
    let (m0, z0, _) = extent(&mut s, "M");
    let up = {
        let d = s.doc().expect("doc");
        d.model.iter().find(|e| e.common.xdata.iter().any(|t| t.value == "UPPER-3")).map(|e| e.handle.hex()).expect("cabinet")
    };
    s.execute("move", &json!({"handles": [up], "delta": [6.0, 0.0]})).expect("move");
    s.execute("bim.layout.follow", &json!({})).expect("follow");
    let (m1, z1, _) = extent(&mut s, "M");
    assert!(((m1[0] + m1[2]) / 2.0 - (m0[0] + m0[2]) / 2.0 - 6.0).abs() <= 0.25, "{m0:?} → {m1:?}");
    assert!((z0 - z1).abs() < 1e-6);
    // The layout prints the tie.
    let lay = s.execute("bim.layout", &json!({"room": "1002"})).expect("layout");
    assert!(lay["text"].as_str().is_some_and(|t| t.contains("ctr inside UPPER-3 ±2 in") && t.contains("66 in AFF")), "{}", lay["text"]);
}

#[test]
fn a_tie_whose_object_is_missing_is_pending_never_pass_or_fail() {
    let _g = serial();
    // No refrigerator in the model: F's tie waits for it.
    let mut s = job(false);
    let r = s.execute("bim.layout.rehost", &json!({})).expect("rehost");
    assert!(r["constraints"]["pending object (refrigerator)"].as_u64().is_some_and(|n| n >= 1), "{r:#}");
    let f = props(&mut s, "F");
    assert!(f["constraint_1"]["value"].as_str().is_some_and(|c| c.contains("\"pending_object\":\"refrigerator\"")), "{f:#}");
    let vf = verdict(&layout_findings(&mut s, "F"), "constraint");
    assert!(!vf.is_empty() && vf.iter().all(|v| v.starts_with("unsure pending object (refrigerator)")), "{vf:?}");
    // A pending tie doesn't narrow the band: F keeps the receptacle's.
    assert!((num(&f, "along_band") - 6.0).abs() < 1e-6, "{f:#}");
    let lay = s.execute("bim.layout", &json!({"room": "1002"})).expect("layout");
    assert!(lay["text"].as_str().is_some_and(|t| t.contains("pending object (refrigerator)")), "{}", lay["text"]);
    // With the refrigerator placed, the same receptacle is tied to it and checked.
    let mut s = job(true);
    s.execute("bim.layout.rehost", &json!({})).expect("rehost");
    let f = props(&mut s, "F");
    assert!(
        f["constraint_1"]["value"]
            .as_str()
            .is_some_and(|c| c.contains("concealed behind REF-1") && !c.contains("\"pending_object\":\"refrigerator\"")),
        "{f:#}"
    );
    let vf = verdict(&layout_findings(&mut s, "F"), "constraint");
    assert!(!vf.is_empty() && vf.iter().all(|v| v.starts_with("pass")), "{vf:?}");
    // A tie whose object is erased afterwards: pending, with the object's kind, never pass or fail.
    let h = handle_of(&mut s, "M");
    s.execute(
        "bim.layout.tie",
        &json!({"handle": h, "ref": "UPPER-3", "plus_minus_in": 2, "inside": true, "source": "interior elevation (made-up)"}),
    )
    .expect("tie");
    let up = {
        let d = s.doc().expect("doc");
        d.model.iter().find(|e| e.common.xdata.iter().any(|t| t.value == "UPPER-3")).map(|e| e.handle.hex()).expect("cabinet")
    };
    s.execute("erase", &json!({"handles": [up]})).expect("erase");
    let vm = verdict(&layout_findings(&mut s, "M"), "constraint");
    assert!(vm.iter().any(|v| v.starts_with("unsure pending object (upper_cabinet)")), "{vm:?}");
    assert!(!vm.iter().any(|v| v.starts_with("pass") || v.starts_with("fail")), "{vm:?}");
}

#[test]
fn plates_that_would_overlap_are_one_ganged_box() {
    let _g = serial();
    let mut s = job(false);
    let r = s.execute("bim.layout.rehost", &json!({})).expect("rehost");
    let (g1, g2, g3, g4) = (props(&mut s, "G1"), props(&mut s, "G2"), props(&mut s, "G3"), props(&mut s, "G4"));
    // 2.9" apart: closer than a 2.75" plate + the gap, so one 2-gang box, said derived.
    assert_eq!(g1["plate_group"]["value"], g2["plate_group"]["value"], "{g1:#}\n{g2:#}");
    assert!(g1["faceplate"]["value"].as_str().is_some_and(|p| p.contains("2-gang")), "{g1:#}");
    assert!(g1["faceplate"]["note"].as_str().is_some_and(|n| n.contains("derived: plates would overlap")), "{g1:#}");
    let (h1, h2) = (handle_of(&mut s, "G1"), handle_of(&mut s, "G2"));
    let listed = r["plates"]["ganged_first"].as_array().expect("ganged list");
    assert!(
        listed.iter().any(|g| {
            let d = g["devices"].to_string();
            g["why"] == "derived: plates would overlap" && d.contains(&h1) && d.contains(&h2)
        }),
        "{listed:#?}"
    );
    let spacing = listed.first().and_then(|g| g["spacing_in"].as_f64()).unwrap_or(0.0);
    assert!(spacing > 2.75 && spacing < 3.5, "plate width + gap: {spacing}");
    // 3.5" apart: two plates, clear of each other.
    assert_ne!(g3["plate_group"]["value"], g4["plate_group"]["value"], "{g3:#}\n{g4:#}");
    assert!(g3["faceplate"]["value"].as_str().is_some_and(|p| p.contains("1-gang")), "{g3:#}");
    for m in ["G1", "G2", "G3", "G4"] {
        let plate = verdict(&layout_findings(&mut s, m), "plate");
        assert!(!plate.is_empty() && plate.iter().all(|v| v.starts_with("pass")), "{m}: {plate:?}");
    }
}

#[test]
fn hostile_layout_params_are_refused() {
    let _g = serial();
    let mut s = job(false);
    assert!(s.execute("bim.layout", &json!({})).is_err());
    assert!(s.execute("bim.layout", &json!({"room": "nowhere"})).is_err());
    assert!(s.execute("bim.faces", &json!({"north": 1e300})).is_err());
    assert!(s.execute("bim.layout.object", &json!({"kind": "range", "box": [0, 0, 0, -1, 1, 1]})).is_err());
    assert!(s.execute("bim.layout.object", &json!({"kind": "range", "box": [0, 0]})).is_err());
    let h = handle_of(&mut s, "A");
    assert!(s.execute("bim.layout.tie", &json!({"handle": h, "ref": "none", "plus_minus_in": 2})).is_err());
    assert!(s.execute("bim.layout.tie", &json!({"handle": h, "ref": "none", "plus_minus_in": -2})).is_err());
}
