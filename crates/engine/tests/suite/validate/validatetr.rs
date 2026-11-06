//! E-RECEPT-TR (Q6142) on a made-up job: guest rooms 101 and 102, a typical unit 104 and a
//! corridor 103. In 101 a TR duplex (its type tag says TR), a GFCI whose product record says it
//! is not TR, and a duplex nothing says anything of; one duplex in the corridor and one in the
//! typical unit. Then a blanket general note, a legend row, and a project's own room names.

use std::path::PathBuf;
use std::sync::Arc;

use cadaddy_engine::Session;
use cadaddy_engine::doc::props::{self, Prop, Source};
use cadaddy_engine::doc::{Attrib, AttribModes, AttribProps, Block, Entity, EntityKind, Insert, Line, Text};
use cadaddy_engine::geom::Vec3;
use serde_json::{Value, json};

/// The plan's origin on the drawing (inches).
const OX: f64 = 1200.0;
const OY: f64 = 1000.0;
/// One sheet: 36" × 24" at 1/8" = 1'-0", in drawing inches.
const SW: f64 = 3456.0;
const SH: f64 = 2304.0;
/// The electrical notes sheet, beside the plan.
const E001: f64 = 6000.0;

fn layer(s: &mut Session, name: &str) {
    if s.execute("layer.new", &json!({"name": name, "current": true})).is_err() {
        s.execute("layer.current", &json!({"name": name})).unwrap();
    }
}

/// A building point (feet) on the plan.
fn at(p: [f64; 2]) -> [f64; 2] {
    [OX + p[0] * 12.0, OY + p[1] * 12.0]
}

fn rect(s: &mut Session, a: [f64; 2], b: [f64; 2]) {
    let v = [at(a), at([b[0], a[1]]), at(b), at([a[0], b[1]])];
    s.execute("pline", &json!({"vertices": v, "closed": true})).unwrap();
}

fn line(s: &mut Session, a: [f64; 2], b: [f64; 2]) {
    s.execute("line", &json!({"points": [at(a), at(b)]})).unwrap();
}

fn text(s: &mut Session, p: [f64; 2], t: &str) {
    s.execute("text", &json!({"at": p, "text": t, "height": 6})).unwrap();
}

/// Receptacle blocks at building points (feet) with a TYPE attribute.
fn place(s: Session, pts: &[[f64; 2]], ty: &str) -> (Session, Vec<String>) {
    let mut d = (*s.doc().unwrap()).clone();
    let style = d.model.iter().find_map(|e| if let EntityKind::Text(t) = &e.kind { Some(t.clone()) } else { None }).unwrap();
    if !d.blocks.contains_key("RECEP") {
        let mut b = Block::new("RECEP");
        let h = d.new_handle();
        b.entities.push(Entity::new(h, EntityKind::Line(Line { a: Vec3::new(-4.0, 0.0, 0.0), b: Vec3::new(4.0, 0.0, 0.0) })));
        d.blocks.insert("RECEP".into(), Arc::new(b));
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
            block: "RECEP".into(),
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

fn spec() -> Value {
    let dev = json!({"ceiling": false, "aff_ft": 1.5, "size_ft": [0.25, 0.1, 0.33]});
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
        "rooms": [{"name": "Guest Room", "number": "101"}, {"name": "Guest Room", "number": "102"}, {"name": "Corridor", "number": "103"}, {"name": "Unit", "number": "104"}],
        "devices": {"by_type": {"DUPLEX": dev, "DUPLEX-TR": dev, "GFCI": dev}},
        "sheets": [
            {"number": "A-101", "title": "FLOOR PLAN - LEVEL 1", "discipline": "A", "level": "L1", "source": "model", "rect": [0.0, 0.0, SW, SH],
             "registration": {"reference": "A-101", "method": "reference", "a": 1.0, "b": 0.0, "tx": 0.0, "ty": 0.0, "matched": 4}},
            {"number": "E-001", "title": "ELECTRICAL NOTES", "discipline": "E", "source": "model", "rect": [E001, 0.0, E001 + SW, SH]}
        ]
    })
}

struct Job {
    s: Session,
    tr: String,
    gfci: String,
    plain: String,
    corridor: String,
    unit: String,
    /// In 101's south wall, 2" outside the room's outline (on its wall line).
    wall_line: String,
}

fn job() -> Job {
    let mut s = Session::new();
    // A 100' × 12' shell; partitions at 15', 30' and 60'.
    layer(&mut s, "A-WALL");
    rect(&mut s, [0.0, 0.0], [100.0, 12.0]);
    rect(&mut s, [0.5, 0.5], [99.5, 11.5]);
    layer(&mut s, "A-WALL-CL");
    for x in [15.0, 30.0, 60.0] {
        line(&mut s, [x, 0.5], [x, 11.5]);
    }
    layer(&mut s, "A-ROOM-TAG");
    text(&mut s, at([5.0, 6.0]), "101");
    text(&mut s, at([20.0, 6.0]), "102");
    text(&mut s, at([45.0, 6.0]), "103");
    text(&mut s, at([80.0, 6.0]), "104");
    layer(&mut s, "G-FRAME");
    s.execute("pline", &json!({"vertices": [[E001, 0.0], [E001 + SW, 0.0], [E001 + SW, SH], [E001, SH]], "closed": true})).unwrap();
    let (s, tr) = place(s, &[[3.0, 0.5]], "DUPLEX-TR");
    let (s, gfci) = place(s, &[[8.0, 0.5]], "GFCI");
    let (s, plain) = place(s, &[[12.0, 0.5], [45.0, 0.5], [80.0, 0.5], [13.0, 0.33]], "DUPLEX");
    let mut s = s;
    s.execute("bspec.set", &json!({"spec": spec(), "replace": true})).unwrap();
    s.execute("model.build", &json!({})).unwrap();
    for h in [&tr[0], &plain[0], &plain[1], &plain[2], &plain[3]] {
        s.execute("bim.source", &json!({"handles": [h], "status": "fixed", "source": "E-101", "kind": "receptacle"})).unwrap();
    }
    s.execute("bim.source", &json!({"handles": [&gfci[0]], "status": "fixed", "source": "E-101", "kind": "gfci_receptacle"})).unwrap();
    // Room 104 is a typical unit (its name says nothing of guests or dwellings).
    let mut sp: Value = serde_json::from_str(s.doc().unwrap().building_spec.as_deref().unwrap()).unwrap();
    sp["unit_types"] = json!([{"id": "ut-a", "tag": "A", "name": "TYPE A", "width_ft": 39.0, "depth_ft": 11.0, "items": []}]);
    sp["unit_instances"] = json!([{"type": "ut-a", "room": "104", "number": "104", "level": "L1", "hand": "normal", "at_ft": [60.0, 0.5]}]);
    s.doc_mut().unwrap().building_spec = Some(sp.to_string());
    // The GFCI's product record, as the job's spec and submittal reading stores it (a property
    // holds a scalar: the record's JSON text).
    let record = json!({"resolved": {"maker": "Acme", "features": ["GFCI", "duplex"], "sources": {"features": "submittal 26 27 26-1 p. 4"}}});
    let s = set_prop(s, &gfci[0], "product", json!(record.to_string()));
    Job {
        s,
        tr: tr[0].clone(),
        gfci: gfci[0].clone(),
        plain: plain[0].clone(),
        corridor: plain[1].clone(),
        unit: plain[2].clone(),
        wall_line: plain[3].clone(),
    }
}

fn tr_findings(s: &mut Session) -> (Value, Vec<Value>) {
    let r = s.execute("validate.prescreen", &json!({"level": "L1", "all": true, "limit": 50, "rules": ["E-RECEPT-TR"]})).unwrap();
    let fs: Vec<Value> = r["findings"].as_array().cloned().unwrap_or_default();
    assert!(fs.iter().all(|f| f["rule"] == "E-RECEPT-TR"), "{r}");
    (r, fs)
}

fn of<'a>(fs: &'a [Value], handle: &str) -> Option<&'a Value> {
    fs.iter().find(|f| f["handle"].as_str().is_some_and(|h| h.eq_ignore_ascii_case(handle)))
}

#[test]
fn the_rule_is_built_in_and_on_with_its_code_cited() {
    let mut s = Session::new();
    s.execute("new", &json!({})).unwrap();
    let r = s.execute("validate.rules", &json!({"id": "E-RECEPT-TR"})).unwrap();
    let rule = &r["rules"][0];
    assert_eq!(rule["enabled"], true, "{r}");
    assert_eq!(rule["source"], "built-in", "{r}");
    assert_eq!(rule["severity"], "high");
    assert_eq!(rule["code"]["cite"], "NEC 406.12", "{r}");
    let rooms: Vec<&str> = rule["code"]["rooms"].as_array().unwrap().iter().filter_map(Value::as_str).collect();
    for want in ["dwelling unit", "guest room", "guest suite"] {
        assert!(rooms.contains(&want), "{rooms:?}");
    }
    // A GFCI receptacle gets it as well as its type rule; a data outlet doesn't.
    let ids = |k: &str, s: &mut Session| -> Vec<String> {
        let r = s.execute("validate.rules", &json!({"kind": k, "detail": false, "limit": 50})).unwrap();
        r["rules"].as_array().unwrap().iter().map(|x| x["id"].as_str().unwrap().to_string()).collect()
    };
    let g = ids("gfci_receptacle", &mut s);
    assert!(g.contains(&"E-RECEPT-TR".to_string()) && g.contains(&"E-RECEPT-TYPE".to_string()), "{g:?}");
    for k in ["usb_receptacle", "quad_receptacle", "duplex_receptacle"] {
        assert!(ids(k, &mut s).contains(&"E-RECEPT-TR".to_string()), "{k}");
    }
    assert!(!ids("data_outlet", &mut s).contains(&"E-RECEPT-TR".to_string()));
}

#[test]
fn guest_room_receptacles_pass_fail_or_are_unsure_and_a_corridor_is_not_checked() {
    let mut j = job();
    let (r, fs) = tr_findings(&mut j.s);
    // The TR duplex: its type tag says TR.
    let tr = of(&fs, &j.tr).unwrap_or_else(|| panic!("no E-RECEPT-TR for the TR duplex: {r}"));
    assert_eq!(tr["verdict"], "pass", "{tr}");
    assert_eq!(tr["measured"]["from"], "type_tag", "{tr}");
    assert!(tr["measured"]["source"].as_str().unwrap().contains("DUPLEX-TR"), "{tr}");
    assert!(tr["measured"]["required_because"].as_str().unwrap().contains("guest room"), "{tr}");
    // The GFCI whose product is not TR: a high fail citing the record.
    let g = of(&fs, &j.gfci).unwrap_or_else(|| panic!("no E-RECEPT-TR for the GFCI: {r}"));
    assert_eq!(g["verdict"], "fail", "{g}");
    assert_eq!(g["severity"], "high");
    assert_eq!(g["measured"]["from"], "product", "{g}");
    assert!(g["measured"]["source"].as_str().unwrap().contains("submittal 26 27 26-1"), "{g}");
    assert_eq!(g["measured"]["tamper_resistant"], false);
    // Nothing says: unsure, counted unmeasured, never a pass.
    let p = of(&fs, &j.plain).unwrap_or_else(|| panic!("no E-RECEPT-TR for the plain duplex: {r}"));
    assert_eq!(p["verdict"], "unsure", "{p}");
    assert!(p["issue"].as_str().unwrap().contains("tamper-resistance not stated in the model or documents"), "{p}");
    assert_eq!(r["unmeasured"]["E-RECEPT-TR"]["tamper-resistance not stated in the model or documents"], 3, "the plain duplexes and the unit's: {r}");
    // On the room's wall line, just outside its outline: the room's.
    let w = of(&fs, &j.wall_line).unwrap_or_else(|| panic!("no E-RECEPT-TR for the receptacle on 101's wall line: {r}"));
    assert!(w["area"].as_str().unwrap().contains("101"), "{w}");
    // The corridor's is not checked; the typical unit's is (by its unit type).
    assert!(of(&fs, &j.corridor).is_none(), "{r}");
    let u = of(&fs, &j.unit).unwrap_or_else(|| panic!("no E-RECEPT-TR in the typical unit: {r}"));
    assert!(u["measured"]["required_because"].as_str().unwrap().contains("typical unit"), "{u}");
    assert_eq!(r["checked"]["E-RECEPT-TR"], json!({"pass": 1, "fail": 1, "unsure": 3}), "{r}");
    // The GFCI is checked for its type too: the rules for it hold both.
    let rules = j.s.execute("validate.rules", &json!({"kind": "gfci_receptacle", "detail": false, "limit": 50})).unwrap();
    assert!(rules.to_string().contains("E-RECEPT-TYPE"), "{rules}");
}

#[test]
fn a_blanket_general_note_makes_every_receptacle_tr_and_is_cited() {
    let mut j = job();
    layer(&mut j.s, "E-NOTE");
    j.s.execute("text", &json!({"at": [E001 + 200.0, 2000.0], "text": "GENERAL NOTES", "height": 6})).unwrap();
    j.s.execute("text", &json!({"at": [E001 + 200.0, 1980.0], "text": "1. ALL RECEPTACLES SHALL BE TAMPER-RESISTANT.", "height": 6})).unwrap();
    let (r, fs) = tr_findings(&mut j.s);
    let p = of(&fs, &j.plain).unwrap();
    assert_eq!(p["verdict"], "pass", "{p}");
    assert_eq!(p["measured"]["from"], "note", "{p}");
    let src = p["measured"]["source"].as_str().unwrap();
    assert!(src.contains("E-001") && src.contains("ALL RECEPTACLES SHALL BE TAMPER-RESISTANT"), "{p}");
    // The product record still comes first: the GFCI fails; the corridor stays unchecked.
    assert_eq!(of(&fs, &j.gfci).unwrap()["verdict"], "fail");
    assert!(of(&fs, &j.corridor).is_none());
    assert_eq!(r["checked"]["E-RECEPT-TR"]["unsure"], 0, "{r}");
}

#[test]
fn a_legend_row_says_a_mark_is_tr() {
    let mut j = job();
    layer(&mut j.s, "E-LGND");
    let (x, y) = (E001 + 200.0, 1500.0);
    j.s.execute("text", &json!({"at": [x, y], "text": "ELECTRICAL SYMBOL LEGEND", "height": 6})).unwrap();
    j.s.execute("text", &json!({"at": [x, y - 12.0], "text": "DUPLEX", "height": 6})).unwrap();
    j.s.execute("text", &json!({"at": [x + 60.0, y - 12.0], "text": "DUPLEX RECEPTACLE, TAMPER RESISTANT", "height": 6})).unwrap();
    let (r, fs) = tr_findings(&mut j.s);
    let p = of(&fs, &j.plain).unwrap();
    assert_eq!((p["verdict"].as_str(), p["measured"]["from"].as_str()), (Some("pass"), Some("legend")), "{r}");
}

#[test]
fn a_project_adds_rooms_the_code_scope_covers() {
    let mut j = job();
    let refused = j.s.execute("validate.rules.set", &json!({"id": "G-HOST", "rooms": ["corridor"], "reason": "made-up test"}));
    assert!(refused.unwrap_err().to_string().contains("checks no code requirement"));
    let bad = j.s.execute(
        "project.rules.set",
        &json!({"domain": "tolerance", "key": "validate.E-RECEPT-TR.rooms", "value": "corridor", "reason": "made-up test"}),
    );
    assert!(bad.is_err(), "rooms live in domain custom");
    let r = j.s.execute("validate.rules.set", &json!({"id": "E-RECEPT-TR", "rooms": ["Corridor"], "reason": "made-up test"})).unwrap();
    assert!(r["rule"]["code"]["rooms"].as_array().unwrap().iter().any(|x| x == "Corridor"), "{r}");
    assert_eq!(r["rule"]["source"], "project", "{r}");
    let (_, fs) = tr_findings(&mut j.s);
    let c = of(&fs, &j.corridor).unwrap_or_else(|| panic!("the corridor is covered now: {fs:?}"));
    assert_eq!(c["verdict"], "unsure", "{c}");
    // Turned off: no findings.
    j.s.execute("validate.rules.set", &json!({"id": "E-RECEPT-TR", "enabled": false, "reason": "made-up test"})).unwrap();
    let r = j.s.execute("validate.prescreen", &json!({"level": "L1", "all": true, "limit": 50})).unwrap();
    assert!(r["findings"].as_array().unwrap().iter().all(|f| f["rule"] != "E-RECEPT-TR"), "{r}");
    assert!(r["disabled"].as_array().unwrap().iter().any(|x| x == "E-RECEPT-TR"), "{r}");
}

fn dir(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("cadaddy-validatetr-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    d
}

#[test]
fn a_packet_carries_each_receptacles_tr_fact_and_source() {
    let mut j = job();
    let areas = j.s.execute("validate.areas", &json!({})).unwrap();
    let id = areas["areas"].as_array().unwrap().iter().find(|a| a["number"] == "101").unwrap()["id"].as_str().unwrap().to_string();
    let out = dir("packet");
    let r = j.s.execute("validate.packet", &json!({"area": id, "out_dir": out.to_string_lossy(), "max_images": 1})).unwrap();
    let folder = r["dir"].as_str().or_else(|| r["path"].as_str()).map(PathBuf::from).unwrap_or_else(|| out.clone());
    let objects = walk(&folder, "objects.json").unwrap_or_else(|| panic!("no objects.json under {}: {r}", folder.display()));
    let o: Value = serde_json::from_str(&std::fs::read_to_string(&objects).unwrap()).unwrap();
    let el = |h: &str| {
        o["elements"]
            .as_array()
            .unwrap()
            .iter()
            .find(|e| e["handle"].as_str().is_some_and(|x| x.eq_ignore_ascii_case(h)))
            .cloned()
            .unwrap_or_else(|| panic!("{h} not in {o}"))
    };
    let tr = el(&j.tr);
    assert_eq!(tr["tamper_resistant"]["tr"], true, "{tr}");
    assert_eq!(tr["tamper_resistant"]["from"], "type_tag", "{tr}");
    assert_eq!(tr["tamper_resistant"]["required"], true, "{tr}");
    let g = el(&j.gfci);
    assert_eq!(g["tamper_resistant"]["tr"], false, "{g}");
    assert!(g["tamper_resistant"]["source"].as_str().unwrap().contains("product record"), "{g}");
    let p = el(&j.plain);
    assert!(p["tamper_resistant"]["tr"].is_null() && p["tamper_resistant"]["from"] == "none", "{p}");
    // The packet's rules hold E-RECEPT-TR, its prescreen the findings.
    assert!(o["prescreen"]["checked"]["E-RECEPT-TR"].is_object(), "{}", o["prescreen"]);
    let _ = std::fs::remove_dir_all(&out);
}

/// The first file named `name` under `dir`.
fn walk(dir: &std::path::Path, name: &str) -> Option<PathBuf> {
    for e in std::fs::read_dir(dir).ok()?.flatten() {
        let p = e.path();
        if p.is_dir() {
            if let Some(f) = walk(&p, name) {
                return Some(f);
            }
        } else if p.file_name().is_some_and(|n| n == name) {
            return Some(p);
        }
    }
    None
}
