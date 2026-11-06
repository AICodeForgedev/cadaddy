//! Switches and lights (Q6145) on a made-up job: guest rooms 101 and 102 beside a corridor.
//!
//! In 101: a type A downlight on leg a (its schedule row and submittal agree), a type B downlight
//! on leg b (the schedule and the submittal give it different catalog numbers, and the schedule
//! says it doesn't dim), a type Q downlight on leg c (the fixture schedule has no Q, and no
//! switch is on leg c), an emergency type EM fixture with no leg, a switch on leg a, a 0-10V
//! dimmer on leg b and a lone 3-way switch on leg a. The plan shows a second type A that the
//! model lacks. In 102: a type A on leg a between a complete 3-way pair.

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
/// The electrical legend sheet, beside the plan.
const E001: f64 = 6000.0;

const RULES: [&str; 6] = ["E-LUM-TYPE", "E-LUM-CONTROL", "E-SWITCH-LEG", "E-SWITCH-DIM", "E-SWITCH-3WAY", "E-LUM-COUNT"];

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

/// Device blocks at building points (feet), each with a TYPE attribute.
fn place(s: Session, devs: &[([f64; 2], &str)]) -> (Session, Vec<String>) {
    let mut d = (*s.doc().unwrap()).clone();
    let style = d.model.iter().find_map(|e| if let EntityKind::Text(t) = &e.kind { Some(t.clone()) } else { None }).unwrap();
    if !d.blocks.contains_key("DEV") {
        let mut b = Block::new("DEV");
        let h = d.new_handle();
        b.entities.push(Entity::new(h, EntityKind::Line(Line { a: Vec3::new(-4.0, 0.0, 0.0), b: Vec3::new(4.0, 0.0, 0.0) })));
        d.blocks.insert("DEV".into(), Arc::new(b));
    }
    let mut hs = Vec::new();
    for (p, ty) in devs {
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
        let q = at(*p);
        let ins = Insert {
            block: "DEV".into(),
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
        e.common.layer = "E-LITE".into();
        d.model.push(e);
        hs.push(h.hex());
    }
    let mut s2 = Session::empty();
    s2.open_drawing(d, "Sample Job", None);
    (s2, hs)
}

/// Store properties on objects, as the job's readers do.
fn set_props(s: Session, sets: &[(&str, &str, &str)]) -> Session {
    let mut d = (*s.doc().unwrap()).clone();
    for (handle, key, value) in sets {
        let h = cadaddy_engine::doc::Handle::parse_hex(handle).unwrap();
        d.modify_entity(h, |e| {
            let mut store = props::read(&e.common.xdata);
            store.set(key, Prop::new(json!(value), Source::computed()));
            e.common.xdata = props::write(&e.common.xdata, &store).into();
        })
        .unwrap();
    }
    let mut s2 = Session::empty();
    s2.open_drawing(d, "Sample Job", None);
    s2
}

fn prints() -> Value {
    let sym = |kind: &str, mark: &str, p: [f64; 2]| json!({"sheet": "E-101", "level": "L1", "kind": kind, "mark": mark, "at": p});
    json!({
        "version": 1,
        "symbols": [
            sym("luminaire", "A", [5.0, 6.0]), sym("luminaire", "A", [6.0, 8.0]), sym("luminaire", "B", [8.0, 6.0]),
            sym("luminaire", "Q", [11.0, 6.0]), sym("luminaire", "EM", [12.0, 9.0]),
            sym("switch", "S", [0.8, 3.0]), sym("switch", "SD", [0.8, 4.0]), sym("switch", "S3", [0.8, 5.0]),
            sym("luminaire", "A", [22.0, 6.0]), sym("switch", "S3", [15.5, 3.0]), sym("switch", "S3", [29.5, 3.0])
        ],
        "schedule": [
            {"sheet": "E-601", "schedule": "LIGHTING FIXTURE SCHEDULE", "mark": "A", "kind": "luminaire",
             "fields": {"description": "4\" RECESSED DOWNLIGHT", "catalogNumber": "AC-100", "voltage": "120V", "dimming": "0-10V", "cct": "3500K"}},
            {"sheet": "E-601", "schedule": "LIGHTING FIXTURE SCHEDULE", "mark": "B", "kind": "luminaire",
             "fields": {"description": "6\" RECESSED DOWNLIGHT", "catalogNumber": "XB-100", "dimming": "NON-DIM"}},
            {"sheet": "E-601", "schedule": "LIGHTING FIXTURE SCHEDULE", "mark": "EM", "kind": "luminaire",
             "fields": {"description": "EMERGENCY BATTERY DOWNLIGHT", "catalogNumber": "EB-1"}}
        ],
        "submittals": [
            {"mark": "A", "section": "26 51 19", "fields": {"manufacturer": "Acme Lighting", "catalogNumber": "AC-100-L35-MVOLT", "dimming": "0-10V", "cct": "35K"}},
            {"mark": "B", "section": "26 51 19", "fields": {"manufacturer": "Acme Lighting", "catalogNumber": "ZZ-900"}},
            {"mark": "EM", "section": "26 51 19", "fields": {"manufacturer": "Acme Lighting", "catalogNumber": "EB-1"}}
        ]
    })
}

fn spec() -> Value {
    let wall = json!({"ceiling": false, "aff_ft": 4.0, "size_ft": [0.25, 0.1, 0.33]});
    let lum = json!({"ceiling": true, "aff_ft": 9.0, "size_ft": [0.5, 0.5, 0.3]});
    json!({
        "units_per_foot": 12,
        "levels": [{"name": "L1", "elevation_ft": 0, "height_ft": 10, "ceiling_ft": 9, "base": [OX, OY], "region": [0, 0, SW, SH]}],
        "layers": [
            {"layer": "A-WALL-CL", "role": "wall", "wall_type": "Partition"},
            {"layer": "A-WALL", "role": "wall", "wall_type": "Exterior"},
            {"layer": "E-LITE", "role": "device"},
            {"layer": "A-ROOM-TAG", "role": "room_tag"}
        ],
        "wall_types": [
            {"name": "Exterior", "thickness_in": 6},
            {"name": "Partition", "thickness_in": 4.875, "from": "centerline"}
        ],
        "rooms": [{"name": "Guest Room", "number": "101"}, {"name": "Guest Room", "number": "102"}, {"name": "Corridor", "number": "103"}],
        "devices": {"by_type": {"A": lum, "B": lum, "Q": lum, "EM": lum, "S": wall, "SD": wall, "S3": wall}},
        "sheets": [
            {"number": "A-101", "title": "FLOOR PLAN - LEVEL 1", "discipline": "A", "level": "L1", "source": "model", "rect": [0.0, 0.0, SW, SH],
             "registration": {"reference": "A-101", "method": "reference", "a": 1.0, "b": 0.0, "tx": 0.0, "ty": 0.0, "matched": 4}},
            {"number": "E-001", "title": "ELECTRICAL LEGEND", "discipline": "E", "source": "model", "rect": [E001, 0.0, E001 + SW, SH]}
        ],
        "prints": prints()
    })
}

struct Job {
    s: Session,
    a: String,
    b: String,
    q: String,
    em: String,
    sw: String,
    dimmer: String,
    lone: String,
    a2: String,
    pair: [String; 2],
}

fn job() -> Job {
    let mut s = Session::new();
    // A 60' × 12' shell; partitions at 15' and 30'.
    layer(&mut s, "A-WALL");
    rect(&mut s, [0.0, 0.0], [60.0, 12.0]);
    rect(&mut s, [0.5, 0.5], [59.5, 11.5]);
    layer(&mut s, "A-WALL-CL");
    for x in [15.0, 30.0] {
        line(&mut s, [x, 0.5], [x, 11.5]);
    }
    layer(&mut s, "A-ROOM-TAG");
    text(&mut s, at([5.0, 2.0]), "101");
    text(&mut s, at([20.0, 2.0]), "102");
    text(&mut s, at([45.0, 6.0]), "103");
    layer(&mut s, "G-FRAME");
    s.execute("pline", &json!({"vertices": [[E001, 0.0], [E001 + SW, 0.0], [E001 + SW, SH], [E001, SH]], "closed": true})).unwrap();
    let (s, h) = place(
        s,
        &[
            ([5.0, 6.0], "A"),
            ([8.0, 6.0], "B"),
            ([11.0, 6.0], "Q"),
            ([12.0, 9.0], "EM"),
            ([0.8, 3.0], "S"),
            ([0.8, 4.0], "SD"),
            ([0.8, 5.0], "S3"),
            ([22.0, 6.0], "A"),
            ([15.5, 3.0], "S3"),
            ([29.5, 3.0], "S3"),
        ],
    );
    let mut s = s;
    s.execute("bspec.set", &json!({"spec": spec(), "replace": true})).unwrap();
    s.execute("model.build", &json!({})).unwrap();
    let kind = |s: &mut Session, hs: &[&String], k: &str| {
        s.execute("bim.source", &json!({"handles": hs, "status": "fixed", "source": "E-101", "kind": k})).unwrap();
    };
    kind(&mut s, &[&h[0], &h[1], &h[2], &h[3], &h[7]], "luminaire");
    kind(&mut s, &[&h[4], &h[5], &h[6], &h[8], &h[9]], "switch");
    let s = set_props(
        s,
        &[
            (&h[0], "switch_leg", "a"),
            (&h[1], "switch_leg", "b"),
            (&h[2], "switch_leg", "c"),
            (&h[4], "switch_leg", "a"),
            (&h[4], "type", "switch, single pole"),
            (&h[5], "switch_leg", "b"),
            (&h[5], "type", "DIMMER, 0-10V"),
            (&h[6], "switch_leg", "a"),
            (&h[6], "type", "switch, 3-way"),
            (&h[7], "switch_leg", "a"),
            (&h[8], "switch_leg", "a"),
            (&h[8], "type", "switch, 3-way"),
            (&h[9], "switch_leg", "a"),
            (&h[9], "type", "switch, 3-way"),
        ],
    );
    Job {
        s,
        a: h[0].clone(),
        b: h[1].clone(),
        q: h[2].clone(),
        em: h[3].clone(),
        sw: h[4].clone(),
        dimmer: h[5].clone(),
        lone: h[6].clone(),
        a2: h[7].clone(),
        pair: [h[8].clone(), h[9].clone()],
    }
}

fn screen(s: &mut Session) -> (Value, Vec<Value>) {
    let r = s.execute("validate.prescreen", &json!({"level": "L1", "all": true, "limit": 50, "rules": RULES})).unwrap();
    let mut fs: Vec<Value> = r["findings"].as_array().cloned().unwrap_or_default();
    let mut cursor = r["next"]["cursor"].as_u64();
    while let Some(c) = cursor {
        let p = s.execute("validate.prescreen", &json!({"level": "L1", "all": true, "limit": 50, "rules": RULES, "cursor": c})).unwrap();
        fs.extend(p["findings"].as_array().cloned().unwrap_or_default());
        cursor = p["next"]["cursor"].as_u64();
    }
    assert!(fs.iter().all(|f| RULES.contains(&f["rule"].as_str().unwrap_or_default())), "{r}");
    (r, fs)
}

fn of<'a>(fs: &'a [Value], rule: &str, handle: &str) -> &'a Value {
    fs.iter()
        .find(|f| f["rule"] == rule && f["handle"].as_str().is_some_and(|h| h.eq_ignore_ascii_case(handle)))
        .unwrap_or_else(|| panic!("no {rule} finding for {handle}: {fs:#?}"))
}

#[test]
fn the_lighting_rules_are_built_in_and_on() {
    let mut s = Session::new();
    s.execute("new", &json!({})).unwrap();
    for id in RULES {
        let r = s.execute("validate.rules", &json!({"id": id})).unwrap();
        assert_eq!(r["rules"][0]["enabled"], true, "{r}");
        assert_eq!(r["rules"][0]["source"], "built-in", "{r}");
    }
    let ids = |k: &str, s: &mut Session| -> Vec<String> {
        let r = s.execute("validate.rules", &json!({"kind": k, "detail": false, "limit": 50})).unwrap();
        r["rules"].as_array().unwrap().iter().map(|x| x["id"].as_str().unwrap().to_string()).collect()
    };
    let lum = ids("wall_luminaire", &mut s);
    assert!(lum.contains(&"E-LUM-TYPE".to_string()) && lum.contains(&"E-LUM-CONTROL".to_string()), "{lum:?}");
    let dim = ids("dimmer", &mut s);
    assert!(dim.contains(&"E-SWITCH-DIM".to_string()) && dim.contains(&"E-SWITCH-LEG".to_string()), "{dim:?}");
    assert!(ids("vacancy_sensor", &mut s).contains(&"E-SWITCH-LEG".to_string()));
    assert!(!ids("receptacle", &mut s).iter().any(|x| x.starts_with("E-LUM") || x.starts_with("E-SWITCH-")));
}

#[test]
fn types_against_the_fixture_schedule_and_the_submittal() {
    let mut j = job();
    let (r, fs) = screen(&mut j.s);
    // A: its schedule row and submittal agree (a base catalog number and its ordering string).
    let a = of(&fs, "E-LUM-TYPE", &j.a);
    assert_eq!(a["verdict"], "pass", "{a}");
    assert!(a["measured"]["schedule"][0]["source"].as_str().unwrap().contains("E-601"), "{a}");
    // B: the schedule and the submittal give different catalog numbers: a conflict citing both.
    let b = of(&fs, "E-LUM-TYPE", &j.b);
    assert_eq!(b["verdict"], "conflict", "{b}");
    assert!(b["issue"].as_str().unwrap().contains("XB-100") && b["issue"].as_str().unwrap().contains("ZZ-900"), "{b}");
    let docs: Vec<&str> = b["documents"].as_array().unwrap().iter().filter_map(|d| d["document"].as_str()).collect();
    assert!(docs.iter().any(|d| d.contains("LIGHTING FIXTURE SCHEDULE")) && docs.iter().any(|d| d.contains("submittal")), "{b}");
    // Q: on the plan, not in the fixture schedule: a conflict between the plan and the schedule.
    let q = of(&fs, "E-LUM-TYPE", &j.q);
    assert_eq!(q["verdict"], "conflict", "{q}");
    assert!(q["issue"].as_str().unwrap().contains("not in the fixture schedule"), "{q}");
    assert_eq!(q["documents"].as_array().unwrap().len(), 2, "{q}");
    assert!(r["checked"]["E-LUM-TYPE"]["conflict"].as_u64().unwrap() >= 2, "{r}");
}

#[test]
fn a_model_product_that_differs_from_the_documents_fails() {
    let j = job();
    let a = j.a.clone();
    let mut s = set_props(j.s, &[(&a, "catalog_number", "OTHER-55")]);
    let (_, fs) = screen(&mut s);
    let f = of(&fs, "E-LUM-TYPE", &a);
    assert_eq!(f["verdict"], "fail", "{f}");
    assert_eq!(f["measured"]["mismatches"][0]["field"], "catalog", "{f}");
}

#[test]
fn fixtures_need_a_control_unless_emergency() {
    let mut j = job();
    let (r, fs) = screen(&mut j.s);
    let a = of(&fs, "E-LUM-CONTROL", &j.a);
    assert_eq!(a["verdict"], "pass", "{a}");
    assert!(a["issue"].as_str().unwrap().contains(&j.sw) || a["measured"]["controlled_by"].to_string().contains(&j.sw), "{a}");
    // Q is on leg c: no switch in 101 is on c.
    let q = of(&fs, "E-LUM-CONTROL", &j.q);
    assert_eq!(q["verdict"], "fail", "{q}");
    assert!(q["issue"].as_str().unwrap().contains("leg c"), "{q}");
    // The emergency fixture: no control needed, its schedule row cited.
    let em = of(&fs, "E-LUM-CONTROL", &j.em);
    assert_eq!(em["verdict"], "pass", "{em}");
    assert!(em["issue"].as_str().unwrap().contains("no control needed") && em["issue"].as_str().unwrap().contains("EMERGENCY"), "{em}");
    // The second room's fixture: its 3-way pair controls it.
    assert_eq!(of(&fs, "E-LUM-CONTROL", &j.a2)["verdict"], "pass");
    // Every switch reaches a fixture of its room on its leg.
    for h in [&j.sw, &j.dimmer, &j.lone, &j.pair[0], &j.pair[1]] {
        assert_eq!(of(&fs, "E-SWITCH-LEG", h)["verdict"], "pass", "{h}: {r}");
    }
}

#[test]
fn a_switch_leg_with_no_fixture_fails() {
    let j = job();
    let sw = j.sw.clone();
    let mut s = set_props(j.s, &[(&sw, "switch_leg", "x")]);
    let (_, fs) = screen(&mut s);
    let f = of(&fs, "E-SWITCH-LEG", &sw);
    assert_eq!(f["verdict"], "fail", "{f}");
    assert!(f["issue"].as_str().unwrap().contains("leg x"), "{f}");
}

#[test]
fn a_dimmer_on_a_non_dim_driver_fails() {
    let mut j = job();
    let (_, fs) = screen(&mut j.s);
    let d = of(&fs, "E-SWITCH-DIM", &j.dimmer);
    assert_eq!(d["verdict"], "fail", "{d}");
    assert_eq!(d["measured"]["protocol"], "0-10V", "{d}");
    assert!(d["issue"].as_str().unwrap().contains("not dimmable"), "{d}");
    assert!(d["measured"]["drivers"][0]["from"].as_str().unwrap().contains("E-601"), "the schedule says so: {d}");
    // A plain switch is no dimmer.
    assert!(!fs.iter().any(|f| f["rule"] == "E-SWITCH-DIM" && f["handle"].as_str().is_some_and(|h| h.eq_ignore_ascii_case(&j.sw))));
    // Its fixture made 0-10V: a pass.
    let b = j.b.clone();
    let dim = j.dimmer.clone();
    let mut s = set_props(j.s, &[(&b, "dimming", "0-10V")]);
    let (_, fs) = screen(&mut s);
    assert_eq!(of(&fs, "E-SWITCH-DIM", &dim)["verdict"], "pass");
}

#[test]
fn a_three_way_switch_needs_its_mate() {
    let mut j = job();
    let (r, fs) = screen(&mut j.s);
    let lone = of(&fs, "E-SWITCH-3WAY", &j.lone);
    assert_eq!(lone["verdict"], "missing", "{lone}");
    assert!(lone["issue"].as_str().unwrap().contains("no mate"), "{lone}");
    for h in &j.pair {
        assert_eq!(of(&fs, "E-SWITCH-3WAY", h)["verdict"], "pass", "{h}");
    }
    assert_eq!(r["checked"]["E-SWITCH-3WAY"]["missing"], 1, "{r}");
    // A single-pole switch and the dimmer are no 3-ways.
    assert!(!fs.iter().any(|f| f["rule"] == "E-SWITCH-3WAY" && f["handle"].as_str().is_some_and(|h| h.eq_ignore_ascii_case(&j.sw))));
}

#[test]
fn plan_counts_per_room_against_the_model() {
    let mut j = job();
    let (r, fs) = screen(&mut j.s);
    let counts: Vec<&Value> = fs.iter().filter(|f| f["rule"] == "E-LUM-COUNT").collect();
    let a101 = counts
        .iter()
        .find(|f| f["area"].as_str().unwrap_or_default().contains("101") && f["measured"]["type"] == "A")
        .unwrap_or_else(|| panic!("no count of type A in 101: {counts:#?}"));
    assert_eq!(a101["verdict"], "missing", "{a101}");
    assert_eq!((a101["measured"]["plan"].as_u64(), a101["measured"]["model"].as_u64()), (Some(2), Some(1)), "{a101}");
    let s3_102 = counts.iter().find(|f| f["area"].as_str().unwrap_or_default().contains("102") && f["measured"]["type"] == "S3").unwrap();
    assert_eq!(s3_102["verdict"], "pass", "{s3_102}");
    assert_eq!(r["checked"]["E-LUM-COUNT"]["missing"], 1, "{r}");
    assert_eq!(r["checked"]["E-LUM-COUNT"]["extra"], 0, "{r}");
}

#[test]
fn without_documents_a_type_is_unsure_never_a_pass() {
    let mut j = job();
    let mut sp: Value = serde_json::from_str(j.s.doc().unwrap().building_spec.as_deref().unwrap()).unwrap();
    sp["prints"]["schedule"] = json!([]);
    sp["prints"]["submittals"] = json!([]);
    j.s.doc_mut().unwrap().building_spec = Some(sp.to_string());
    let (r, fs) = screen(&mut j.s);
    let a = of(&fs, "E-LUM-TYPE", &j.a);
    assert_eq!(a["verdict"], "unsure", "{a}");
    assert!(r["unmeasured"]["E-LUM-TYPE"]["the documents read hold no fixture schedule"].as_u64().unwrap() >= 4, "{r}");
    assert_eq!(r["checked"]["E-LUM-TYPE"]["pass"], 0, "{r}");
    // The dimmer's fixture no longer says how it dims: unsure, not a fail or a pass.
    assert_eq!(of(&fs, "E-SWITCH-DIM", &j.dimmer)["verdict"], "unsure");
}

#[test]
fn the_jobs_own_conflicts_are_carried_into_the_run() {
    let mut j = job();
    let mut sp: Value = serde_json::from_str(j.s.doc().unwrap().building_spec.as_deref().unwrap()).unwrap();
    sp["rfis"] = json!([{
        "kind": "sheet_conflict",
        "level": "L1",
        "at": [OX + 5.0 * 12.0, OY + 6.0 * 12.0],
        "sheets": ["E-101", "E-201"],
        "about": "luminaire type A",
        "issue": "the lighting plan and the enlarged plan show type A with different mounting",
        "assumed": "recessed"
    }]);
    j.s.doc_mut().unwrap().building_spec = Some(sp.to_string());
    let (_, fs) = screen(&mut j.s);
    let c = fs.iter().find(|f| f["source"] == "build").unwrap_or_else(|| panic!("no carried conflict: {fs:#?}"));
    assert_eq!((c["verdict"].as_str(), c["rule"].as_str()), (Some("conflict"), Some("E-LUM-TYPE")), "{c}");
    assert!(c["area"].as_str().unwrap().contains("101"), "{c}");
    assert_eq!(c["documents"].as_array().unwrap().len(), 2, "{c}");
    // The run starts from them.
    let out = dir("run");
    let r = j.s.execute("validate", &json!({"level": "L1", "out_dir": out.to_string_lossy(), "rules": RULES})).unwrap();
    let conflicts = &r["prescreen"]["measured"]["conflicts"];
    assert_eq!(conflicts["from_build"], 1, "{r}");
    assert!(conflicts["total"].as_u64().unwrap() >= 3, "the build's and the prescreen's own: {r}");
    let _ = std::fs::remove_dir_all(&out);
}

fn dir(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("cadaddy-validatelighting-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    d
}

#[test]
fn a_packet_carries_the_fixture_schedule_rows_submittals_and_control_legend() {
    let mut j = job();
    // The submittal's type catalog (as bim.submittals keeps it): type A on page 7.
    j.s = set_props(j.s, &[(&j.a, "submittal", "Sample Lighting Package")]);
    {
        let d = j.s.doc_mut().unwrap();
        let mut t = cadaddy_engine::doc::bimspec::BimType::new("A");
        t.category = "luminaire".into();
        t.set_field("catalogNumber", cadaddy_engine::doc::bimspec::SpecField::new("AC-100-L35-MVOLT").with_source("p.7"));
        d.bim_types.set(t);
    }
    // The legend's rows for the controls.
    layer(&mut j.s, "E-LGND");
    let (x, y) = (E001 + 200.0, 1500.0);
    j.s.execute("text", &json!({"at": [x, y], "text": "ELECTRICAL SYMBOL LEGEND", "height": 6})).unwrap();
    j.s.execute("text", &json!({"at": [x, y - 12.0], "text": "S3", "height": 6})).unwrap();
    j.s.execute("text", &json!({"at": [x + 60.0, y - 12.0], "text": "3-WAY SWITCH", "height": 6})).unwrap();
    j.s.execute("text", &json!({"at": [x, y - 24.0], "text": "SD", "height": 6})).unwrap();
    j.s.execute("text", &json!({"at": [x + 60.0, y - 24.0], "text": "DIMMER, 0-10V", "height": 6})).unwrap();
    let areas = j.s.execute("validate.areas", &json!({})).unwrap();
    let id = areas["areas"].as_array().unwrap().iter().find(|a| a["number"] == "101").unwrap()["id"].as_str().unwrap().to_string();
    let out = dir("packet");
    let r = j.s.execute("validate.packet", &json!({"area": id, "out_dir": out.to_string_lossy(), "max_images": 1})).unwrap();
    let o: Value = serde_json::from_str(&std::fs::read_to_string(out.join("objects.json")).unwrap()).unwrap();
    let l = &o["lighting"];
    let ty = |m: &str| l["types"].as_array().unwrap().iter().find(|t| t["type"] == m).cloned().unwrap_or_else(|| panic!("no type {m}: {l}"));
    let a = ty("A");
    assert!(a["schedule"][0]["fields"]["catalogNumber"] == "AC-100", "{a}");
    assert!(a["submittal"]["sources"].to_string().contains("26 51 19"), "{a}");
    assert!(a["submittal"]["cut_sheets"].to_string().contains("Sample Lighting Package p.7"), "{a}");
    assert!(ty("Q")["unknown"].to_string().contains("not in the fixture schedule"), "{l}");
    let legend = l["control_legend"].to_string();
    assert!(legend.contains("3-WAY SWITCH") && legend.contains("DIMMER, 0-10V"), "{l}");
    let sd = l["control_legend"].as_array().unwrap().iter().find(|c| c["mark"] == "SD").unwrap();
    assert_eq!(sd["type"], "DIMMER, 0-10V", "what the job read the dimmer as: {sd}");
    // Each element its facts; the cut sheet page is asked for (the file is not among the job's
    // submittals here).
    let el =
        o["elements"].as_array().unwrap().iter().find(|e| e["handle"].as_str().is_some_and(|h| h.eq_ignore_ascii_case(&j.dimmer))).unwrap().clone();
    assert_eq!(el["lighting"]["dimmer"], true, "{el}");
    assert_eq!(el["lighting"]["protocol"], "0-10V", "{el}");
    assert!(r["left_out"].to_string().contains("submittal Sample Lighting Package"), "{}", r["left_out"]);
    let unlisted = r["submittals_unlisted"].as_array().unwrap().iter().find(|u| u["submittal"] == "Sample Lighting Package").cloned();
    assert!(unlisted.as_ref().is_some_and(|u| u["pages"].as_array().unwrap().contains(&json!(7))), "{}", r["submittals_unlisted"]);
    assert!(unlisted.is_some_and(|u| u["for"].to_string().contains("cut sheet")), "{}", r["submittals_unlisted"]);
    // The packet's prescreen holds the lighting rules.
    for id in RULES {
        assert!(o["prescreen"]["checked"][id].is_object(), "{id}: {}", o["prescreen"]["checked"]);
    }
    let _ = std::fs::remove_dir_all(&out);
}

/// Without the prints' symbols, the plan's count is the job's symbol table: its device tags, at
/// their drawing point over units per foot (the level's base taken off), each symbol in its
/// room or within 2 ft of it (a switch drawn beside its wall line).
#[test]
fn plan_counts_from_the_jobs_symbol_table() {
    let mut j = job();
    let mut sp: Value = serde_json::from_str(j.s.doc().unwrap().building_spec.as_deref().unwrap()).unwrap();
    sp["prints"]["symbols"] = json!([]);
    let tag = |kind: &str, mark: &str, p: [f64; 2]| {
        let q = at(p);
        json!({"id": format!("device:L1:{},{}", p[0], p[1]), "kind": kind, "level": "L1", "at_ft": [q[0] / 12.0, q[1] / 12.0],
               "fields": {"type": {"value": mark, "source": "plan"}}})
    };
    sp["tags"] = json!([
        tag("luminaire", "A", [5.0, 6.0]),
        tag("luminaire", "A", [6.0, 8.0]),
        tag("luminaire", "B", [8.0, 6.0]),
        tag("luminaire", "Q", [11.0, 6.0]),
        tag("luminaire", "EM", [12.0, 9.0]),
        // Drawn 1.3 ft outside 101's outline, beside its wall.
        tag("switch", "S", [-0.8, 3.0]),
        tag("switch", "SD", [0.8, 4.0]),
        tag("switch", "S3", [0.8, 5.0])
    ]);
    j.s.doc_mut().unwrap().building_spec = Some(sp.to_string());
    let (r, fs) = screen(&mut j.s);
    let in101 = |m: &str| {
        fs.iter()
            .find(|f| f["rule"] == "E-LUM-COUNT" && f["area"].as_str().unwrap_or_default().contains("101") && f["measured"]["type"] == m)
            .cloned()
            .unwrap_or_else(|| panic!("no count of {m} in 101: {r}"))
    };
    let a = in101("A");
    assert_eq!((a["verdict"].as_str(), a["measured"]["plan"].as_u64()), (Some("missing"), Some(2)), "{a}");
    assert!(a["measured"]["plan_from"].as_str().unwrap().contains("symbol table"), "{a}");
    for m in ["B", "Q", "EM", "S", "SD", "S3"] {
        assert_eq!(in101(m)["verdict"], "pass", "{m}: {r}");
    }
}
