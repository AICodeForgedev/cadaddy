//! The prescreen follows the document authority the job's notes state (Q6563), on a made-up job:
//! one guest room of a unit type. The power plan's general note sends its devices' locations and
//! mounting heights to the architecture's unit plan, which draws the room's walls, its device
//! symbols and an AFF note on each with a leader. The switch's `48" AFF` is lettered nearer the
//! receptacle beside it than the switch; its leader ends on the switch.
//!
//! The model: the switch where the power plan (reference only) drew it, 5 ft off the unit plan's
//! spot; a receptacle at the unit plan's spot but at 48" with a height record that took the
//! switch's note (the wrong pairing); a receptacle right in both. The unit plan also shows a
//! noted device no element stands for.

use std::sync::Arc;

use cadaddy_engine::Session;
use cadaddy_engine::doc::{Attrib, AttribModes, AttribProps, Block, Entity, EntityKind, Insert, Line, Text};
use cadaddy_engine::geom::Vec3;
use serde_json::{Value, json};

/// The plan's origin on the drawing (inches): the level's base.
const OX: f64 = 1200.0;
const OY: f64 = 1000.0;
/// One sheet: 36" × 24", in drawing inches.
const SW: f64 = 3456.0;
const SH: f64 = 2304.0;
/// The power plan's sheet, registered onto the floor plan; the unit plans' sheet.
const E1: f64 = 8000.0;
const A8: f64 = 16000.0;
/// The unit plan view's middle on its sheet: the room's middle.
const VC: [f64; 2] = [A8 + 1500.0, 1000.0];
const MID: [f64; 2] = [10.5, 7.0];
/// The unit plan's spots (feet, as the room lies): the switch, the receptacle beside it, the
/// north receptacle, and an east wall device no element stands for.
const S_AT: [f64; 2] = [2.5, 0.7];
const R_AT: [f64; 2] = [4.5, 0.7];
const N_AT: [f64; 2] = [10.0, 13.3];
const X_AT: [f64; 2] = [20.3, 7.0];
/// Where the power plan drew the switch.
const S_DRAWN: [f64; 2] = [7.5, 0.7];

fn layer(s: &mut Session, name: &str) {
    if s.execute("layer.new", &json!({"name": name, "current": true})).is_err() {
        s.execute("layer.current", &json!({"name": name})).unwrap();
    }
}

/// A building point (feet) on the plan.
fn at(p: [f64; 2]) -> [f64; 2] {
    [OX + p[0] * 12.0, OY + p[1] * 12.0]
}

/// A building point (feet) on the unit plan view.
fn on_view(p: [f64; 2]) -> [f64; 2] {
    [VC[0] + (p[0] - MID[0]) * 12.0, VC[1] + (p[1] - MID[1]) * 12.0]
}

fn seg(s: &mut Session, a: [f64; 2], b: [f64; 2]) {
    s.execute("line", &json!({"points": [a, b]})).unwrap();
}

fn rect(s: &mut Session, a: [f64; 2], b: [f64; 2]) {
    let v = [at(a), at([b[0], a[1]]), at(b), at([a[0], b[1]])];
    s.execute("pline", &json!({"vertices": v, "closed": true})).unwrap();
}

fn text(s: &mut Session, p: [f64; 2], t: &str, h: f64) {
    s.execute("text", &json!({"at": p, "text": t, "height": h})).unwrap();
}

/// Block references at building points (feet), on `layer` with a TYPE attribute.
fn place(s: Session, block: &str, layer: &str, pts: &[[f64; 2]], ty: &str) -> (Session, Vec<String>) {
    let mut d = (*s.doc().unwrap()).clone();
    let style = d.model.iter().find_map(|e| if let EntityKind::Text(t) = &e.kind { Some(t.clone()) } else { None }).unwrap();
    if !d.blocks.contains_key(block) {
        let mut b = Block::new(block);
        let h = d.new_handle();
        b.entities.push(Entity::new(h, EntityKind::Line(Line { a: Vec3::new(-4.0, 0.0, 0.0), b: Vec3::new(4.0, 0.0, 0.0) })));
        d.blocks.insert(block.into(), Arc::new(b));
    }
    let mut hs = Vec::new();
    for p in pts {
        let attribs = vec![Attrib {
            tag: "TYPE".into(),
            text: Text { value: ty.into(), ..style.clone() },
            invisible: true,
            constant: false,
            prompt: String::new(),
            props: AttribProps::default(),
            multiline: None,
            modes: AttribModes::default(),
        }];
        let q = at(*p);
        let ins = Insert {
            block: block.into(),
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
        e.common.layer = layer.into();
        d.model.push(e);
        hs.push(h.hex());
    }
    let mut s2 = Session::empty();
    s2.open_drawing(d, "Sample Job", None);
    (s2, hs)
}

fn spec() -> Value {
    json!({
        "units_per_foot": 12,
        "levels": [{"name": "L1", "elevation_ft": 0, "height_ft": 10, "ceiling_ft": 9, "base": [OX, OY], "region": [0, 0, SW, SH]}],
        "layers": [
            {"layer": "A-WALL-CL", "role": "wall", "wall_type": "Partition"},
            {"layer": "A-WALL", "role": "wall", "wall_type": "Exterior"},
            {"layer": "E-POWR", "role": "device"},
            {"layer": "A-ROOM-TAG", "role": "room_tag"}
        ],
        "wall_types": [
            {"name": "Exterior", "thickness_in": 6},
            {"name": "Partition", "thickness_in": 4.875, "from": "centerline"}
        ],
        "rooms": [{"name": "Guest Room", "number": "101"}],
        "devices": {"by_type": {
            "REC": {"ceiling": false, "aff_ft": 1.5},
            "RECX": {"ceiling": false, "aff_ft": 4.0},
            "SW": {"ceiling": false, "aff_ft": 4.0}
        }},
        "sheets": [
            {"number": "A-101", "title": "FLOOR PLAN - LEVEL 1", "discipline": "A", "level": "L1", "source": "model", "rect": [0.0, 0.0, SW, SH],
             "registration": {"reference": "A-101", "method": "reference", "a": 1.0, "b": 0.0, "tx": 0.0, "ty": 0.0, "matched": 4}},
            {"number": "E-101", "title": "POWER PLAN - LEVEL 1", "discipline": "E", "level": "L1", "source": "model", "rect": [E1, 0.0, E1 + SW, SH],
             "registration": {"reference": "A-101", "method": "tags", "a": 1.0, "b": 0.0, "tx": -E1, "ty": 0.0, "matched": 4}},
            {"number": "A-801", "title": "ENLARGED UNIT PLANS", "discipline": "A", "source": "model", "rect": [A8, 0.0, A8 + SW, SH]}
        ]
    })
}

struct Job {
    s: Session,
    switch: String,
    /// The receptacle at the unit plan's spot whose height took the switch's note.
    wrong_note: String,
    north: String,
}

/// A device symbol on the unit plan with its height note at `note` (feet, as the room lies) and a
/// leader from the note to the symbol.
fn noted_symbol(s: &mut Session, sym: [f64; 2], note: [f64; 2], words: &str) {
    let c = on_view(sym);
    s.execute("circle", &json!({"center": c, "radius": 3})).unwrap();
    let n = on_view(note);
    text(s, n, words, 4.0);
    let up = if note[1] > sym[1] { 3.0 } else { -3.0 };
    seg(s, n, [c[0], c[1] + up]);
}

fn job() -> Job {
    let mut s = Session::new();
    // The room: a 21' × 14' shell of 6" walls, a bath fin off its north wall.
    layer(&mut s, "A-WALL");
    rect(&mut s, [0.0, 0.0], [21.0, 14.0]);
    rect(&mut s, [0.5, 0.5], [20.5, 13.5]);
    layer(&mut s, "A-WALL-CL");
    s.execute("line", &json!({"points": [at([13.0, 13.5]), at([13.0, 9.0])]})).unwrap();
    layer(&mut s, "A-ROOM-TAG");
    text(&mut s, at([10.0, 9.0]), "101", 6.0);
    layer(&mut s, "G-FRAME");
    for x in [0.0, E1, A8] {
        s.execute("pline", &json!({"vertices": [[x, 0.0], [x + SW, 0.0], [x + SW, SH], [x, SH]], "closed": true})).unwrap();
    }
    // The power plan's general note: its spots and heights are the architecture's.
    layer(&mut s, "E-ANNO");
    text(&mut s, [E1 + 200.0, 300.0], "1. SEE ARCHITECTURAL UNIT PLANS FOR DEVICE LOCATIONS AND MOUNTING HEIGHTS.", 6.0);
    // The unit plan: the room's walls (both faces) and fin, its symbols and notes.
    layer(&mut s, "A-ENLG");
    let walls: [([f64; 2], [f64; 2]); 10] = [
        ([0.0, 0.0], [21.0, 0.0]),
        ([0.5, 0.5], [20.5, 0.5]),
        ([0.5, 13.5], [20.5, 13.5]),
        ([0.0, 14.0], [21.0, 14.0]),
        ([0.0, 0.0], [0.0, 14.0]),
        ([0.5, 0.5], [0.5, 13.5]),
        ([20.5, 0.5], [20.5, 13.5]),
        ([21.0, 0.0], [21.0, 14.0]),
        ([12.797, 9.0], [12.797, 13.5]),
        ([13.203, 9.0], [13.203, 13.5]),
    ];
    for (a, b) in walls {
        seg(&mut s, on_view(a), on_view(b));
    }
    // The switch's note lettered nearer the receptacle than the switch: its leader says whose.
    noted_symbol(&mut s, S_AT, [4.4, 2.2], "48\" AFF");
    noted_symbol(&mut s, R_AT, [6.5, 3.4], "18\" AFF");
    noted_symbol(&mut s, N_AT, [10.3, 11.6], "18\" AFF");
    noted_symbol(&mut s, X_AT, [17.5, 8.4], "48\" AFF");
    // The model.
    let (s, rec) = place(s, "RECEP", "E-POWR", &[N_AT], "REC");
    let (s, recx) = place(s, "RECEPX", "E-POWR", &[R_AT], "RECX");
    let (mut s, sw) = place(s, "SWITCH", "E-POWR", &[S_DRAWN], "SW");
    s.execute("bspec.set", &json!({"spec": spec(), "replace": true})).unwrap();
    s.execute("model.build", &json!({})).unwrap();
    for (hs, kind) in [(&rec, "receptacle"), (&recx, "receptacle"), (&sw, "switch")] {
        for h in hs.iter() {
            s.execute("bim.source", &json!({"handles": [h], "status": "fixed", "source": "E-101", "kind": kind})).unwrap();
        }
    }
    // The job's height record of the receptacle beside the switch: the switch's note.
    {
        use cadaddy_engine::doc::props::{self, Prop, Source, SourceKind};
        let h = cadaddy_engine::doc::Handle::parse_hex(&recx[0]).unwrap();
        let d = s.doc_mut().unwrap();
        assert!(d.model.modify(h, |e| {
            let mut store = props::read(&e.common.xdata);
            store.set("mount_height", Prop::new(48.0, Source::from_doc(SourceKind::Note, "48\" AFF on A-801")).with_unit("in"));
            e.common.xdata = props::write(&e.common.xdata, &store).into();
        }));
    }
    // The unit type, its plan on the architecture's sheet; room 101 its instance.
    let mut sp: Value = serde_json::from_str(s.doc().unwrap().building_spec.as_deref().unwrap()).unwrap();
    let view = [VC[0] - 11.5 * 12.0, VC[1] - 7.5 * 12.0, VC[0] + 11.5 * 12.0, VC[1] + 7.5 * 12.0];
    sp["unit_types"] = json!([{"id": "UA", "name": "UNIT A",
        "views": [{"sheet": "A-801", "title": "UNIT A - ENLARGED PLAN", "rect": view, "sheet_rect": [A8, 0.0, A8 + SW, SH]}],
        "source": {"sheet": "A-801", "by": "made-up test"}}]);
    sp["unit_instances"] = json!([{"number": "101", "level": "L1", "type": "UA", "hand": "normal"}]);
    s.doc_mut().unwrap().building_spec = Some(sp.to_string()).into();
    // What the job read on the power plan (its spots reference only).
    let on_power = |p: [f64; 2]| {
        let q = at(p);
        [q[0] + E1, q[1]]
    };
    let devices = json!([
        {"tag": "REC", "kind": "receptacle", "at": on_power(N_AT), "sheet": "E-101", "level": "L1"},
        {"tag": "REC", "kind": "receptacle", "at": on_power(R_AT), "sheet": "E-101", "level": "L1"},
        {"tag": "SW", "kind": "switch", "at": on_power(S_DRAWN), "sheet": "E-101", "level": "L1"}
    ]);
    let state = json!({"guided_job": {
        "id": "made-up-job", "goal": "bim", "created": "2026-01-01T00:00:00Z", "pdfs_path": "/made-up/prints",
        "stages": [], "rules": [], "audit": [], "question_seq": 0, "devices": devices
    }});
    s.doc_mut().unwrap().agent = Some(state.to_string()).into();
    Job { s, switch: sw[0].clone(), wrong_note: recx[0].clone(), north: rec[0].clone() }
}

fn screen(s: &mut Session, rules: &[&str]) -> (Value, Vec<Value>) {
    let mut out = Vec::new();
    let mut cursor = 0;
    loop {
        let r = s.execute("validate.prescreen", &json!({"level": "L1", "all": true, "limit": 50, "cursor": cursor, "rules": rules})).unwrap();
        out.extend(r["findings"].as_array().cloned().unwrap_or_default());
        match r["next"]["cursor"].as_u64().or_else(|| r["next"].as_u64()) {
            Some(n) if n as usize > cursor => cursor = n as usize,
            _ => return (r, out),
        }
    }
}

fn of<'a>(fs: &'a [Value], rule: &str, handle: &str) -> Vec<&'a Value> {
    fs.iter().filter(|f| f["rule"] == rule && f["handle"].as_str().is_some_and(|h| h.eq_ignore_ascii_case(handle))).collect()
}

#[test]
fn heights_come_from_the_governing_unit_plans_note_its_leader_ties() {
    let mut j = job();
    let (r, fs) = screen(&mut j.s, &["G-HEIGHT"]);
    // The receptacle the job gave the switch's note: the unit plan says 18", not its record's 48".
    let rx = of(&fs, "G-HEIGHT", &j.wrong_note);
    let rx = rx.first().unwrap_or_else(|| panic!("no G-HEIGHT for the receptacle: {r:#}"));
    assert_eq!((rx["verdict"].clone(), rx["measured"]["expected_in"].clone()), (json!("fail"), json!(18.0)), "{rx:#}");
    let gov = &r["governing"];
    assert_eq!(gov["deferring_sheets"]["E-101"]["sends"], "locations and heights", "{r:#}");
    assert_eq!(gov["rooms_laid"], 1, "the unit plan laid on room 101 by its walls: {gov:#}");
    assert_eq!(rx["measured"]["expected_source"]["sheet"], "A-801", "{rx:#}");
    assert_eq!(rx["measured"]["authority"]["level"], "governing", "{rx:#}");
    assert!(rx["measured"]["expected_from"].as_str().unwrap().contains("leader"), "{rx:#}");
    // The switch takes its own note (its leader), though it is lettered nearer the receptacle.
    let sw = of(&fs, "G-HEIGHT", &j.switch);
    let sw = sw.first().unwrap_or_else(|| panic!("no G-HEIGHT for the switch: {r:#}"));
    assert_eq!(sw["verdict"], "pass", "{sw:#}");
    assert!((sw["measured"]["expected_in"].as_f64().unwrap() - 48.0).abs() < 1e-6, "{sw:#}");
    let n = of(&fs, "G-HEIGHT", &j.north);
    assert_eq!(n.first().map(|f| f["verdict"].clone()), Some(json!("pass")), "{n:#?}");
}

#[test]
fn a_device_off_the_governing_plans_spot_fails_its_layout() {
    let mut j = job();
    let (r, fs) = screen(&mut j.s, &["G-LAYOUT"]);
    let gov = |h: &str| of(&fs, "G-LAYOUT", h).into_iter().find(|f| f["check"] == "governing").cloned();
    // The switch where the reference-only power plan drew it: 5 ft off the unit plan's spot.
    let sw = gov(&j.switch).unwrap_or_else(|| panic!("no governing check of the switch: {r:#}"));
    assert_eq!(sw["verdict"], "fail", "{sw:#}");
    assert!((sw["measured"]["off_in"].as_f64().unwrap() - 60.0).abs() < 1.0, "{sw:#}");
    assert_eq!(sw["measured"]["target"], "document", "{sw:#}");
    assert_eq!(sw["expected_source"]["sheet"], "A-801", "{sw:#}");
    assert!(sw["issue"].as_str().unwrap().contains("E-101"), "why the unit plan governs: {sw:#}");
    for h in [&j.north, &j.wrong_note] {
        let f = gov(h).unwrap_or_else(|| panic!("no governing check of {h}: {r:#}"));
        assert_eq!(f["verdict"], "pass", "{f:#}");
    }
}

#[test]
fn a_noted_spot_of_the_governing_plan_no_element_takes_is_reported() {
    let mut j = job();
    let (r, fs) = screen(&mut j.s, &["G-PRESENT"]);
    let loose: Vec<&Value> = fs.iter().filter(|f| f["rule"] == "G-PRESENT" && f["measured"]["basis"] == "governing_plan").collect();
    assert_eq!(loose.len(), 1, "{r:#}");
    let l = loose[0];
    assert_eq!(l["verdict"], "unsure", "{l:#}");
    let at = l["measured"]["expected_at"].as_array().unwrap();
    assert!((at[0].as_f64().unwrap() - X_AT[0]).abs() < 0.2 && (at[1].as_f64().unwrap() - X_AT[1]).abs() < 0.2, "{l:#}");
    // The power plan's symbols are present; their places are told against the governing plan.
    let sw = fs.iter().find(|f| f["rule"] == "G-PRESENT" && f["handle"].as_str().is_some_and(|h| h.eq_ignore_ascii_case(&j.switch))).unwrap();
    assert_eq!(sw["measured"]["governing"]["sheet"], "A-801", "{sw:#}");
}
