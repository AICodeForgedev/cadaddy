//! One MEP element at a time (Q6502): a duct segment added onto an existing run's end, a register
//! moved 2 ft, a pipe deleted and a unit's type mark changed, each with one command; the plan's
//! runs read again and the schedules' lists written again (as a job's next pass does), the model
//! rebuilt: all four edits are still there, each with the documents' value beside it, and
//! `bim.chain` walks from a register through the added segment to the unit.

use cadaddy_engine::Session;
use cadaddy_engine::cmd::wallgen::made_as;
use cadaddy_engine::doc::{Entity, bimspec, props};
use serde_json::{Value, json};

fn layer(s: &mut Session, name: &str) {
    if s.execute("layer.new", &json!({"name": name, "current": true})).is_err() {
        s.execute("layer.current", &json!({"name": name})).unwrap();
    }
}

fn rect(s: &mut Session, a: [f64; 2], b: [f64; 2]) {
    s.execute("pline", &json!({"vertices": [a, [b[0], a[1]], b, [a[0], b[1]]], "closed": true})).unwrap();
}

/// What the schedule and terminal readers write: an air handler at the supply duct's start and
/// two registers, one 2 ft off the duct, one by the corner the added segment will turn.
fn read_lists(s: &mut Session) {
    s.execute(
        "bspec.set",
        &json!({"spec": {"mep": {
            "terminals": [
                {"system": "M-DUCT-SUPP", "level": "L1", "at": [150.0, 84.0], "size_in": [12.0, 12.0], "bottom_ft": 9.0, "terminal_kind": "register"},
                {"system": "M-DUCT-SUPP", "level": "L1", "at": [276.0, 170.0], "size_in": [12.0, 12.0], "bottom_ft": 9.0, "terminal_kind": "register"}
            ],
            "equipment": [
                {"tag": "AHU-1", "kind": "ahu", "level": "L1", "min": [0.0, 40.0], "max": [24.0, 80.0], "bottom_ft": 8.0, "height_ft": 3.0, "systems": ["M-DUCT-SUPP"], "confidence": 1.0}
            ]
        }}}),
    )
    .unwrap();
}

/// A small building: one storey's walls, a tagged supply duct and a cold water line on the plan.
fn building() -> Session {
    let mut s = Session::new();
    layer(&mut s, "A-WALL");
    rect(&mut s, [-12.0, -12.0], [492.0, 372.0]);
    rect(&mut s, [-6.0, -6.0], [486.0, 366.0]);
    layer(&mut s, "M-DUCT-SUPP");
    s.execute("pline", &json!({"vertices": [[24, 60], [264, 60]]})).unwrap();
    s.execute("text", &json!({"at": [180.0, 70.0], "text": "24x12 BOT @ 10'-0\"", "height": 4})).unwrap();
    layer(&mut s, "P-DOMW-CW");
    s.execute("line", &json!({"points": [[100, 120], [100, 300]]})).unwrap();
    s.execute("text", &json!({"at": [104.0, 140.0], "text": "2\" CW CL @ 10'-6\"", "height": 4})).unwrap();
    s.execute(
        "bspec.set",
        &json!({"replace": true, "spec": {
            "levels": [{"name": "L1", "elevation_ft": 0, "height_ft": 14, "ceiling_ft": 9, "region": [-100, -100, 1000, 1000], "base": [0, 0]}],
            "layers": [{"layer": "A-WALL", "role": "wall"}]
        }}),
    )
    .unwrap();
    s.execute("mep.read", &json!({"apply": true})).unwrap();
    read_lists(&mut s);
    s.execute("model.build", &json!({"echo": false})).unwrap();
    s
}

fn tags(e: &Entity) -> Vec<String> {
    e.common.xdata.iter().filter(|t| t.code == 1000).map(|t| t.value.clone()).collect()
}

fn tag<'a>(t: &'a [String], key: &str) -> Option<&'a str> {
    t.iter().find_map(|x| x.strip_prefix(key).and_then(|v| v.strip_prefix(':')))
}

/// The built MEP elements whose part and system are these, with their tags.
fn built(s: &Session, part: &str, system: &str) -> Vec<(String, Vec<String>)> {
    s.doc()
        .unwrap()
        .model
        .iter()
        .filter(|e| made_as(e).as_deref() == Some("mep"))
        .map(|e| (e.handle.hex(), tags(e)))
        .filter(|(_, t)| tag(t, "part").is_some_and(|p| p.starts_with(part)) && (system.is_empty() || tag(t, "system") == Some(system)))
        .collect()
}

fn register_at(s: &Session, x: f64) -> Option<String> {
    built(s, "terminal", "M-DUCT-SUPP")
        .into_iter()
        .find(|(_, t)| tag(t, "mepid").is_some_and(|m| m.ends_with(&format!("@{x:.2},84.00"))))
        .map(|(h, _)| h)
}

fn props_of(s: &Session, h: &str) -> props::Props {
    let h = cadaddy_engine::doc::Handle::parse_hex(h).unwrap();
    props::read(&s.doc().unwrap().model.get(h).unwrap().common.xdata)
}

fn text_of(ps: &props::Props, k: &str) -> String {
    ps.get(k).map(props::Prop::text).unwrap_or_default()
}

/// The four edits of the test, each one command.
fn four_edits(s: &mut Session) -> Value {
    let duct = s.execute("mep.list", &json!({"system": "M-DUCT-SUPP"})).unwrap()["items"][0]["id"].as_str().unwrap().to_string();
    let add = s
        .execute(
            "mep.add",
            &json!({"element": "duct", "points": [[0, 0], [264, 180]], "from": {"run": duct}, "reason": "the riser drawn on the detail"}),
        )
        .unwrap();
    assert_eq!(add["status"], "applied", "{add}");
    assert_eq!(add["edit"]["run"]["points"], json!([[264.0, 60.0], [264.0, 180.0]]), "its first point is on the duct's end: {add}");
    assert_eq!(add["edit"]["run"]["system"], "M-DUCT-SUPP", "the system, size and height come from the run it joins");
    assert_eq!(add["edit"]["run"]["size_in"], json!([24.0, 12.0]));
    let reg = register_at(s, 150.0).expect("the register is built");
    let mv = s.execute("mep.move", &json!({"handle": reg, "delta_ft": [2.0, 0.0], "reason": "clear of the light"})).unwrap();
    assert_eq!(mv["element"]["at"], json!([174.0, 84.0]), "{mv}");
    let pipe = built(s, "segment", "P-DOMW-CW").first().map(|(h, _)| h.clone()).expect("the pipe is built");
    s.execute("mep.delete", &json!({"handle": pipe, "reason": "abandoned in place per the demolition note", "by": "agent"})).unwrap();
    s.execute("mep.set", &json!({"tag": "AHU-1", "type_mark": "AHU-TYPE-B", "properties": {"FILTER": "MERV 13"}})).unwrap();
    s.execute("mep.edits", &json!({})).unwrap()
}

/// What a job's next pass does: the plan's runs read again and the schedules' lists written again.
fn read_again_and_rebuild(s: &mut Session) -> Value {
    s.execute("mep.read", &json!({"apply": true})).unwrap();
    read_lists(s);
    s.execute("model.build", &json!({"echo": false})).unwrap()
}

#[test]
fn single_mep_edits_survive_the_lists_read_again_and_a_rebuild_and_the_chain_walks_the_added_segment() {
    let mut s = building();
    let before = built(&s, "segment", "M-DUCT-SUPP").len();
    assert_eq!(before, 1, "one supply duct as drawn");
    let listed = four_edits(&mut s);
    assert_eq!((listed["total"].as_u64(), listed["applied"].as_u64(), listed["orphaned"].as_u64()), (Some(4), Some(4), Some(0)), "{listed}");

    let r = read_again_and_rebuild(&mut s);
    assert_eq!(r["mepEdits"]["applied"], json!(4), "the build applied all four edits: {}", r["mepEdits"]);
    // 1. The added segment: built, inferred, with its basis.
    let added: Vec<(String, Vec<String>)> =
        built(&s, "segment", "M-DUCT-SUPP").into_iter().filter(|(_, t)| tag(t, "mepid").is_some_and(|m| m.starts_with("edit:ME-1"))).collect();
    assert_eq!(added.len(), 1, "the added duct segment is in the rebuilt model");
    let (seg, seg_tags) = &added[0];
    assert_eq!(tag(seg_tags, "status"), Some("inferred"));
    assert!(tag(seg_tags, "basis").is_some_and(|b| b.contains("riser drawn on the detail")), "{seg_tags:?}");
    // 2. The register 2 ft over, the documents' place beside it.
    assert!(register_at(&s, 150.0).is_none(), "the register is not back where the plan had it");
    let moved = register_at(&s, 174.0).expect("the register is 2 ft over");
    let ps = props_of(&s, &moved);
    assert_eq!(text_of(&ps, "position"), "14.50,7.00 ft");
    assert_eq!(text_of(&ps, "design:position"), "12.50,7.00 ft");
    assert!(text_of(&ps, "correction:position").starts_with("mep edit ME-2 (user): clear of the light"), "{ps:?}");
    // 3. The pipe deleted, though the plan still draws it and the re-read put it back in the list.
    assert!(built(&s, "", "P-DOMW-CW").is_empty(), "the deleted pipe stays out");
    let spec = s.execute("bspec.get", &json!({})).unwrap();
    assert!(spec["spec"]["mep"]["runs"].as_array().unwrap().iter().any(|r| r["system"] == "P-DOMW-CW"), "the documents' list keeps it");
    // 4. The unit's type mark, the schedule's mark beside it.
    let ahu = built(&s, "equipment", "").into_iter().find(|(_, t)| tag(t, "tag") == Some("AHU-1")).expect("the unit").0;
    let h = cadaddy_engine::doc::Handle::parse_hex(&ahu).unwrap();
    assert_eq!(bimspec::entity_type_mark(&s.doc().unwrap().model.get(h).unwrap().common.xdata), Some("AHU-TYPE-B"));
    let ps = props_of(&s, &ahu);
    assert_eq!((text_of(&ps, "type_mark"), text_of(&ps, "design:type_mark")), ("AHU-TYPE-B".to_string(), "AHU-1".to_string()));
    assert_eq!((text_of(&ps, "FILTER"), text_of(&ps, "design:FILTER")), ("MERV 13".to_string(), "(none)".to_string()));
    // bim.chain: from the register by the added segment, through it, to the unit.
    let far = built(&s, "terminal", "M-DUCT-SUPP")
        .into_iter()
        .find(|(_, t)| tag(t, "mepid").is_some_and(|m| m.ends_with("@276.00,170.00")))
        .expect("the register by the corner")
        .0;
    let chain = s.execute("bim.chain", &json!({"handle": far})).unwrap();
    assert_eq!(chain["source"]["tag"], "AHU-1", "{chain}");
    let path: Vec<&str> = chain["path"].as_array().unwrap().iter().filter_map(|n| n["handle"].as_str()).collect();
    assert!(path.contains(&seg.as_str()), "the walk goes through the added segment: {chain}");
    assert!(path.len() >= 4, "register, added segment, its elbow, the drawn duct, the unit: {chain}");
    // And the moved register is fed by the drawn duct.
    assert_eq!(s.execute("bim.chain", &json!({"handle": moved})).unwrap()["source"]["tag"], "AHU-1");
}

#[test]
fn an_edit_whose_element_the_documents_drop_is_listed_orphaned_and_revert_puts_an_element_back() {
    let mut s = building();
    let pipe = s.execute("mep.list", &json!({"system": "P-DOMW-CW"})).unwrap()["items"][0]["id"].as_str().unwrap().to_string();
    s.execute("mep.delete", &json!({"run": pipe})).unwrap();
    let reg = register_at(&s, 150.0).unwrap();
    s.execute("mep.resize", &json!({"handle": reg, "size_in": [10, 10]})).unwrap();
    // The register leaves the documents (a revision takes it out): its edit is kept, orphaned.
    s.execute(
        "bspec.set",
        &json!({"spec": {"mep": {"terminals": [{"system": "M-DUCT-SUPP", "level": "L1", "at": [276.0, 170.0], "size_in": [12.0, 12.0], "bottom_ft": 9.0, "terminal_kind": "register"}]}}}),
    )
    .unwrap();
    let r = s.execute("model.build", &json!({"echo": false})).unwrap();
    assert_eq!((r["mepEdits"]["applied"].as_u64(), r["mepEdits"]["orphaned"].as_u64()), (Some(1), Some(1)), "{}", r["mepEdits"]);
    let orphaned = s.execute("mep.edits", &json!({"status": "orphaned"})).unwrap();
    assert_eq!(orphaned["edits"][0]["op"], "resize", "{orphaned}");
    assert!(orphaned["edits"][0]["why"].as_str().unwrap().contains("any more"));
    // Revert the delete: the pipe is built again.
    assert!(built(&s, "", "P-DOMW-CW").is_empty());
    let back = s.execute("mep.edits", &json!({"revert": "ME-1"})).unwrap();
    assert_eq!((back["reverted"]["op"].as_str(), back["rebuilt"].as_bool()), (Some("delete"), Some(true)), "{back}");
    assert!(!built(&s, "", "P-DOMW-CW").is_empty(), "the pipe is as the documents give it again");
}

#[test]
fn single_edits_refuse_bad_input_and_add_equipment_and_sprinklers_with_their_provenance() {
    let mut s = building();
    for (cmd, p) in [
        ("mep.add", json!({"element": "duct", "level": "L1", "system": "NOPE", "points": [[0, 0], [10, 0]]})),
        ("mep.add", json!({"element": "duct", "level": "L1", "system": "M-DUCT-SUPP", "points": [[0, 0], [1e300, 0]]})),
        ("mep.add", json!({"element": "duct", "level": "L1", "system": "M-DUCT-SUPP", "points": [[0, 0], [10, 0]], "size_in": [1e9, 1]})),
        ("mep.add", json!({"element": "pipe", "level": "L1", "system": "M-DUCT-SUPP", "points": [[0, 0]]})),
        ("mep.add", json!({"element": "equipment", "level": "L1", "tag": "AHU-1", "kind": "ahu", "at": [0, 0], "size_ft": [2, 2], "height_ft": 2})),
        (
            "mep.add",
            json!({"element": "equipment", "level": "Nowhere", "tag": "X-1", "kind": "fan", "at": [0, 0], "size_ft": [2, 2], "height_ft": 2}),
        ),
        ("mep.move", json!({"tag": "AHU-1"})),
        ("mep.move", json!({"tag": "AHU-1", "bottom_ft": 1e9})),
        ("mep.move", json!({"tag": "NOT-THERE", "delta_ft": [1, 0]})),
        ("mep.set", json!({"run": "R1", "type_mark": "X"})),
        ("mep.set", json!({"tag": "AHU-1", "properties": {"A=B": "x"}})),
        ("mep.resize", json!({"tag": "AHU-1", "size_ft": [-1, 2]})),
        ("mep.edits", json!({"revert": "ME-99"})),
        ("mep.delete", json!({})),
    ] {
        assert!(s.execute(cmd, &p).is_err(), "{cmd} {p} must be refused");
    }
    assert_eq!(s.execute("mep.edits", &json!({})).unwrap()["total"], json!(0), "a refused edit is not kept");
    // A fan coil and a sprinkler head no plan draws, inferred with their basis.
    let fcu = s
        .execute(
            "mep.add",
            &json!({"element": "equipment", "level": "L1", "tag": "FCU-1", "kind": "fan_coil", "at": [400, 300], "size_ft": [4, 2], "height_ft": 1.5, "bottom_ft": 9.5, "type_mark": "FC-A", "reason": "the unit the schedule lists and no plan shows"}),
        )
        .unwrap();
    assert_eq!(fcu["element"]["min"], json!([376.0, 288.0]), "{fcu}");
    s.execute("mep.add", &json!({"element": "sprinkler", "level": "L1", "system": "M-DUCT-SUPP", "at": [300, 300]})).unwrap();
    let unit = built(&s, "equipment", "").into_iter().find(|(_, t)| tag(t, "tag") == Some("FCU-1")).expect("the fan coil is built");
    assert_eq!(tag(&unit.1, "status"), Some("inferred"));
    assert!(tag(&unit.1, "basis").is_some_and(|b| b.contains("no plan shows")), "{:?}", unit.1);
    let head = built(&s, "terminal:sprinkler_pendent", "").into_iter().next().expect("the head is built");
    assert_eq!(tag(&head.1, "status"), Some("inferred"));
    // Without a rebuild the edit is kept and waits for the next build.
    let r = s.execute("mep.move", &json!({"edit": "ME-1", "delta_ft": [1, 0], "rebuild": false})).unwrap();
    assert_eq!(r["rebuilt"], json!(false));
    assert_eq!(r["element"]["min"], json!([388.0, 288.0]));
}

#[test]
fn every_parameter_the_commands_take_is_one_their_docs_name() {
    let target = ["handle", "run", "tag", "edit", "element", "level", "at", "system", "reason", "by", "rebuild"];
    let own: [(&str, &[&str]); 6] = [
        (
            "mep.add",
            &[
                "element",
                "level",
                "system",
                "points",
                "size_in",
                "bottom_ft",
                "top_ft",
                "from",
                "to",
                "at",
                "kind",
                "tag",
                "size_ft",
                "height_ft",
                "systems",
                "type_mark",
                "properties",
                "made",
                "basis",
                "source_sheet",
                "reason",
                "by",
                "rebuild",
            ],
        ),
        ("mep.move", &["delta_ft", "to", "bottom_ft"]),
        ("mep.resize", &["size_in", "size_ft", "height_ft"]),
        ("mep.delete", &[]),
        ("mep.set", &["type_mark", "new_system", "kind", "properties"]),
        ("mep.edits", &["element", "status", "revert", "rebuild", "cursor", "limit"]),
    ];
    let specs = cadaddy_engine::command_specs();
    for (id, names) in own {
        let spec = specs.iter().find(|c| c.id == id).unwrap_or_else(|| panic!("{id} is a command"));
        let all: Vec<&str> = if id == "mep.add" || id == "mep.edits" { names.to_vec() } else { target.iter().chain(names.iter()).copied().collect() };
        let p: serde_json::Map<String, Value> = all.iter().map(|k| (k.to_string(), json!(1))).collect();
        assert_eq!(cadaddy_engine::unknown_params(spec, &Value::Object(p)), Vec::<String>::new(), "{id}");
        assert_eq!(cadaddy_engine::unknown_params(spec, &json!({"zzz_not_one": 1})), vec!["zzz_not_one".to_string()], "{id} refuses others");
    }
}
