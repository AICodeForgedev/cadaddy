//! G-KEYNOTE (Q6147) on a made-up two-storey job. Level 1: a corridor (100) under a 9 ft
//! ceiling, rooms 101 and 102 behind a 2-hour corridor wall. Its power plan E-101 has a
//! keynote list (1: an 8"x8"x6" box with 1" conduit and pull tape above the corridor's hard lid;
//! 2: a 12"x12"x6" riser box with 2" conduit between the floors; 3: an 8"x8"x6" box; 4: an exit
//! sign) and markers in hexagons: 1 and 4 in the corridor, 3 twice (one by a box of the wrong
//! size, one with nothing there), 2 in room 101; a bare circuit number "1" with no hexagon is no
//! marker. The exit sign is a luminaire whose schedule row says EXIT SIGN. Level 2's plan E-201
//! has the same list and a 2 over the riser, drawn as a number over a mask.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use cadaddy_engine::Session;
use cadaddy_engine::doc::props::{self, Prop, Source};
use cadaddy_engine::doc::{Attrib, AttribModes, AttribProps, Block, Entity, EntityKind, Insert, Line, Text, Wipeout};
use cadaddy_engine::geom::{Vec2, Vec3};
use serde_json::{Value, json};

/// Level 1's plan origin on the drawing (inches); level 2's plan sits above it.
const OX: f64 = 1200.0;
const OY: f64 = 1000.0;
const L2Y: f64 = 4000.0;
/// One sheet: 36" × 24" at 1/8" = 1'-0", in drawing inches.
const SW: f64 = 3456.0;
const SH: f64 = 2304.0;

fn layer(s: &mut Session, name: &str) {
    if s.execute("layer.new", &json!({"name": name, "current": true})).is_err() {
        s.execute("layer.current", &json!({"name": name})).unwrap();
    }
}

/// A building point (feet) on a level's plan, in drawing inches.
fn at_on(level: u8, p: [f64; 2]) -> [f64; 2] {
    let y0 = if level == 2 { L2Y } else { 0.0 };
    [OX + p[0] * 12.0, y0 + OY + p[1] * 12.0]
}

fn at(p: [f64; 2]) -> [f64; 2] {
    at_on(1, p)
}

fn rect(s: &mut Session, level: u8, a: [f64; 2], b: [f64; 2]) {
    let v = [at_on(level, a), at_on(level, [b[0], a[1]]), at_on(level, b), at_on(level, [a[0], b[1]])];
    s.execute("pline", &json!({"vertices": v, "closed": true})).unwrap();
}

fn line(s: &mut Session, a: [f64; 2], b: [f64; 2]) {
    s.execute("line", &json!({"points": [at(a), at(b)]})).unwrap();
}

fn text(s: &mut Session, p: [f64; 2], t: &str, h: f64) {
    s.execute("text", &json!({"at": p, "text": t, "height": h})).unwrap();
}

/// A keynote marker: its number in a hexagon, at a building point of a level's plan.
fn marker(s: &mut Session, level: u8, p: [f64; 2], n: &str) {
    let q = at_on(level, p);
    layer(s, "E-ANNO-KEYN");
    text(s, q, n, 6.0);
    let c = [q[0] + 2.0, q[1] + 3.0];
    let v: Vec<[f64; 2]> = (0..6).map(|i| f64::from(i) * std::f64::consts::PI / 3.0).map(|t| [c[0] + 9.0 * t.cos(), c[1] + 9.0 * t.sin()]).collect();
    s.execute("pline", &json!({"vertices": v, "closed": true})).unwrap();
}

/// A keynote marker drawn as its number over a mask (a wipeout blanking the linework under it).
fn masked_marker(s: Session, level: u8, p: [f64; 2], n: &str) -> Session {
    let mut s = s;
    let q = at_on(level, p);
    layer(&mut s, "E-ANNO-KEYN");
    text(&mut s, q, n, 6.0);
    let mut d = (*s.doc().unwrap()).clone();
    let boundary = [[q[0] - 1.0, q[1] - 2.0], [q[0] + 6.0, q[1] - 2.0], [q[0] + 6.0, q[1] + 8.0], [q[0] - 1.0, q[1] + 8.0]];
    let h = d.new_handle();
    let mut e = Entity::new(h, EntityKind::Wipeout(Wipeout { boundary: boundary.iter().map(|b| Vec2::new(b[0], b[1])).collect() }));
    e.common.layer = "E-ANNO-KEYN".into();
    d.model.push(e);
    let mut s2 = Session::empty();
    s2.open_drawing(d, "Sample Job", None);
    s2
}

/// The sheet's keynote list, left column at a building point (feet) of the level's sheet.
fn keynote_list(s: &mut Session, level: u8) {
    let o = at_on(level, [100.0, 30.0]);
    layer(s, "E-ANNO-TEXT");
    text(s, o, "ELECTRICAL KEYNOTES:", 9.0);
    let lines = [
        (0.0, "1. PROVIDE 8\"X8\"X6\" JUNCTION BOX HERE. RUN 1\" CONDUIT"),
        (14.0, "WITH PULL TAPE ABOVE THE HARD LID CEILING OF THE"),
        (14.0, "CORRIDOR. KEEP THE 2-HOUR WALL RATING."),
        (0.0, "2. RISER BOX FOR THE BDA SYSTEM, 12\"X12\"X6\"."),
        (14.0, "RUN 2\" CONDUIT BETWEEN THE RISER BOXES OF EACH"),
        (14.0, "FLOOR. KEEP THE 2-HOUR RATING OF FLOORS AND WALLS."),
        (0.0, "3. PROVIDE 8\"X8\"X6\" JUNCTION BOX FOR A SPARE."),
        (0.0, "4. PROVIDE ONE EXIT SIGN OVER THE DOOR, SEE THE"),
        (14.0, "FIXTURE SCHEDULE."),
    ];
    for (i, (dx, t)) in lines.iter().enumerate() {
        text(s, [o[0] + dx, o[1] - 14.0 - 10.0 * i as f64], t, 6.0);
    }
}

/// Device blocks of a type (`JBOX`, `XA`) at building points (feet) of a level.
fn boxes(s: Session, items: &[(u8, [f64; 2], &str)]) -> (Session, Vec<String>) {
    let mut d = (*s.doc().unwrap()).clone();
    let style = d.model.iter().find_map(|e| if let EntityKind::Text(t) = &e.kind { Some(t.clone()) } else { None }).unwrap();
    if !d.blocks.contains_key("JBOX") {
        let mut b = Block::new("JBOX");
        let h = d.new_handle();
        b.entities.push(Entity::new(h, EntityKind::Line(Line { a: Vec3::new(-4.0, 0.0, 0.0), b: Vec3::new(4.0, 0.0, 0.0) })));
        d.blocks.insert("JBOX".into(), Arc::new(b));
    }
    let mut hs = Vec::new();
    for (level, p, ty) in items {
        let attribs = vec![Attrib {
            tag: "TYPE".into(),
            text: Text { value: ty.to_string(), ..style.clone() },
            invisible: true,
            constant: false,
            prompt: String::new(),
            props: AttribProps::default(),
            multiline: None,
            modes: AttribModes::default(),
        }];
        let q = at_on(*level, *p);
        let ins = Insert {
            block: "JBOX".into(),
            insert: Vec3::new(q[0], q[1], 0.0),
            scale: Vec3::new(1.0, 1.0, 1.0),
            rotation: 0.0,
            attribs,
            cols: 1,
            rows: 1,
            col_spacing: 0.0,
            row_spacing: 0.0,
            clip: None,
        };
        let h = d.new_handle();
        let mut e = Entity::new(h, EntityKind::Insert(ins));
        e.common.layer = "E-POWR".into();
        d.model.push(e);
        hs.push(h.hex());
    }
    let mut s2 = Session::empty();
    s2.open_drawing(d, "Sample Job", None);
    (s2, hs)
}

/// Store a property on an object, as the job's readers do.
fn set_prop(s: Session, handle: &str, key: &str, value: Value) -> Session {
    let mut d = (*s.doc().unwrap()).clone();
    let h = cadaddy_engine::doc::Handle::parse_hex(handle).unwrap();
    d.modify_entity(h, |e| {
        let mut store = props::read(&e.common.xdata);
        store.set(key, Prop::new(value.clone(), Source::computed()));
        e.common.xdata = props::write(&e.common.xdata, &store).into();
    })
    .unwrap();
    let mut s2 = Session::empty();
    s2.open_drawing(d, "Sample Job", None);
    s2
}

/// The BDA conduit: 1" from the corridor box into room 101 through the 2-hour wall, and the 2"
/// riser in room 101 (`riser_top_ft` above level 1's floor).
fn mep(conduit_in: f64, bottom_ft: f64, riser_top_ft: f64) -> Value {
    json!({
        "systems": [{"name": "BDA", "layer": "E-BDA", "kind": "conduit", "size_in": [1, 0], "bottom_ft": 9.5}],
        "runs": [
            {"system": "BDA", "level": "L1", "points": [at([11.0, 6.0]), at([16.0, 6.0]), at([16.0, 16.0])], "size_in": [conduit_in, 0],
             "bottoms_ft": [bottom_ft, bottom_ft], "size_source": "note", "height_source": "note", "id": "R1"},
            {"system": "BDA", "level": "L1", "points": [at([5.5, 20.0])], "size_in": [2, 0], "bottoms_ft": [1.0], "top_ft": riser_top_ft,
             "size_source": "note", "height_source": "note", "id": "R2"}
        ]
    })
}

fn spec(mep: Value) -> Value {
    let dev = json!({"ceiling": true, "aff_ft": 9.5, "size_ft": [0.5, 0.5, 0.33]});
    let sheet = |n: &str, level: &str, y: f64| {
        json!({"number": n, "title": format!("POWER PLAN - {level}"), "discipline": "E", "level": level, "source": "model", "rect": [0.0, y, SW, y + SH],
               "registration": {"reference": n, "method": "reference", "a": 1.0, "b": 0.0, "tx": 0.0, "ty": 0.0, "matched": 4}})
    };
    json!({
        "units_per_foot": 12,
        "levels": [
            {"name": "L1", "elevation_ft": 0, "height_ft": 10, "ceiling_ft": 9, "base": [OX, OY], "region": [0, 0, SW, SH]},
            {"name": "L2", "elevation_ft": 10, "height_ft": 10, "ceiling_ft": 9, "base": [OX, L2Y + OY], "region": [0, L2Y, SW, L2Y + SH]}
        ],
        "layers": [
            {"layer": "A-WALL-CL", "role": "wall", "wall_type": "Partition"},
            {"layer": "A-WALL-2HR", "role": "wall", "wall_type": "2 HR Corridor"},
            {"layer": "A-WALL", "role": "wall", "wall_type": "Exterior"},
            {"layer": "E-POWR", "role": "device"},
            {"layer": "A-ROOM-TAG", "role": "room_tag"}
        ],
        "wall_types": [
            {"name": "Exterior", "thickness_in": 6},
            {"name": "Partition", "thickness_in": 4.875, "from": "centerline"},
            {"name": "2 HR Corridor", "thickness_in": 6.25, "from": "centerline"}
        ],
        "rooms": [
            {"name": "Corridor", "number": "100", "level": "L1"},
            {"name": "Riser", "number": "101", "level": "L1"},
            {"name": "Office", "number": "102", "level": "L1"},
            {"name": "Riser", "number": "201", "level": "L2"}
        ],
        "devices": {"by_type": {"JBOX": dev}},
        "rcp": {"ceilings": [{"level": "L1", "room": "100", "outline": [at([0.5, 0.5]), at([39.5, 0.5]), at([39.5, 12.0]), at([0.5, 12.0])], "height_ft": 9}]},
        "mep": mep,
        "sheets": [sheet("E-101", "L1", 0.0), sheet("E-201", "L2", L2Y)]
    })
}

struct Job {
    s: Session,
    good_box: String,
    small_box: String,
    riser_box: String,
}

fn job(mep_spec: Value) -> Job {
    job_with(mep_spec, ("bda_riser_box", "12\"X12\"X6\""))
}

/// The job, with level 1's riser box of this kind and size.
fn job_with(mep_spec: Value, riser_box: (&str, &str)) -> Job {
    let mut s = Session::new();
    // Level 1: a 40' × 24' shell, the corridor's 2-hour wall at 12', a partition at 20'.
    layer(&mut s, "A-WALL");
    rect(&mut s, 1, [0.0, 0.0], [40.0, 24.0]);
    rect(&mut s, 1, [0.5, 0.5], [39.5, 23.5]);
    layer(&mut s, "A-WALL-2HR");
    line(&mut s, [0.5, 12.0], [39.5, 12.0]);
    layer(&mut s, "A-WALL-CL");
    line(&mut s, [20.0, 12.0], [20.0, 23.5]);
    // Level 2: the shell.
    layer(&mut s, "A-WALL");
    rect(&mut s, 2, [0.0, 0.0], [40.0, 24.0]);
    rect(&mut s, 2, [0.5, 0.5], [39.5, 23.5]);
    layer(&mut s, "A-ROOM-TAG");
    text(&mut s, at([30.0, 6.0]), "100", 6.0);
    text(&mut s, at([10.0, 18.0]), "101", 6.0);
    text(&mut s, at([30.0, 18.0]), "102", 6.0);
    text(&mut s, at_on(2, [20.0, 12.0]), "201", 6.0);
    // The keynote lists and markers.
    keynote_list(&mut s, 1);
    keynote_list(&mut s, 2);
    marker(&mut s, 1, [10.0, 6.0], "1");
    marker(&mut s, 1, [25.0, 6.0], "3");
    marker(&mut s, 1, [37.0, 6.0], "3");
    marker(&mut s, 1, [31.0, 9.0], "4");
    marker(&mut s, 1, [5.0, 20.0], "2");
    // Level 2's: a number over a mask, no hexagon.
    let mut s = masked_marker(s, 2, [5.0, 20.0], "2");
    // A circuit number, no hexagon round it: not a marker.
    layer(&mut s, "E-ANNO-TEXT");
    text(&mut s, at([18.0, 3.0]), "1", 6.0);
    let (s, hs) =
        boxes(s, &[(1, [11.0, 6.5], "JBOX"), (1, [26.0, 6.5], "JBOX"), (1, [5.5, 20.5], "JBOX"), (2, [5.5, 20.5], "JBOX"), (1, [32.0, 11.5], "XA")]);
    let mut s = s;
    let r = s.execute("bspec.set", &json!({"spec": spec(mep_spec), "replace": true})).unwrap();
    assert!(r["errors"].as_array().is_none_or(|e| e.is_empty()), "{r}");
    s.execute("model.build", &json!({})).unwrap();
    let kinds = ["junction_box", "junction_box", riser_box.0, "bda_riser_box", "wall_luminaire"];
    for (h, k) in hs.iter().zip(kinds) {
        s.execute("bim.source", &json!({"handles": [h], "status": "fixed", "source": "E-101", "kind": k})).unwrap();
    }
    // The light fixture schedule: XA is the exit sign.
    let row = json!({"sheet": "E-601", "schedule": "luminaire", "mark": "XA", "kind": "luminaire", "fields": {"description": "EXIT SIGN, LED, WALL MOUNTED"}});
    s.execute("bim.prints", &json!({"prints": {"schedule": [row]}, "merge": true})).unwrap();
    let sizes = ["8x8x6", "4\"x4\"x2-1/8\"", riser_box.1, "12x12x6"];
    let mut s = s;
    for (h, z) in hs.iter().zip(sizes) {
        s = set_prop(s, h, "size", json!(z));
    }
    Job { s, good_box: hs[0].clone(), small_box: hs[1].clone(), riser_box: hs[2].clone() }
}

fn standard() -> Value {
    mep(1.0, 9.5, 12.0)
}

fn keynote_findings(s: &mut Session, level: &str) -> (Value, Vec<Value>) {
    let r = s.execute("validate.prescreen", &json!({"level": level, "all": true, "limit": 50, "rules": ["G-KEYNOTE"]})).unwrap();
    let fs: Vec<Value> = r["findings"].as_array().cloned().unwrap_or_default();
    assert!(fs.iter().all(|f| f["rule"] == "G-KEYNOTE"), "{r}");
    (r, fs)
}

/// The finding for keynote `n` near building point `p` of kind `kind`.
fn finding<'a>(fs: &'a [Value], n: &str, p: [f64; 2], kind: &str) -> &'a Value {
    fs.iter()
        .find(|f| {
            f["measured"]["keynote"]["number"] == n
                && f["kind"] == kind
                && f["at"].as_array().is_some_and(|a| (a[0].as_f64().unwrap() - p[0]).abs() < 1.0 && (a[1].as_f64().unwrap() - p[1]).abs() < 1.0)
        })
        .unwrap_or_else(|| panic!("no G-KEYNOTE {kind} finding for keynote {n} at {p:?}: {}", Value::Array(fs.to_vec())))
}

#[test]
fn the_rule_is_built_in_with_its_reach_and_size_tolerances() {
    let mut s = Session::new();
    s.execute("new", &json!({})).unwrap();
    let r = s.execute("validate.rules", &json!({"id": "G-KEYNOTE"})).unwrap();
    let rule = &r["rules"][0];
    assert_eq!(rule["enabled"], true, "{r}");
    assert_eq!(rule["source"], "built-in");
    assert_eq!(rule["severity"], "high");
    assert_eq!(rule["tolerance"]["route_ft"], 10.0, "{r}");
    assert_eq!(rule["tolerance"]["size_in"], 0.1, "{r}");
    // A project can widen how far from its marker the called object may be.
    s.execute("validate.rules.set", &json!({"id": "G-KEYNOTE", "tolerance": {"route_ft": 15}, "reason": "markers drawn off to the side"})).unwrap();
    let r = s.execute("validate.rules", &json!({"id": "G-KEYNOTE"})).unwrap();
    assert_eq!(r["rules"][0]["tolerance"]["route_ft"], 15.0, "{r}");
}

#[test]
fn a_box_present_of_the_wrong_size_and_missing_and_the_conduit_with_its_wall() {
    let mut j = job(standard());
    let (r, fs) = keynote_findings(&mut j.s, "L1");
    // Keynote 1's box: present, 8x8x6, about a foot from its marker.
    let b = finding(&fs, "1", [10.0, 6.0], "junction_box");
    assert_eq!(b["verdict"], "pass", "{b}");
    assert_eq!(b["handle"].as_str().unwrap().to_ascii_uppercase(), j.good_box.to_ascii_uppercase());
    assert_eq!(b["measured"]["size_in"], json!([8.0, 8.0, 6.0]), "{b}");
    assert_eq!(b["measured"]["keynote"]["sheet"], "E-101");
    assert!(b["measured"]["keynote"]["text"].as_str().unwrap().starts_with("PROVIDE 8\"X8\"X6\" JUNCTION BOX"), "{b}");
    assert_eq!(b["measured"]["marker"]["sheet"], "E-101");
    assert_eq!(b["area"], "L1/100", "{b}");
    // Keynote 3 by a 4x4 box: the wrong size, a high fail citing the note.
    let small = finding(&fs, "3", [25.0, 6.0], "junction_box");
    assert_eq!(small["verdict"], "fail", "{small}");
    assert_eq!(small["severity"], "high");
    assert_eq!(small["handle"].as_str().unwrap().to_ascii_uppercase(), j.small_box.to_ascii_uppercase());
    assert!(small["issue"].as_str().unwrap().contains("the box there is 4\"x4\"x2-1/8\""), "{small}");
    assert!(small["issue"].as_str().unwrap().contains("keynote 3 on E-101"), "{small}");
    // Keynote 3 with nothing within 10 ft: missing, citing the note and the marker.
    let none = finding(&fs, "3", [37.0, 6.0], "junction_box");
    assert_eq!(none["verdict"], "missing", "{none}");
    assert!(none["handle"].is_null());
    assert!(none["issue"].as_str().unwrap().contains("FOR A SPARE"), "{none}");
    assert!(none["expected"].as_str().unwrap().contains("an 8\"x8\"x6\" junction box within 10 ft"), "{none}");
    // Keynote 1's conduit: 1", above the 9 ft ceiling, in the corridor; it crosses the
    // 2-hour corridor wall into 101 (listed with the rating).
    let c = finding(&fs, "1", [10.0, 6.0], "conduit");
    assert_eq!(c["verdict"], "pass", "{c}");
    assert_eq!(c["handle"], "R1");
    assert_eq!(c["measured"]["call"]["pull_tape"], true);
    assert_eq!(c["measured"]["ceiling"]["height_ft"], 9.0, "{c}");
    assert_eq!(c["measured"]["ceiling"]["from"], "reflected ceiling plan", "{c}");
    assert!(c["measured"]["in_corridor_share"].as_f64().unwrap() > 0.4, "{c}");
    let pen = c["measured"]["penetrations"].as_array().unwrap();
    assert_eq!(pen.len(), 1, "{c}");
    assert_eq!(pen[0]["assembly"], "wall");
    assert_eq!(pen[0]["wall_type"], "2 HR Corridor", "{c}");
    assert_eq!(pen[0]["needs_hr"], 2.0);
    assert_eq!(pen[0]["from"], "wall type and keynote");
    assert!((pen[0]["at_ft"][1].as_f64().unwrap() - 12.0).abs() < 0.2, "{c}");
    // The bare circuit number is no marker; nothing else is checked.
    assert!(fs.iter().all(|f| (f["at"][0].as_f64().unwrap() - 18.0).abs() > 1.0), "{r}");
    // Keynote 4's exit sign: the luminaire whose schedule row says EXIT SIGN.
    let x = finding(&fs, "4", [31.0, 9.0], "exit_sign");
    assert_eq!(x["verdict"], "pass", "{x}");
    assert_eq!(x["measured"]["found"][0]["kind"], "wall_luminaire", "{x}");
    assert_eq!(r["checked"]["G-KEYNOTE"], json!({"pass": 5, "fail": 1, "missing": 1, "unsure": 0}), "{r}");
}

#[test]
fn keynote_conduit_of_another_size_or_under_the_ceiling_fails() {
    // 3/4" where the note says 1".
    let mut j = job(mep(0.75, 9.5, 12.0));
    let (_, fs) = keynote_findings(&mut j.s, "L1");
    let c = finding(&fs, "1", [10.0, 6.0], "conduit");
    assert_eq!(c["verdict"], "fail", "{c}");
    assert!(c["found"].as_str().unwrap().contains("3/4\""), "{c}");
    assert!(c["expected"].as_str().unwrap().contains("1\" conduit with pull tape"), "{c}");
    // 1", but at 8 ft: under the corridor's 9 ft hard lid.
    let mut j = job(mep(1.0, 8.0, 12.0));
    let (_, fs) = keynote_findings(&mut j.s, "L1");
    let c = finding(&fs, "1", [10.0, 6.0], "conduit");
    assert_eq!(c["verdict"], "fail", "{c}");
    assert!(c["issue"].as_str().unwrap().contains("under the 9 ft ceiling"), "{c}");
}

#[test]
fn a_riser_goes_up_to_the_next_floors_box_and_its_floor_is_listed() {
    let mut j = job(standard());
    let (_, fs) = keynote_findings(&mut j.s, "L1");
    let b = finding(&fs, "2", [5.0, 20.0], "bda_riser_box");
    assert_eq!(b["verdict"], "pass", "{b}");
    assert_eq!(b["handle"].as_str().unwrap().to_ascii_uppercase(), j.riser_box.to_ascii_uppercase());
    let r = finding(&fs, "2", [5.0, 20.0], "conduit");
    assert_eq!(r["verdict"], "pass", "{r}");
    assert_eq!(r["handle"], "R2");
    assert_eq!(r["measured"]["goes_to"]["level"], "L2", "{r}");
    assert_eq!(r["measured"]["goes_to"]["rise_ft"], 10.0, "{r}");
    let pen = r["measured"]["penetrations"].as_array().unwrap();
    assert_eq!(pen.len(), 1, "{r}");
    assert_eq!(pen[0]["assembly"], "floor");
    assert_eq!(pen[0]["level"], "L2");
    assert_eq!(pen[0]["needs_hr"], 2.0, "{r}");
    // Level 2's marker: the riser from level 1 comes up to it.
    let (_, fs2) = keynote_findings(&mut j.s, "L2");
    let up = finding(&fs2, "2", [5.0, 20.0], "conduit");
    assert_eq!(up["verdict"], "pass", "{up}");
    assert_eq!(up["measured"]["comes_from"]["level"], "L1", "{up}");
    // A riser that stops 6 ft up doesn't reach the box on L2.
    let mut j = job(mep(1.0, 9.5, 6.0));
    let (_, fs) = keynote_findings(&mut j.s, "L1");
    let short = finding(&fs, "2", [5.0, 20.0], "conduit");
    assert_eq!(short["verdict"], "fail", "{short}");
    assert!(short["issue"].as_str().unwrap().contains("stops 6 ft up"), "{short}");
    let (_, fs2) = keynote_findings(&mut j.s, "L2");
    assert_eq!(finding(&fs2, "2", [5.0, 20.0], "conduit")["verdict"], "missing");
}

#[test]
fn a_device_box_or_a_branch_circuit_by_a_bda_marker_is_not_the_bda_work() {
    // Room 101 holds only a device's 4x4 box and a branch circuit's 3/4" drop by the riser marker.
    let mut m = mep(1.0, 9.5, 12.0);
    m["systems"]
        .as_array_mut()
        .unwrap()
        .push(json!({"name": "BRANCH", "layer": "E-BRANCH", "kind": "conduit", "size_in": [0.75, 0], "bottom_ft": 9.5}));
    m["runs"][1]["system"] = json!("BRANCH");
    m["runs"][1]["size_in"] = json!([0.75, 0]);
    let mut j = job_with(m, ("junction_box", "4x4x2-1/8"));
    let (_, fs) = keynote_findings(&mut j.s, "L1");
    let b = finding(&fs, "2", [5.0, 20.0], "bda_riser_box");
    assert_eq!(b["verdict"], "missing", "{b}");
    assert_eq!(b["measured"]["other_boxes"][0]["size_in"], json!([4.0, 4.0, 2.125]), "{b}");
    let r = finding(&fs, "2", [5.0, 20.0], "conduit");
    assert_eq!(r["verdict"], "missing", "{r}");
    assert!(r["found"].as_str().unwrap().contains("no BDA riser conduit") && r["found"].as_str().unwrap().contains("only other systems"), "{r}");
    assert_eq!(r["measured"]["other_conduit"][0]["piece"]["system"], "BRANCH", "{r}");
}

#[test]
fn a_view_registered_on_another_levels_plan_places_no_marker() {
    // Level 2's sheet carries level 1's fit (as a sheet printing two levels' views can).
    let mut j = job(standard());
    let mut sp: Value = serde_json::from_str(j.s.doc().unwrap().building_spec.as_deref().unwrap()).unwrap();
    let e201 = sp["sheets"].as_array_mut().unwrap().iter_mut().find(|l| l["number"] == "E-201").unwrap();
    e201["registration"]["reference"] = json!("E-101");
    j.s.doc_mut().unwrap().building_spec = Some(sp.to_string());
    let (r, fs) = keynote_findings(&mut j.s, "L2");
    assert!(fs.is_empty(), "{r}");
    assert_eq!(
        r["unmeasured"]["G-KEYNOTE"]["keynote marker not placed: its registration places it on another level's plan (fitted on that level's view)"],
        1,
        "{r}"
    );
}

#[test]
fn a_device_nothing_names_is_missing_with_what_is_there_listed() {
    // Without the fixture schedule nothing says the luminaire XA is an exit sign.
    let mut j = job(standard());
    j.s.execute("bim.prints", &json!({"clear": true})).unwrap();
    let (_, fs) = keynote_findings(&mut j.s, "L1");
    let x = finding(&fs, "4", [31.0, 9.0], "exit_sign");
    assert_eq!(x["verdict"], "missing", "{x}");
    assert_eq!(x["measured"]["nearest_others"][0]["mark"], "XA", "{x}");
}

fn dir(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("cadaddy-vkeynote-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    d
}

fn read_json(p: &Path) -> Value {
    serde_json::from_str(&std::fs::read_to_string(p).unwrap_or_else(|e| panic!("{}: {e}", p.display()))).unwrap()
}

#[test]
fn a_packet_carries_the_keynote_list_and_its_markers_crops() {
    let mut j = job(standard());
    let out = dir("corridor");
    j.s.execute("validate.packet", &json!({"area": "L1/100", "out_dir": out.to_string_lossy()})).unwrap();
    let o = read_json(&out.join("objects.json"));
    let kn = &o["keynotes"];
    let list = kn["lists"].as_array().unwrap().iter().find(|l| l["sheet"] == "E-101").unwrap_or_else(|| panic!("{kn}"));
    let numbers: Vec<&str> = list["notes"].as_array().unwrap().iter().map(|n| n["number"].as_str().unwrap()).collect();
    assert_eq!(numbers, ["1", "2", "3", "4"], "{list}");
    assert!(list["notes"][1]["text"].as_str().unwrap().contains("RUN 2\" CONDUIT BETWEEN THE RISER BOXES OF EACH FLOOR"), "{list}");
    assert_eq!(list["notes"][0]["calls"]["boxes"][0]["size_in"], json!([8.0, 8.0, 6.0]), "{list}");
    // The corridor's markers (1, both 3s and 4), each with its crop; 2 is in room 101.
    let ms = kn["markers"].as_array().unwrap();
    let mut got: Vec<&str> = ms.iter().filter(|m| m["in_area"] == true).map(|m| m["number"].as_str().unwrap()).collect();
    got.sort_unstable();
    assert_eq!(got, ["1", "3", "3", "4"], "{kn}");
    let m = read_json(&out.join("manifest.json"));
    let crops: Vec<&Value> = m["prints"].as_array().unwrap().iter().collect();
    assert!(crops.iter().any(|c| c["kind"] == "keynotes" && c["sheet"] == "E-101"), "{m}");
    for mk in ms.iter().filter(|m| m["in_area"] == true) {
        let shows = mk["crop_shows"].as_str().unwrap();
        let c = crops.iter().find(|c| c["kind"] == "keynote_marker" && c["shows"] == shows).unwrap_or_else(|| panic!("no crop {shows}: {m}"));
        assert!(out.join(c["file"].as_str().unwrap()).exists(), "{c}");
    }
    // The prescreen in the packet holds G-KEYNOTE's findings; the prompt says where to look.
    let pre = o["prescreen"]["findings"].as_array().unwrap();
    assert!(pre.iter().any(|f| f["rule"] == "G-KEYNOTE" && f["verdict"] == "missing"), "{}", o["prescreen"]);
    let prompt = std::fs::read_to_string(out.join("prompt.md")).unwrap();
    assert!(prompt.contains("`keynotes`"), "{prompt}");
    let rules = read_json(&out.join("rules.json"));
    assert!(rules["rules"].as_array().unwrap().iter().any(|r| r["id"] == "G-KEYNOTE"), "{rules}");
    let _ = std::fs::remove_dir_all(&out);
}

#[test]
fn numbers_no_list_has_and_lists_with_no_notes_mark_nothing() {
    let mut s = Session::new();
    layer(&mut s, "A-WALL");
    rect(&mut s, 1, [0.0, 0.0], [40.0, 24.0]);
    rect(&mut s, 1, [0.5, 0.5], [39.5, 23.5]);
    layer(&mut s, "A-ROOM-TAG");
    text(&mut s, at([30.0, 6.0]), "100", 6.0);
    // A title with nothing under it, and a hexagon round a number of no list.
    layer(&mut s, "E-ANNO-TEXT");
    text(&mut s, at([100.0, 30.0]), "ELECTRICAL KEYNOTES:", 9.0);
    marker(&mut s, 1, [10.0, 6.0], "4");
    let mut sp = spec(json!({}));
    sp["rooms"] = json!([{"name": "Corridor", "number": "100", "level": "L1"}]);
    s.execute("bspec.set", &json!({"spec": sp, "replace": true})).unwrap();
    s.execute("model.build", &json!({})).unwrap();
    let (r, fs) = keynote_findings(&mut s, "L1");
    assert!(fs.is_empty(), "{r}");
    assert_eq!(r["checked"]["G-KEYNOTE"], json!({"pass": 0, "fail": 0, "missing": 0, "unsure": 0}), "{r}");
}
