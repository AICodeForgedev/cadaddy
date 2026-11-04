//! Q6178 dimreader: dimensions an enlarged sheet draws as plain line work and text (as a PDF
//! brings them in), read, placed on the model through the sheet's registration, attached to what
//! they measure, and documented on the devices. Made-up plans only.

use std::sync::{Mutex, MutexGuard};

use cadaddy_engine::Session;
use cadaddy_engine::doc::{Arc, Circle, Drawing, Entity, EntityKind, Handle, Line, Text};
use cadaddy_engine::geom::Vec3;
use serde_json::{Value, json};

static SERIAL: Mutex<()> = Mutex::new(());

fn serial() -> MutexGuard<'static, ()> {
    SERIAL.lock().unwrap_or_else(|e| e.into_inner())
}

fn push(d: &mut Drawing, layer: &str, kind: EntityKind) {
    let h = Handle(d.handseed.max(0x100));
    d.handseed = h.0 + 1;
    let mut e = Entity::new(h, kind);
    e.common.layer = layer.into();
    d.model.push(e);
}

/// A line in model inches.
fn line_in(d: &mut Drawing, layer: &str, a: (f64, f64), b: (f64, f64)) {
    push(d, layer, EntityKind::Line(Line { a: Vec3 { x: a.0, y: a.1, z: 0.0 }, b: Vec3 { x: b.0, y: b.1, z: 0.0 } }));
}

/// A wall line in feet.
fn wall(d: &mut Drawing, a: (f64, f64), b: (f64, f64)) {
    line_in(d, "A-WALL", (a.0 * 12.0, a.1 * 12.0), (b.0 * 12.0, b.1 * 12.0));
}

fn text_on(s: &mut Session, layer: &str, at: [f64; 2], t: &str) {
    if s.execute("layer.new", &json!({"name": layer, "current": true})).is_err() {
        s.execute("layer.current", &json!({"name": layer})).expect("layer");
    }
    s.execute("text", &json!({"at": at, "text": t, "height": 6})).expect("text");
}

/// Two rooms side by side, 6" walls: 1001 west 0.5–19.75', 1002 east 20.25–40', 0.5–9.5' deep;
/// a 3' door gap in 1002's north wall at 30'–33'.
fn two_rooms() -> Drawing {
    let mut d = Drawing::new_imperial();
    let outer = [((0., 0.), (40.5, 0.)), ((40.5, 0.), (40.5, 10.)), ((40.5, 10.), (33., 10.)), ((30., 10.), (0., 10.)), ((0., 10.), (0., 0.))];
    let west = [((0.5, 0.5), (19.75, 0.5)), ((19.75, 0.5), (19.75, 9.5)), ((19.75, 9.5), (0.5, 9.5)), ((0.5, 9.5), (0.5, 0.5))];
    let east = [
        ((20.25, 0.5), (40.0, 0.5)),
        ((40.0, 0.5), (40.0, 9.5)),
        ((40.0, 9.5), (33.0, 9.5)),
        ((30.0, 9.5), (20.25, 9.5)),
        ((20.25, 9.5), (20.25, 0.5)),
    ];
    for (a, b) in outer.into_iter().chain(west).chain(east) {
        wall(&mut d, a, b);
    }
    // The door's jambs close each wall's ends; its leaf, open, and its swing.
    wall(&mut d, (30.0, 9.5), (30.0, 10.0));
    wall(&mut d, (33.0, 9.5), (33.0, 10.0));
    line_in(&mut d, "A-DOOR", (396.0, 114.0), (396.0, 78.0));
    push(
        &mut d,
        "A-DOOR",
        EntityKind::Arc(Arc {
            center: Vec3 { x: 396.0, y: 114.0, z: 0.0 },
            radius: 36.0,
            start: std::f64::consts::PI,
            end: 1.5 * std::f64::consts::PI,
        }),
    );
    d
}

fn device(x: f64, y: f64, mark: &str, kind: &str, symbol: &str) -> Value {
    json!({
        "id": format!("device:L1:{x:.2},{y:.2}:{mark}"), "kind": kind, "level": "L1", "at_ft": [x, y],
        "fields": {
            "type": {"value": mark, "source": "plan"},
            "device_type": {"value": format!("{kind} {mark}"), "source": "plan"},
            "sheet": {"value": "E-901", "source": "plan"},
            "symbol": {"value": symbol, "source": "plan"},
        },
    })
}

/// The enlarged sheet's registration: turned a quarter, drawn at twice the plan's size, moved:
/// model = [a·x − b·y + tx, b·x + a·y + ty].
const A: f64 = 0.0;
const B: f64 = 0.5;
const TX: f64 = 400.0;
const TY: f64 = -1050.0;

/// Where a model point (inches) is drawn on the enlarged sheet.
fn on_sheet(p: (f64, f64)) -> (f64, f64) {
    let det = A * A + B * B;
    let (qx, qy) = (p.0 - TX, p.1 - TY);
    ((A * qx + B * qy) / det, (-B * qx + A * qy) / det)
}

/// The enlarged sheet's rectangle (model space, away from the plan), and the second sheet's.
const SHEET: [f64; 4] = [2000.0, -200.0, 2400.0, 900.0];
const SHEET2_SHIFT: (f64, f64) = (600.0, 37.0);

fn spec(tags: Vec<Value>, second_sheet: bool) -> Value {
    let mut sheets = vec![
        json!({"number": "A-900", "level": "L1", "discipline": "A", "rect": [-24, -24, 600, 200],
               "registration": {"reference": "A-900", "method": "reference", "a": 1, "b": 0, "tx": 0, "ty": 0}}),
        json!({"number": "E-950", "level": "L1", "discipline": "E", "rect": SHEET,
               "registration": {"reference": "A-900", "method": "tags", "a": A, "b": B, "tx": TX, "ty": TY, "matched": 12, "residual": 1.0}}),
    ];
    if second_sheet {
        let r = [SHEET[0] + SHEET2_SHIFT.0, SHEET[1] + SHEET2_SHIFT.1, SHEET[2] + SHEET2_SHIFT.0, SHEET[3] + SHEET2_SHIFT.1];
        // Its level is not said: the borrowed registration gives it.
        sheets.push(json!({"number": "E-951", "discipline": "E", "rect": r}));
    }
    json!({
        "version": 1, "units_per_foot": 12,
        "levels": [{"name": "L1", "elevation_ft": 0, "height_ft": 10, "ceiling_ft": 9, "base": [0, 0]}],
        "layers": [{"layer": "A-WALL", "role": "wall", "wall_type": "Partition"}, {"layer": "A-DOOR", "role": "door"}, {"layer": "A-ROOM-TAG", "role": "room_tag"}],
        "wall_types": [{"name": "Partition", "thickness_in": 6}],
        "openings": {"enabled": true, "min_ft": 1.5, "max_ft": 16.0, "door_head_ft": 7.0, "window_head_ft": 7.0, "window_sill_ft": 2.0},
        "sheets": sheets,
        "tags": tags,
    })
}

/// What the enlarged sheet draws, in model inches (drawn on the sheet through [`on_sheet`],
/// then moved by `shift` on the sheet): dimension lines with ticks at faces and edges, dots at
/// devices, dashed extension lines, values as text (two merged in one text), and the room's
/// walls for its background.
struct Sheet {
    shift: (f64, f64),
    ents: Vec<EntityKind>,
}

impl Sheet {
    fn p(&self, m: (f64, f64)) -> Vec3 {
        let q = on_sheet(m);
        Vec3 { x: q.0 + self.shift.0, y: q.1 + self.shift.1, z: 0.0 }
    }
    fn line(&mut self, a: (f64, f64), b: (f64, f64)) {
        let (a, b) = (self.p(a), self.p(b));
        self.ents.push(EntityKind::Line(Line { a, b }));
    }
    /// A tick (a 45° slash, 1" each way in model terms) at a witness.
    fn tick(&mut self, at: (f64, f64)) {
        self.line((at.0 - 1.0, at.1 - 1.0), (at.0 + 1.0, at.1 + 1.0));
    }
    fn dot(&mut self, at: (f64, f64)) {
        let c = self.p(at);
        self.ents.push(EntityKind::Circle(Circle { center: c, radius: 0.7 }));
    }
    /// A device's symbol (a 6" circle) where the sheet shows it.
    fn symbol(&mut self, at: (f64, f64)) {
        let c = self.p(at);
        self.ents.push(EntityKind::Circle(Circle { center: c, radius: 6.0 }));
    }
    /// An extension line from the dimension line (`y_line`) to `to` (model y), dashed.
    fn ext(&mut self, x: f64, y_line: f64, to: f64) {
        let s = if to > y_line { 1.0 } else { -1.0 };
        let mut y = y_line - 2.0 * s;
        while (to - y) * s > 0.0 {
            let y1 = if ((y + 5.0 * s) - to) * s > 0.0 { to } else { y + 5.0 * s };
            self.line((x, y), (x, y1));
            y = y1 + 1.0 * s;
        }
    }
    /// A value reading along +x model, its baseline 1" above the dimension line, starting at x.
    fn value(&mut self, text: &str, x: f64, y_line: f64) {
        let at = self.p((x, y_line + 1.0));
        // Model +x is sheet -y here: the text reads down the sheet.
        self.ents.push(EntityKind::Text(Text {
            insert: at,
            align_pt: None,
            height: 6.0,
            value: text.into(),
            rotation: -std::f64::consts::FRAC_PI_2,
            width_factor: 1.0,
            oblique: 0.0,
            style: "Standard".into(),
            halign: Default::default(),
            valign: Default::default(),
        }));
    }
}

/// The enlarged sheet's dimensions (see the tests for what each measures).
fn draw_sheet(shift: (f64, f64)) -> Vec<EntityKind> {
    let mut sh = Sheet { shift, ents: Vec::new() };
    // The background: the rooms' walls, and casework lines of every length (what makes the
    // plan the same plan on another sheet).
    for (a, b) in [
        ((0.0, 0.0), (486.0, 0.0)),
        ((486.0, 0.0), (486.0, 120.0)),
        ((0.0, 120.0), (360.0, 120.0)),
        ((396.0, 120.0), (486.0, 120.0)),
        ((0.0, 0.0), (0.0, 120.0)),
        ((6.0, 6.0), (237.0, 6.0)),
        ((237.0, 6.0), (237.0, 114.0)),
        ((237.0, 114.0), (6.0, 114.0)),
        ((6.0, 114.0), (6.0, 6.0)),
        ((243.0, 6.0), (480.0, 6.0)),
        ((480.0, 6.0), (480.0, 114.0)),
        ((243.0, 114.0), (243.0, 6.0)),
    ] {
        sh.line(a, b);
    }
    // The devices' symbols, where this sheet shows them (X 10" off the model's; Z not shown).
    for at in [(222.0, 7.2), (186.0, 7.2), (162.0, 7.2), (414.0, 112.8), (267.0, 7.2), (255.0, 7.2), (48.0, 112.8)] {
        sh.symbol(at);
    }
    for k in 0..24 {
        let y = 40.0 + f64::from(k) * 2.5;
        sh.line((20.0 + f64::from(k), y), (60.0 + f64::from(k) * 3.0, y));
    }
    // 1. A chain of three along 1001's south wall (its face at y = 6"): from the east face
    // (x = 237") to devices A (222"), B (186") and C (162"): 2'-0", 3'-0", 1'-3".
    let y = 30.0;
    sh.line((158.0, y), (241.0, y));
    sh.tick((237.0, y));
    sh.ext(237.0, y, 8.0);
    for x in [222.0, 186.0, 162.0] {
        sh.dot((x, y));
        sh.ext(x, y, 9.0);
    }
    // Two values merged into one text, as a PDF often brings them in.
    sh.value("2'-0\" 3'-0\"", 166.0, y);
    sh.value("1'-3\"", 226.0, y);
    // 2. 1002's north face (y = 114"), its door gap 360"–396": from the door's east edge to
    // device D (414"): 1'-6".
    let y = 90.0;
    sh.line((392.0, y), (418.0, y));
    sh.tick((396.0, y));
    sh.ext(396.0, y, 113.0);
    sh.dot((414.0, y));
    sh.ext(414.0, y, 111.0);
    sh.value("1'-6\"", 399.0, y);
    // 3. 1002's south face: from its west face (x = 243") to device X: 2'-0" on the sheet; the
    // model has X 10" further (a conflict).
    let y = 40.0;
    sh.line((239.0, y), (271.0, y));
    sh.tick((243.0, y));
    sh.ext(243.0, y, 8.0);
    sh.dot((267.0, y));
    sh.ext(267.0, y, 9.0);
    sh.value("2'-0\"", 248.0, y);
    // 4. 1001's width, face to face: 19'-3"; and from its west face to where device Z is in the
    // model, which this sheet doesn't draw (a room's dimension, not Z's): 7'-10". One baseline.
    let y = 60.0;
    sh.line((2.0, y), (241.0, y));
    sh.tick((6.0, y));
    sh.tick((237.0, y));
    sh.value("19'-3\"", 110.0, y);
    let y = 70.0;
    sh.line((2.0, y), (104.0, y));
    sh.tick((6.0, y));
    sh.tick((100.0, y));
    sh.ext(100.0, y, 9.0);
    sh.value("7'-10\"", 40.0, y);
    // 5. Across the demising wall: from 1001's east face (x = 237") to device Y in 1002 (255"):
    // 1'-6", which is 1'-0" from 1002's own west face.
    let y = 50.0;
    sh.line((233.0, y), (259.0, y));
    sh.tick((237.0, y));
    sh.dot((255.0, y));
    sh.ext(255.0, y, 9.0);
    sh.value("1'-6\"", 240.0, y);
    // 6. 1001's north wall: from the west edge of the upper cabinet there (30"–60", its front
    // at y = 102") to device W (48"): 1'-6".
    let y = 90.0;
    sh.line((26.0, y), (52.0, y));
    sh.tick((30.0, y));
    sh.ext(30.0, y, 101.0);
    sh.dot((48.0, y));
    sh.ext(48.0, y, 111.0);
    sh.value("1'-6\"", 33.0, y);
    sh.ents
}

fn job(second_sheet: bool) -> Session {
    let mut s = Session::empty();
    let mut d = two_rooms();
    for k in draw_sheet((0.0, 0.0)) {
        push(&mut d, "0", k);
    }
    if second_sheet {
        for k in draw_sheet(SHEET2_SHIFT) {
            push(&mut d, "0", k);
        }
    }
    s.open_drawing(d, "Dimensioned Rooms", None);
    text_on(&mut s, "A-ROOM-TAG", [110.0, 64.0], "OFFICE");
    text_on(&mut s, "A-ROOM-TAG", [116.0, 52.0], "1001");
    text_on(&mut s, "A-ROOM-TAG", [350.0, 64.0], "STORAGE");
    text_on(&mut s, "A-ROOM-TAG", [356.0, 52.0], "1002");
    let tags = vec![
        device(18.5, 0.6, "A", "receptacle", "1F0"),
        device(15.5, 0.6, "B", "receptacle", "1F1"),
        device(13.5, 0.6, "C", "receptacle", "1F2"),
        device(34.5, 9.4, "D", "receptacle", "1F3"),
        // Drawn at 22.25' (2'-0" from the west face); the model has it at 23.083'.
        device(20.25 + 34.0 / 12.0, 0.6, "X", "receptacle", "1F4"),
        device(255.0 / 12.0, 0.6, "Y", "receptacle", "1F5"),
        device(100.0 / 12.0, 0.6, "Z", "receptacle", "1F6"),
        device(4.0, 9.4, "W", "receptacle", "1F7"),
    ];
    s.execute("bspec.set", &json!({"replace": true, "spec": spec(tags, second_sheet)})).expect("spec");
    s.execute("model.build", &json!({})).expect("build");
    s.execute("rooms.build", &json!({})).expect("rooms");
    s.execute("bim.props.fill", &json!({})).expect("fill");
    // An upper cabinet on 1001's north wall, 30"–60" (made-up casework).
    s.execute("bim.layout.object", &json!({"kind": "upper_cabinet", "tag": "CAB-1", "level": "L1", "box": [30.0, 102.0, 54.0, 60.0, 114.0, 84.0], "source": "made-up elevation"}))
        .expect("cabinet");
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

#[test]
fn a_chain_on_a_turned_enlarged_sheet_documents_each_devices_offset() {
    let _g = serial();
    let mut s = job(false);
    let r = s.execute("bim.layout.dims", &json!({"limit": 20})).expect("dims");
    let e950 = &r["by_sheet"]["E-950"];
    // Read: the chain's three, the door's, the conflict's, the room's two, the demising wall's; all placed through the sheet's registration (turned 90°, half size,
    // moved).
    assert_eq!((e950["read"].as_u64(), e950["placed"].as_u64()), (Some(9), Some(9)), "{r:#}");
    assert_eq!(e950["registration"], json!("tags onto A-900"), "{e950}");
    // Baselines: dimensions sharing a witness line on another dimension line — the room's and Z's
    // from the west face; the chain's, the room's and the demising wall's at the east face.
    assert_eq!((e950["chains"].as_u64(), e950["baseline_groups"].as_u64()), (Some(1), Some(2)), "{e950}");
    // Z's: its far end is on nothing (no symbol there, so not on Z).
    assert_eq!((e950["attached"]["both_ends"].as_u64(), e950["attached"]["one_end"].as_u64()), (Some(8), Some(1)), "{e950}");
    // Devices in the chain end the stretch on each side: C, B twice, A twice; then D, X, Y, W.
    assert_eq!(e950["ends"]["device centre"], json!(9), "{e950}");
    assert_eq!(e950["ends"]["casework edge"], json!(1), "{e950}");
    // The chain's east face, the conflict's west face, the room's two, Z's west face, the
    // demising wall's.
    assert_eq!(e950["ends"]["wall face"], json!(6), "{e950}");
    assert_eq!(e950["ends"]["opening edge"], json!(1), "{e950}");
    // The witness points land on the model where the devices are.
    let dims = r["dimensions_first"].as_array().expect("listed");
    let chain_a = dims.iter().find(|d| d["value"] == json!("1'-3\"")).expect("1'-3\"");
    let xs: Vec<f64> = chain_a["model_ft"].as_array().unwrap().iter().map(|p| p[0].as_f64().unwrap()).collect();
    assert!(xs.iter().any(|x| (x - 18.5).abs() < 0.01) && xs.iter().any(|x| (x - 19.75).abs() < 0.01), "{chain_a}");
    // Rehost documents the offsets: each from the chain's fixed end, the east face.
    let rh = s.execute("bim.layout.rehost", &json!({})).expect("rehost");
    assert_eq!(rh["drawn_dimensions"]["devices_documented"], json!(7), "{:#}", rh["drawn_dimensions"]);
    for (mark, inches) in [("A", 15.0), ("B", 51.0), ("C", 75.0)] {
        let p = props(&mut s, mark);
        assert_eq!(p["offset_from"]["value"], json!("1001/E"), "{mark}: {p:#}");
        assert_eq!(p["offset_along"]["value"].as_f64(), Some(inches), "{mark}: {p:#}");
        assert_eq!(p["offset_along"]["source"]["kind"], json!("print"), "{mark}");
        assert_eq!(p["offset_along"]["source"]["doc"], json!("E-950"), "{mark}");
        assert!(p["offset_along"]["source"]["at"].is_array(), "the witness's sheet point: {mark}");
        assert!(p["offset_along"]["note"].as_str().is_some_and(|n| n.contains("documented") && n.contains("E-950")), "{mark}: {p:#}");
        assert!(p.get("offset_conflict").is_none_or(Value::is_null), "{mark} agrees with the model");
    }
    // Y: dimensioned from the demising wall's far face; documented from its own room's face, the
    // wall's thickness off the drawn value.
    let y = props(&mut s, "Y");
    assert_eq!((y["offset_from"]["value"].as_str(), y["offset_along"]["value"].as_f64()), (Some("1002/W"), Some(12.0)), "{y:#}");
    assert_eq!(y["offset_along"]["source"]["doc"], json!("E-950"));
    // W: from the cabinet's edge.
    let w = props(&mut s, "W");
    assert!(w["offset_from"]["value"].as_str().is_some_and(|t| t.contains("CAB-1")), "{w:#}");
    assert_eq!(w["offset_along"]["value"].as_f64(), Some(18.0), "{w:#}");
    // Z: the sheet draws no device there: its offset stays measured from the plan.
    let z = props(&mut s, "Z");
    assert_eq!(z["offset_along"]["source"]["kind"], json!("computed"), "{z:#}");
    assert!(z["offset_along"]["note"].as_str().is_some_and(|n| n.contains("derived: measured from the plan")), "{z:#}");
}

#[test]
fn a_dimension_to_a_door_edge_is_the_devices_reference() {
    let _g = serial();
    let mut s = job(false);
    s.execute("bim.layout.rehost", &json!({})).expect("rehost");
    let p = props(&mut s, "D");
    let from = p["offset_from"]["value"].as_str().unwrap_or_default().to_string();
    assert!(from.contains("edge") && from.contains("1002/N"), "{p:#}");
    assert_eq!(p["offset_along"]["value"].as_f64(), Some(18.0), "{p:#}");
    assert_eq!(p["offset_along"]["source"]["doc"], json!("E-950"));
}

#[test]
fn a_drawn_value_the_model_disagrees_with_is_a_conflict_for_review() {
    let _g = serial();
    let mut s = job(false);
    let rh = s.execute("bim.layout.rehost", &json!({})).expect("rehost");
    let dd = &rh["drawn_dimensions"];
    assert_eq!(dd["conflicts"], json!(1), "{dd:#}");
    let c = &dd["conflicts_first"][0];
    assert_eq!((c["drawn_in"].as_f64(), c["model_in"].as_f64(), c["band_in"].as_f64()), (Some(24.0), Some(34.0), Some(6.0)), "{c}");
    assert_eq!(c["from"], json!("1002/W"));
    let p = props(&mut s, "X");
    assert!(p["offset_conflict"]["value"].as_str().is_some_and(|t| t.contains("2 ft") && t.contains("2 ft 10 in")), "{p:#}");
    // It is in the review queue, with both values and their sources.
    let q = s.execute("review.list", &json!({"kind": "conflict"})).expect("review");
    let items = q["items"].as_array().expect("items");
    assert_eq!(items.len(), 1, "{q:#}");
    let next = s.execute("review.next", &json!({"kind": "conflict"})).expect("next");
    let vals = next["item"]["detail"]["values"].as_array().expect("values");
    assert_eq!(next["item"]["detail"]["key"], json!("offset_along"));
    assert_eq!(vals[0]["value"].as_f64(), Some(24.0));
    assert_eq!(vals[0]["source"]["doc"], json!("E-950"));
    assert_eq!(vals[1]["value"].as_f64(), Some(34.0));
    // The dims report says so too; the others pass.
    let r = s.execute("bim.layout.dims", &json!({})).expect("dims");
    assert_eq!(r["conflicts"], json!(1), "{r:#}");
    // Once the model agrees (the device moved to the drawn spot), the conflict closes.
    let h = handle_of(&mut s, "X");
    s.execute("move", &json!({"handles": [h], "from": [0, 0], "to": [-10, 0]})).expect("move");
    let rh = s.execute("bim.layout.rehost", &json!({})).expect("rehost again");
    assert_eq!(rh["drawn_dimensions"]["conflicts"], json!(0), "{:#}", rh["drawn_dimensions"]);
    let q = s.execute("review.list", &json!({"kind": "conflict"})).expect("review");
    assert_eq!(q["items"].as_array().map(Vec::len), Some(0), "{q:#}");
}

#[test]
fn a_sheet_with_no_registration_borrows_the_one_of_the_sheet_drawing_its_plan() {
    let _g = serial();
    let mut s = job(true);
    let r = s.execute("bim.layout.dims", &json!({})).expect("dims");
    let e951 = &r["by_sheet"]["E-951"];
    assert!(e951["registration"].as_str().is_some_and(|t| t.starts_with("lines") && t.contains("E-950")), "{e951:#}");
    assert_eq!((e951["read"].as_u64(), e951["placed"].as_u64()), (Some(9), Some(9)), "{e951:#}");
    assert_eq!(e951["attached"]["both_ends"], json!(8), "{e951:#}");
}

#[test]
fn an_unregistered_sheet_reports_why_and_hostile_params_are_refused() {
    let _g = serial();
    let mut s = job(false);
    // Take the enlarged sheet's registration away: read, not placed, said why.
    let mut sp = s.execute("bspec.get", &json!({})).expect("spec")["spec"].clone();
    if let Some(list) = sp["sheets"].as_array_mut() {
        for l in list.iter_mut() {
            if l["number"] == json!("E-950") {
                l["registration"] = Value::Null;
            }
        }
    }
    s.execute("bspec.set", &json!({"replace": true, "spec": sp})).expect("spec");
    let r = s.execute("bim.layout.dims", &json!({})).expect("dims");
    let e950 = &r["by_sheet"]["E-950"];
    assert_eq!((e950["read"].as_u64(), e950["placed"].as_u64()), (Some(9), Some(0)), "{e950:#}");
    assert_eq!(e950["unplaced"]["its sheet is not registered on a level's plan"], json!(9), "{e950:#}");
    for p in [json!({"limit": -3}), json!({"sheet": 7}), json!({"north": "x"}), json!({"limit": 1e300, "sheet": "NOPE"})] {
        let _ = s.execute("bim.layout.dims", &p);
    }
}
