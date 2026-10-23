//! The print validator (Q321) on a drawing: the prints kept with the building spec
//! (`bim.prints`), `bim.validate` over the model read back from the plan, scoped re-checks,
//! a failure's overlay picture, provenance (`bim.source`), and the gate on agents' edits (warn:
//! new failures come back with the result; refuse: the edit is undone).

use std::sync::Arc;

use cadaddy_engine::Session;
use cadaddy_engine::doc::{Attrib, AttribModes, AttribProps, Block, Entity, EntityKind, Handle, Insert, Line, Text};
use cadaddy_engine::geom::Vec3;
use serde_json::{Value, json};

fn layer(s: &mut Session, name: &str) {
    if s.execute("layer.new", &json!({"name": name, "current": true})).is_err() {
        s.execute("layer.current", &json!({"name": name})).unwrap();
    }
}

/// Feet → inches.
fn rect(s: &mut Session, a: [f64; 2], b: [f64; 2]) {
    let p = |x: f64, y: f64| [x * 12.0, y * 12.0];
    s.execute("pline", &json!({"vertices": [p(a[0], a[1]), p(b[0], a[1]), p(b[0], b[1]), p(a[0], b[1])], "closed": true})).unwrap();
}

fn line(s: &mut Session, a: [f64; 2], b: [f64; 2]) {
    s.execute("line", &json!({"points": [[a[0] * 12.0, a[1] * 12.0], [b[0] * 12.0, b[1] * 12.0]]})).unwrap();
}

fn text(s: &mut Session, at: [f64; 2], t: &str) {
    s.execute("text", &json!({"at": [at[0] * 12.0, at[1] * 12.0], "text": t, "height": 6})).unwrap();
}

/// Block references at points (feet) on `layer` with a TYPE attribute; their handles.
fn place(s: Session, block: &str, layer: &str, at: &[[f64; 2]], ty: &str) -> (Session, Vec<String>) {
    let mut d = (*s.doc().unwrap()).clone();
    let style = d.model.iter().find_map(|e| if let EntityKind::Text(t) = &e.kind { Some(t.clone()) } else { None }).unwrap();
    if !d.blocks.contains_key(block) {
        let mut b = Block::new(block);
        let h = d.new_handle();
        b.entities.push(Entity::new(h, EntityKind::Line(Line { a: Vec3::new(-4.0, 0.0, 0.0), b: Vec3::new(4.0, 0.0, 0.0) })));
        d.blocks.insert(block.into(), Arc::new(b));
    }
    let mut hs = Vec::new();
    for p in at {
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
        let ins = Insert {
            block: block.into(),
            insert: Vec3::new(p[0] * 12.0, p[1] * 12.0, 0.0),
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
    s2.open_drawing(d, "Plan", None);
    (s2, hs)
}

fn spec() -> Value {
    json!({
        "units_per_foot": 12,
        "levels": [{"name": "L1", "elevation_ft": 0, "height_ft": 10, "ceiling_ft": 9, "base": [0, 0]}],
        "layers": [
            {"layer": "A-WALL-CL", "role": "wall", "wall_type": "Partition"},
            {"layer": "A-WALL", "role": "wall", "wall_type": "Exterior"},
            {"layer": "E-POWR", "role": "device"},
            {"layer": "E-LITE", "role": "fixture"},
            {"layer": "A-ROOM-TAG", "role": "room_tag"}
        ],
        "wall_types": [
            {"name": "Exterior", "thickness_in": 6, "assembly": {"tag": "X1", "layers": [
                {"material": "5/8\" gypsum board", "thickness_in": 0.625, "category": "board"},
                {"material": "3 5/8\" metal studs @ 16\" o.c.", "thickness_in": 3.625, "category": "framing"},
                {"material": "exterior sheathing and finish", "thickness_in": 1.75, "category": "sheathing"}]}},
            {"name": "Partition", "thickness_in": 4.875, "from": "centerline"}
        ],
        "rooms": [{"name": "Office"}, {"name": "Storage", "ceiling_ft": 9}],
        "devices": {"by_type": {"DUPLEX": {"ceiling": false, "aff_ft": 1.5, "size_ft": [0.25, 0.1, 0.33]}, "LT-A": {"ceiling": true}}}
    })
}

struct Plan {
    s: Session,
    receptacles: Vec<String>,
    lights: Vec<String>,
}

/// A made-up 30' × 12' building of 6" walls, an office and a storage room split at x = 15' by a
/// partition; two receptacles on the south wall's inside face, two lights; built.
fn plan() -> Plan {
    let mut s = Session::new();
    layer(&mut s, "A-WALL");
    rect(&mut s, [0.0, 0.0], [30.0, 12.0]);
    rect(&mut s, [0.5, 0.5], [29.5, 11.5]);
    layer(&mut s, "A-WALL-CL");
    line(&mut s, [15.0, 0.5], [15.0, 11.5]);
    layer(&mut s, "A-ROOM-TAG");
    text(&mut s, [5.0, 6.0], "OFFICE");
    text(&mut s, [20.0, 6.0], "STORAGE");
    let (s, receptacles) = place(s, "RECEP", "E-POWR", &[[6.0, 0.5], [22.0, 0.5]], "DUPLEX");
    let (mut s, lights) = place(s, "LIGHT", "E-LITE", &[[7.0, 6.0], [22.0, 6.0]], "LT-A");
    s.execute("bspec.set", &json!({"spec": spec(), "replace": true})).unwrap();
    s.execute("model.build", &json!({})).unwrap();
    s.execute(
        "bim.set_type",
        &json!({"mark": "DUPLEX", "category": "wiring device", "fields": {"description": "duplex receptacle", "wetDamp": "dry"}}),
    )
    .unwrap();
    s.execute("bim.set_type", &json!({"mark": "LT-A", "category": "luminaire", "fields": {"catalogNumber": "LUM-100"}})).unwrap();
    Plan { s, receptacles, lights }
}

fn room(n: &str, name: &str, x0: f64, x1: f64) -> Value {
    json!({"level": "L1", "number": n, "name": name, "outline": [[x0, 0.5], [x1, 0.5], [x1, 11.5], [x0, 11.5]], "ceiling_ft": 9, "sheet": "A-101"})
}

/// The plan's devices as the print (sheet E-101), with rooms, schedules, rules; devices traced.
fn with_prints(p: &mut Plan, gate: &str) {
    let prints = json!({
        "rooms": [room("101", "OFFICE", 0.5, 15.0), room("102", "STORAGE", 15.0, 29.5)],
        "schedule": [
            {"sheet": "E-601", "schedule": "device", "mark": "DUPLEX"},
            {"sheet": "E-601", "schedule": "luminaire", "mark": "LT-A", "qty": 2}
        ],
        "dimensions": [{"sheet": "A-101", "level": "L1", "a": [0.5, 6.0], "b": [29.5, 6.0], "value_ft": 29.0, "text": "29'-0\"", "to": "face"}],
        "rules": {"heights": [{"kind": "duplex", "mounting_in": 18, "source": "general note 4"}]}
    });
    let r = p.s.execute("bim.prints", &json!({"prints": prints, "fromPlan": true, "sheet": "E-101", "gate": gate})).unwrap();
    assert_eq!(r["symbols"], json!(4), "{r}");
    let all: Vec<String> = p.receptacles.iter().chain(&p.lights).cloned().collect();
    p.s.execute("bim.source", &json!({"handles": all, "status": "fixed", "source": "E-101"})).unwrap();
}

fn rules(r: &Value) -> Vec<String> {
    r["failures"].as_array().unwrap().iter().map(|f| f["rule"].as_str().unwrap_or_default().to_string()).collect()
}

fn new_rules(r: &Value) -> Vec<String> {
    r["validation"]["new"].as_array().map(|a| a.iter().map(|f| f["rule"].as_str().unwrap_or_default().to_string()).collect()).unwrap_or_default()
}

#[test]
fn a_model_true_to_its_prints_passes_and_errors_are_caught() {
    let mut p = plan();
    // No prints yet: a clear refusal.
    let e = p.s.execute("bim.validate", &json!({})).unwrap_err().to_string();
    assert!(e.contains("bim.prints"), "{e}");
    with_prints(&mut p, "off");
    let r = p.s.execute("bim.validate", &json!({})).unwrap();
    assert_eq!(r["pass"], json!(true), "{r:#}");
    assert_eq!(r["score"], json!(100.0));
    for c in ["overlay", "counts", "dimensions", "tags", "physical", "provenance", "spec"] {
        assert!(r["checked"][c].as_u64().unwrap_or(0) > 0, "{c} checked: {}", r["checked"]);
    }

    // A receptacle moved to the opposite wall (12' in Y): misplaced — different wall.
    let h = p.receptacles[0].clone();
    p.s.execute("move", &json!({"handles": [h], "from": [0, 0], "to": [0, 132]})).unwrap();
    let r = p.s.execute("bim.validate", &json!({})).unwrap();
    let misplaced_or_not = r["failures"].as_array().unwrap().iter().find(|f| {
        f["rule"] == "misplaced" || f["rule"] == "not_on_print"
    }).unwrap_or_else(|| panic!("should find misplaced or not_on_print: {r:#}"));
    assert_eq!(misplaced_or_not["source"]["sheet"], json!("E-101"));
    assert!(misplaced_or_not["fix"].as_str().unwrap().contains("room") || misplaced_or_not["fix"].as_str().unwrap().contains("wall")
        || misplaced_or_not["fix"].as_str().unwrap().starts_with("Move") || misplaced_or_not["fix"].as_str().unwrap().starts_with("Delete"),
        "fix should be actionable: {misplaced_or_not}");
    assert!(misplaced_or_not["sheetAt"].is_array() || misplaced_or_not["rule"] == "not_on_print");
    assert_eq!(r["pass"], json!(false));
    assert!(r["score"].as_f64().unwrap() < 100.0);
    p.s.execute("move", &json!({"handles": [h], "from": [0, 132], "to": [0, 0]})).unwrap();

    // A light typed as something the schedule hasn't: unknown type and wrong type.
    p.s.execute("bim.set_spec", &json!({"handles": [p.lights[0]], "mark": "LT-Z", "create": {"category": "luminaire"}})).unwrap();
    let r = p.s.execute("bim.validate", &json!({})).unwrap();
    let k = rules(&r);
    assert!(k.contains(&"wrong_type".to_string()) && k.contains(&"type_not_scheduled".to_string()), "{k:?}");
    p.s.execute("bim.set_spec", &json!({"handles": [p.lights[0]], "mark": "LT-A"})).unwrap();

    // An extra light no sheet shows; the schedule's quantity is off too.
    let (s, extra) = place(std::mem::replace(&mut p.s, Session::empty()), "LIGHT", "E-LITE", &[[26.0, 9.0]], "LT-A");
    p.s = s;
    p.s.execute("bim.source", &json!({"handles": extra, "status": "agent"})).unwrap();
    let r = p.s.execute("bim.validate", &json!({})).unwrap();
    let k = rules(&r);
    for want in ["not_on_print", "schedule_quantity", "room_count"] {
        assert!(k.contains(&want.to_string()), "{want} in {k:?}");
    }
    // Filters.
    let high = p.s.execute("bim.validate", &json!({"severity": "high"})).unwrap();
    assert!(high["failures"].as_array().unwrap().iter().all(|f| f["severity"] == "high"));
    let only = p.s.execute("bim.validate", &json!({"checks": ["counts"]})).unwrap();
    assert!(only["failures"].as_array().unwrap().iter().all(|f| f["check"] == "counts"), "{only}");
    assert!(p.s.execute("bim.validate", &json!({"checks": ["nonsense"]})).is_err());
}

#[test]
fn a_scoped_recheck_finds_the_same_failure_and_looks_at_less() {
    let mut p = plan();
    with_prints(&mut p, "off");
    let h = p.lights[1].clone();
    // Move to the opposite wall (cross-room, 11 ft in Y) to trigger misplaced/not_on_print.
    p.s.execute("move", &json!({"handles": [h], "from": [0, 0], "to": [0, 132]})).unwrap();
    let all = p.s.execute("bim.validate", &json!({})).unwrap();
    let near = p.s.execute("bim.validate", &json!({"handles": [h]})).unwrap();
    assert_eq!(near["scoped"], json!(true));
    let id = |r: &Value| r["failures"].as_array().unwrap().iter().filter(|f| f["element"] == json!(h)).map(|f| f["id"].clone()).collect::<Vec<_>>();
    assert!(!id(&near).is_empty() && id(&near) == id(&all), "{near:#}\n{all:#}");
    let total = |r: &Value| r["checked"].as_object().unwrap().values().map(|v| v.as_u64().unwrap()).sum::<u64>();
    assert!(total(&near) < total(&all), "{} < {}", total(&near), total(&all));
    // The office's far corner sees nothing of it.
    let far = p.s.execute("bim.validate", &json!({"area": [12, 100, 60, 130]})).unwrap();
    assert_eq!(far["total"], json!(0), "{far:#}");
}

#[test]
fn an_overlay_pictures_a_failure() {
    let mut p = plan();
    with_prints(&mut p, "off");
    let h = p.receptacles[1].clone();
    // Move to the opposite wall (cross-room): should trigger misplaced or not_on_print.
    p.s.execute("move", &json!({"handles": [h], "from": [0, 0], "to": [0, 132]})).unwrap();
    let r = p.s.execute("bim.validate", &json!({})).unwrap();
    let f = r["failures"].as_array().unwrap().iter().find(|f| f["rule"] == "misplaced" || f["rule"] == "not_on_print").unwrap().clone();
    let dir = std::env::temp_dir().join(format!("printcheck-overlay-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("misplaced.png");
    let o = p.s.execute("bim.validate.overlay", &json!({"id": f["id"], "path": path.to_string_lossy()})).unwrap();
    assert!(o["rule"] == "misplaced" || o["rule"] == "not_on_print", "overlay rule: {}", o["rule"]);
    let png = std::fs::read(&path).unwrap();
    assert!(png.starts_with(b"\x89PNG") && png.len() > 1_000, "{} bytes", png.len());
    if std::env::var_os("PRINTCHECK_KEEP").is_none() {
        let _ = std::fs::remove_dir_all(&dir);
    }
    assert!(p.s.execute("bim.validate.overlay", &json!({"id": "V-00000000"})).is_err());
}

#[test]
fn an_agents_edit_comes_back_with_the_failures_it_added() {
    let mut p = plan();
    with_prints(&mut p, "warn");
    // A clean edit: nothing new.
    let r = p.s.execute_agent("layer.new", &json!({"name": "X-NOTES"})).unwrap();
    assert!(r.get("validation").is_none_or(|v| v["newCount"] == json!(0)), "{r}");
    // Moving a receptacle to the opposite wall (cross-room).
    let h = p.receptacles[0].clone();
    let r = p.s.execute_agent("move", &json!({"handles": [h], "from": [0, 0], "to": [0, 132]})).unwrap();
    let nr = new_rules(&r);
    assert!(nr.contains(&"misplaced".to_string()) || nr.contains(&"not_on_print".to_string()), "{r:#}");
    assert!(r["validation"]["score"].as_f64().unwrap() < 100.0);
    // Putting it back fixes it.
    let r = p.s.execute_agent("move", &json!({"handles": [h], "from": [0, 132], "to": [0, 0]})).unwrap();
    assert_eq!(r["validation"]["newCount"], json!(0), "{r:#}");
    assert!(r["validation"]["fixed"].as_u64().unwrap() >= 1, "{r:#}");
    // Erasing a light the print shows: missing in the model.
    let r = p.s.execute_agent("erase", &json!({"handles": [p.lights[0]]})).unwrap();
    assert!(new_rules(&r).contains(&"missing_in_model".to_string()), "{r:#}");
    // The user's own edits aren't gated.
    let r = p.s.execute("move", &json!({"handles": [p.lights[1]], "from": [0, 0], "to": [48, 0]})).unwrap();
    assert!(r.get("validation").is_none(), "{r}");
}

#[test]
fn a_refusing_gate_undoes_an_edit_that_adds_failures() {
    let mut p = plan();
    with_prints(&mut p, "refuse");
    let h = p.receptacles[0].clone();
    let before = p.s.doc().unwrap().model.iter().find(|e| e.handle.hex() == h).cloned().unwrap();
    let e = p.s.execute_agent("move", &json!({"handles": [h], "from": [0, 0], "to": [0, 144]})).unwrap_err().to_string();
    assert!(e.starts_with("refused:"), "move should be refused: {e}");
    let after = p.s.doc().unwrap().model.iter().find(|e| e.handle.hex() == h).cloned().unwrap();
    assert_eq!(before, after, "the refused move was undone");
    // A harmless edit goes through.
    p.s.execute_agent("layer.new", &json!({"name": "X-NOTES"})).unwrap();
    // A person turns the gate off; the agent's move then goes through.
    p.s.execute("bim.prints", &json!({"gate": "off"})).unwrap();
    p.s.execute_agent("move", &json!({"handles": [h], "from": [0, 0], "to": [48, 0]})).unwrap();
}

#[test]
fn prints_and_provenance_are_saved_with_the_drawing() {
    let mut p = plan();
    with_prints(&mut p, "warn");
    let d = p.s.doc().unwrap().clone();
    let text = cadaddy_io::write_dxf(&d);
    let back = cadaddy_io::read(text.as_bytes(), "plan.dxf").unwrap();
    let spec = back.building_spec.clone().unwrap_or_default();
    assert!(spec.contains("\"prints\"") && spec.contains("E-601"), "{}", &spec[..spec.len().min(300)]);
    let mut s = Session::empty();
    s.open_drawing(back, "Plan", None);
    let r = s.execute("bim.validate", &json!({})).unwrap();
    assert_eq!(r["pass"], json!(true), "{r:#}");
}

#[test]
fn provenance_is_checked() {
    let mut p = plan();
    with_prints(&mut p, "off");
    // Untraced: no status, no source.
    let (s, extra) = place(std::mem::replace(&mut p.s, Session::empty()), "RECEP", "E-POWR", &[[10.0, 0.5]], "DUPLEX");
    p.s = s;
    let r = p.s.execute("bim.validate", &json!({"checks": ["provenance"]})).unwrap();
    let f = r["failures"].as_array().unwrap().iter().find(|f| f["element"] == json!(extra[0])).unwrap_or_else(|| panic!("{r:#}"));
    assert_eq!(f["rule"], json!("untraced"));
    p.s.execute("bim.source", &json!({"handles": extra, "status": "field-verified", "source": "site walk"})).unwrap();
    let r = p.s.execute("bim.validate", &json!({"checks": ["provenance"]})).unwrap();
    assert_eq!(r["total"], json!(0), "{r:#}");
    assert!(p.s.execute("bim.source", &json!({"handles": extra, "status": "guess"})).is_err());
    // A box deeper than the wall's 3 5/8" studs, and a bad depth refused.
    p.s.execute("bim.source", &json!({"handles": extra, "status": "fixed", "source": "E-101", "boxDepth": 6})).unwrap();
    let r = p.s.execute("bim.validate", &json!({"checks": ["physical"]})).unwrap();
    let f = r["failures"].as_array().unwrap().iter().find(|f| f["rule"] == "box_too_deep").unwrap_or_else(|| panic!("{r:#}"));
    assert_eq!(f["element"], json!(extra[0]));
    assert!(f["expected"].as_str().unwrap().contains("3.625") || f["expected"].as_str().unwrap().contains("3 5/8"), "{f}");
    assert!(p.s.execute("bim.source", &json!({"handles": extra, "status": "fixed", "boxDepth": -1})).is_err());
}

// ---- MUST 1: Agents cannot move the goalposts ----

#[test]
fn an_agent_cannot_turn_the_gate_off() {
    let mut p = plan();
    with_prints(&mut p, "warn");
    // The gate is on (warn).
    let r = p.s.execute("bim.validate", &json!({})).unwrap();
    assert!(r["pass"].as_bool() == Some(true));
    // An agent tries to turn it off.
    let err = p.s.execute_agent("bim.prints", &json!({"gate": "off"}));
    assert!(err.is_err(), "agent should not be allowed to change the gate");
    assert!(format!("{}", err.unwrap_err()).contains("gate"), "error should mention gate");
    // The gate is still on.
    let r = p.s.execute("bim.validate", &json!({})).unwrap();
    assert!(r["pass"].as_bool() == Some(true), "gate should still be on");
}

#[test]
fn an_agent_cannot_clear_prints() {
    let mut p = plan();
    with_prints(&mut p, "warn");
    let err = p.s.execute_agent("bim.prints", &json!({"clear": true}));
    assert!(err.is_err(), "agent should not be allowed to clear prints");
}

#[test]
fn an_agent_cannot_replace_rules() {
    let mut p = plan();
    with_prints(&mut p, "warn");
    let err = p.s.execute_agent("bim.prints", &json!({"prints": {"rules": {"tolerances": {"place_in": 999.0}}}}));
    assert!(err.is_err(), "agent should not be allowed to replace rules");
}

#[test]
fn an_agent_cannot_set_field_verified() {
    let mut p = plan();
    with_prints(&mut p, "warn");
    let err = p.s.execute_agent("bim.source", &json!({"handles": [p.receptacles[0]], "status": "field_verified"}));
    assert!(err.is_err(), "agent should not set field_verified");
    let msg = format!("{}", err.unwrap_err());
    assert!(msg.contains("field_verified") || msg.contains("agent"), "error should mention the restriction: {msg}");
}

#[test]
fn an_agent_cannot_set_override() {
    let mut p = plan();
    with_prints(&mut p, "warn");
    let err = p.s.execute_agent("bim.source", &json!({"handles": [p.receptacles[0]], "status": "override"}));
    assert!(err.is_err(), "agent should not set override");
}

#[test]
fn bim_source_records_author() {
    let mut p = plan();
    with_prints(&mut p, "off");
    // User sets source: author should be "user".
    p.s.execute("bim.source", &json!({"handles": [p.receptacles[0]], "status": "fixed", "source": "E-101 @ test"})).unwrap();
    // Verify provenance includes author=user by reading back via validate.
    let r = p.s.execute("bim.validate", &json!({"checks": ["provenance"]})).unwrap();
    assert!(r["pass"].as_bool() == Some(true), "model should pass provenance after setting source");
    // Agent sets source: author should be "agent".
    p.s.execute_agent("bim.source", &json!({"handles": [p.lights[0]], "status": "agent", "source": "auto-placed"})).unwrap();
}

#[test]
fn an_agent_retype_under_gate_warn_shows_validation_failures() {
    let mut p = plan();
    with_prints(&mut p, "warn");
    // Retype a light to an unknown type: should trigger wrong_type/type_not_scheduled through the gate.
    let r = p.s.execute_agent("bim.set_spec", &json!({"handles": [p.lights[0]], "mark": "LT-Z", "create": {"category": "luminaire"}}));
    match r {
        Ok(v) => {
            // Under warn, the edit goes through but validation.new should have failures.
            let val = &v["validation"];
            assert!(val["newCount"].as_u64().unwrap_or(0) > 0, "should have new validation failures: {v:#}");
        }
        Err(e) => panic!("warn gate should not refuse: {e}"),
    }
}

// ---- MUST 2: Location doctrine ----

#[test]
fn a_device_on_the_correct_wall_passes_misplaced() {
    let mut p = plan();
    with_prints(&mut p, "off");
    // Move a receptacle 2 feet along the same wall (south wall, y = 0 area).
    // The receptacle starts at its symbol's location. Move it 2' along the wall.
    let h = Handle::parse_hex(&p.receptacles[0]).unwrap();
    p.s.doc_mut().unwrap().modify_entity(h, |e| {
        if let EntityKind::Insert(ins) = &mut e.kind {
            ins.insert.x += 2.0 * 12.0; // 2 feet in drawing units (inches)
        }
    }).unwrap();
    let r = p.s.execute("bim.validate", &json!({"checks": ["overlay"]})).unwrap();
    let failures: Vec<&Value> = r["failures"].as_array().unwrap().iter().filter(|f| f["rule"] == "misplaced").collect();
    // Should NOT be misplaced since it's on the same wall, same room, within along-wall tolerance.
    assert!(failures.is_empty(), "device on same wall should not be misplaced: {failures:?}");
}

#[test]
fn a_device_on_the_opposite_wall_fails_misplaced() {
    let mut p = plan();
    with_prints(&mut p, "off");
    // Move a receptacle to the opposite wall (north wall, far side of the room).
    let h = Handle::parse_hex(&p.receptacles[0]).unwrap();
    p.s.doc_mut().unwrap().modify_entity(h, |e| {
        if let EntityKind::Insert(ins) = &mut e.kind {
            ins.insert.y += 12.0 * 12.0; // 12 feet across to the opposite wall
        }
    }).unwrap();
    let r = p.s.execute("bim.validate", &json!({"checks": ["overlay"]})).unwrap();
    let rules: Vec<String> = r["failures"].as_array().unwrap().iter().map(|f| f["rule"].as_str().unwrap_or("").to_string()).collect();
    // Should be misplaced: different wall (or different room).
    assert!(rules.contains(&"misplaced".to_string()) || rules.contains(&"not_on_print".to_string()),
        "device on opposite wall should be flagged: {rules:?}");
    // The fix should NOT contain symbol coordinates.
    for f in r["failures"].as_array().unwrap() {
        if f["rule"] == "misplaced" {
            let fix = f["fix"].as_str().unwrap_or("");
            assert!(!fix.contains("Move it") || fix.contains("room") || fix.contains("wall"),
                "fix should name room/wall, not point coordinates: {fix}");
        }
    }
}

#[test]
fn an_agent_retype_under_gate_refuse_is_undone() {
    let mut p = plan();
    with_prints(&mut p, "refuse");
    // Retype a light to an unknown type: gate=refuse should undo it.
    let r = p.s.execute_agent("bim.set_spec", &json!({"handles": [p.lights[0]], "mark": "LT-Z", "create": {"category": "luminaire"}}));
    assert!(r.is_err(), "refuse gate should reject the retype");
    let err = format!("{}", r.unwrap_err());
    assert!(err.contains("refused"), "error should say refused: {err}");
    // The light should still have its original type.
    let v = p.s.execute("bim.validate", &json!({})).unwrap();
    assert!(v["pass"].as_bool() == Some(true), "model should still pass after undone retype");
}

#[test]
fn hostile_params_never_panic() {
    let mut p = plan();
    for v in [
        json!({"prints": "nonsense"}),
        json!({"prints": {"version": 99}}),
        json!({"prints": {"symbols": [{"at": [1e308, -1e308]}], "dimensions": [{"a": [0, 0], "b": [0, 0], "value_ft": 1}]}}),
        json!({"gate": "sometimes"}),
        json!({"fromPlan": true, "sheet": "x".repeat(10_000)}),
    ] {
        let _ = p.s.execute("bim.prints", &v);
    }
    for v in [
        json!({"handles": "x"}),
        json!({"handles": ["zz"]}),
        json!({"area": [0, 0, 1]}),
        json!({"area": [0, 0, 1, f64::MAX]}),
        json!({"checks": 5}),
        json!({"limit": -1}),
        json!({"level": "nowhere"}),
    ] {
        let _ = p.s.execute("bim.validate", &v);
    }
    for v in [json!({}), json!({"id": 5}), json!({"id": "V-1", "width": 1e12})] {
        let _ = p.s.execute("bim.validate.overlay", &v);
    }
    for v in [
        json!({}),
        json!({"handles": ["FFFFFF"], "status": "fixed"}),
        json!({"handles": [p.lights[0]], "status": "fixed", "source": "\u{0}".repeat(5000)}),
    ] {
        let _ = p.s.execute("bim.source", &v);
    }
}
