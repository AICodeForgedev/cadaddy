use super::*;
use cadaddy_doc::{Circle, EntityKind, Handle, RawTag};
use cadaddy_geom::Vec3;

/// A model element the way the build tags one (`part`, level, source), at (x, y) feet, with
/// properties `props` (key, value, source kind, doc).
fn element(d: &mut Drawing, part: &str, at: (f64, f64), props: &[(&str, Value, SourceKind, &str)]) -> Handle {
    let h = Handle(d.handseed.max(0x200));
    d.handseed = h.0 + 1;
    let mut x = vec![
        RawTag { code: 1001, value: super::super::wallgen::APP.into() },
        RawTag { code: 1000, value: part.into() },
        RawTag { code: 1000, value: "Level 1".into() },
        RawTag { code: 1000, value: "0".into() },
    ];
    let mut store = Props::default();
    for (k, v, sk, doc) in props {
        let src = if doc.is_empty() { Source::of(*sk) } else { Source::from_doc(*sk, doc) };
        store.set(k, Prop::new(v.clone(), src));
    }
    x = props::write(&x, &store);
    let mut e = Entity::new(h, EntityKind::Circle(Circle { center: Vec3::new(at.0 * 12.0, at.1 * 12.0, 18.0), radius: 2.0 }));
    e.common.xdata = x.into();
    e.common.layer = "E-POWER".into();
    d.model.push(e);
    h
}

fn receptacle(d: &mut Drawing, at: (f64, f64), extra: &[(&str, Value, SourceKind, &str)]) -> Handle {
    let mut p: Vec<(&str, Value, SourceKind, &str)> = vec![
        ("kind", json!("receptacle"), SourceKind::Print, "Sheet P1"),
        ("type_mark", json!("R1"), SourceKind::Print, "Sheet P1"),
        ("circuit", json!("12"), SourceKind::Print, "Sheet P1"),
        ("panel", json!("PA"), SourceKind::Print, "Sheet P1"),
        ("mount_height", json!(18), SourceKind::Default, ""),
        ("room", json!("Office 1"), SourceKind::Computed, ""),
    ];
    p.extend(extra.iter().cloned());
    element(d, "device", at, &p)
}

fn session(d: Drawing) -> Session {
    let mut s = Session::empty();
    s.open_drawing(d, "Sample Floor Plan", None);
    s
}

fn field<'a>(form: &'a Value, key: &str) -> &'a Value {
    form["fields"].as_array().and_then(|a| a.iter().find(|f| f["key"] == json!(key))).unwrap_or(&Value::Null)
}

fn element_form_of(s: &Session, h: Handle) -> Value {
    let d = s.doc().unwrap();
    element_form(d, d.model.get(h).unwrap()).unwrap()
}

#[test]
fn the_shipped_templates_parse_and_cover_every_trade_listed() {
    let ts = templates();
    for k in [
        "receptacle",
        "switch",
        "occupancy_sensor",
        "luminaire",
        "panel",
        "junction_box",
        "conduit",
        "fitting",
        "cable",
        "wire",
        "fire_alarm_device",
        "data_outlet",
        "thermostat",
        "ptac",
        "ahu",
        "fan",
        "terminal",
        "duct",
        "fixture",
        "pipe",
        "wall",
        "door",
        "window",
        "drywall_sheet",
        "stud",
        "concrete_pour",
        "reinforcement",
        "ceiling",
        "floor",
    ] {
        assert!(ts.iter().any(|t| t.kind == k), "no template for {k}");
    }
    // Every shipped file parses fully (a typo would drop a whole trade silently).
    for f in TRADE_FILES {
        let tf: TradeFile = serde_json::from_str(f).unwrap();
        assert!(!tf.templates.is_empty() && !tf.trade.is_empty());
        for t in &tf.templates {
            let mut keys = BTreeSet::new();
            for fl in &t.fields {
                assert!(keys.insert(fl.key.clone()), "{}: {} twice", t.kind, fl.key);
                assert!(["text", "number", "bool", "enum", "ref"].contains(&fl.ty.as_str()), "{}.{}: type {}", t.kind, fl.key, fl.ty);
                assert!(fl.ty != "enum" || !fl.allowed.is_empty(), "{}.{}: an enum lists its values", t.kind, fl.key);
            }
        }
    }
    // The luminaire has `emergency`; a receptacle does not.
    let lum = ts.iter().find(|t| t.kind == "luminaire").unwrap();
    let rec = ts.iter().find(|t| t.kind == "receptacle").unwrap();
    assert!(lum.fields.iter().any(|f| f.key == "emergency") && !rec.fields.iter().any(|f| f.key == "emergency"));
    // The coordinator's names for receptacle fields.
    for k in [
        "circuit",
        "panel",
        "breaker",
        "poles",
        "gfci_breaker",
        "circuit_description",
        "tr_documented",
        "required_features",
        "breaker_type",
        "gfci",
        "protected_by",
        "tamper_resistant",
        "product",
        "color",
        "configuration",
        "rating",
        "host_wall",
        "host_face",
        "mount_height",
        "room",
        "level",
    ] {
        assert!(rec.fields.iter().any(|f| f.key == k), "receptacle lacks {k}");
    }
}

#[test]
fn a_kind_takes_the_narrowest_template_that_holds_it() {
    let ts = templates();
    let t = |k: &str| template_for(&ts, k).map(|t| t.kind.clone());
    assert_eq!(t("receptacle").as_deref(), Some("receptacle"));
    assert_eq!(t("gfci_receptacle").as_deref(), Some("receptacle"));
    assert_eq!(t("Duplex Receptacle").as_deref(), Some("receptacle"));
    assert_eq!(t("light_fixture").as_deref(), Some("luminaire"));
    assert_eq!(t("legend:recessed downlight").as_deref(), Some("luminaire"));
    assert_eq!(t("lavatory").as_deref(), Some("fixture"));
    assert_eq!(t("return_grille").as_deref(), Some("terminal"));
    assert_eq!(t("horn_strobe").as_deref(), Some("fire_alarm_notification"));
    assert_eq!(t("smoke_detector").as_deref(), Some("fire_alarm_device"));
    assert_eq!(t("exhaust_fan").as_deref(), Some("fan"));
    assert_eq!(t("occupancy sensor").as_deref(), Some("occupancy_sensor"));
    assert_eq!(t("legend:night glow marker 24v"), None);
    assert_eq!(t("device"), None);
    assert_eq!(t(""), None);
    assert_eq!(norm_kind("legend:Night  Glow Marker"), "legend:night glow marker");
    assert_eq!(norm_kind(" Exhaust Fan "), "exhaust_fan");
}

#[test]
fn the_first_pass_makes_a_form_per_kind_from_its_template_else_a_generic_one() {
    let mut d = Drawing::new_imperial();
    let r = receptacle(&mut d, (5.0, 5.0), &[]);
    element(&mut d, "device", (9.0, 5.0), &[("kind", json!("legend:night glow marker 24v"), SourceKind::Legend, "Legend sheet")]);
    let mut s = session(d);
    // Before the pass: the template's form, not yet the project's.
    let f = element_form_of(&s, r);
    assert_eq!(f["template"], json!("receptacle"));
    assert_eq!(f["project"], json!(false));
    let out = job_pass(&mut s, "job: test").unwrap();
    assert_eq!(out["created"], json!(2), "{out}");
    assert_eq!(out["from_template"], json!(1));
    assert_eq!(out["generic"], json!(1));
    let forms = s.execute(FORMS, &json!({})).unwrap();
    assert_eq!(forms["project"], json!(true));
    let all = forms["forms"].as_array().unwrap();
    let rec = all.iter().find(|f| f["kind"] == json!("receptacle")).unwrap();
    assert_eq!(rec["template"], json!("receptacle"));
    assert_eq!(field(rec, "gfci")["origin"], json!("template"));
    let generic_form = all.iter().find(|f| f["kind"] == json!("legend:night glow marker 24v")).unwrap();
    assert_eq!(generic_form["template"], json!("generic"));
    let keys: Vec<&str> = generic_form["fields"].as_array().unwrap().iter().filter_map(|f| f["key"].as_str()).collect();
    assert_eq!(keys, ["kind", "mark", "location", "host", "level", "room", "source"]);
    // Logged: who and why.
    let log = s.execute(LOG, &json!({})).unwrap();
    assert!(
        log["changes"]
            .as_array()
            .unwrap()
            .iter()
            .any(|c| c["action"] == json!("create") && c["who"] == json!("job: test") && c["kind"] == json!("receptacle")),
        "{log}"
    );
    // A second pass makes nothing new.
    let again = job_pass(&mut s, "job: test").unwrap();
    assert_eq!(again["created"], json!(0));
    // bim.element shows the form.
    let f = element_form_of(&s, r);
    assert_eq!(f["project"], json!(true));
    assert_eq!(field(&f, "circuit")["status"], json!("documented"));
    assert_eq!(field(&f, "circuit")["value"], json!("12"));
}

#[test]
fn a_field_a_legend_gives_is_added_to_the_form_with_its_source_and_author() {
    let mut d = Drawing::new_imperial();
    let r = receptacle(&mut d, (5.0, 5.0), &[("finish", json!("stainless plate"), SourceKind::Legend, "Symbol legend row R1")]);
    receptacle(&mut d, (8.0, 5.0), &[("ifc_class", json!("IfcOutlet"), SourceKind::Print, "Sheet P1")]);
    let mut s = session(d);
    let out = job_pass(&mut s, "job: test").unwrap();
    let added = out["added"].as_array().unwrap();
    assert_eq!(added.len(), 1, "bookkeeping keys are never fields: {out}");
    assert_eq!(added[0]["key"], json!("finish"));
    let forms = s.execute(FORMS, &json!({"kind": "receptacle"})).unwrap();
    let f = field(&forms["forms"][0], "finish");
    assert_eq!(f["origin"], json!("added"));
    assert_eq!(f["required"], json!(true), "a legend defines the kind's attribute");
    assert_eq!(f["source"], json!("legend: Symbol legend row R1"));
    assert_eq!(f["author"], json!("job: test"));
    // The second receptacle lacks it: missing there, documented on the first.
    assert_eq!(field(&element_form_of(&s, r), "finish")["status"], json!("documented"));
    // An agent's recorded value under a new key adds that key too.
    let rec = s.execute(RECORD, &json!({"handle": r.hex(), "key": "plate_material", "value": "nylon", "source": {"kind": "submittal", "doc": "Wiring devices product data p.3"}, "author": "agent: reader"})).unwrap();
    assert_eq!(rec["set"], json!(1), "{rec}");
    assert_eq!(rec["added_fields"][0]["key"], json!("plate_material"));
    let f = element_form_of(&s, r);
    assert_eq!(field(&f, "plate_material")["status"], json!("documented"));
    assert_eq!(field(&f, "plate_material")["source"]["kind"], json!("submittal"));
}

#[test]
fn n_a_fields_by_condition_and_for_the_kind_with_reasons_and_revert() {
    let mut d = Drawing::new_imperial();
    let inside = receptacle(&mut d, (5.0, 5.0), &[("gfci", json!("none"), SourceKind::Note, "General note 4")]);
    let outside = receptacle(
        &mut d,
        (40.0, 5.0),
        &[("placement", json!("exterior"), SourceKind::Print, "Sheet P1"), ("gfci", json!("device"), SourceKind::Note, "General note 4")],
    );
    let mut s = session(d);
    job_pass(&mut s, "job: test").unwrap();
    // By condition: weather-resistant applies outdoors only; protected_by where there is GFCI.
    let fi = element_form_of(&s, inside);
    assert_eq!(field(&fi, "weather_resistant")["status"], json!("n/a"));
    assert!(field(&fi, "weather_resistant")["reason"].as_str().unwrap().contains("required only where"));
    assert_eq!(field(&fi, "weather_resistant")["required"], json!(false));
    assert_eq!(field(&fi, "protected_by")["status"], json!("n/a"));
    let fo = element_form_of(&s, outside);
    assert_eq!(field(&fo, "weather_resistant")["status"], json!("missing"));
    assert_eq!(field(&fo, "weather_resistant")["required"], json!(true));
    assert_eq!(field(&fo, "protected_by")["required"], json!(true));
    // For the whole kind, with the reason.
    let r = s.execute(FIELD, &json!({"kind": "receptacle", "key": "box_type", "action": "na", "reason": "the spec leaves boxes to the installer", "author": "agent: spec"})).unwrap();
    let id = r["change"].as_u64().unwrap();
    let f = field(&element_form_of(&s, inside), "box_type").clone();
    assert_eq!(f["status"], json!("n/a"));
    assert_eq!(f["reason"], json!("the spec leaves boxes to the installer"));
    // N/A needs a reason.
    assert!(s.execute(FIELD, &json!({"kind": "receptacle", "key": "color", "action": "na"})).is_err());
    // Reverted: the field applies again (optional box_type shows missing, not required).
    let rv = s.execute(REVERT, &json!({"change": id})).unwrap();
    assert_eq!(rv["reverted"], json!(true));
    assert_eq!(field(&element_form_of(&s, inside), "box_type")["status"], json!("missing"));
    let log = s.execute(LOG, &json!({"kind": "receptacle"})).unwrap();
    let changes = log["changes"].as_array().unwrap();
    assert!(changes.iter().any(|c| c["id"] == json!(id) && c["reverted"] == json!(true)));
    assert!(changes.iter().any(|c| c["action"] == json!("revert")));
    // Reverting twice changes nothing.
    assert_eq!(s.execute(REVERT, &json!({"change": id})).unwrap()["reverted"], json!(false));
    // A field added then removed, both reverted newest first.
    let add = s.execute(FIELD, &json!({"kind": "receptacle", "key": "label", "action": "add", "type": "text", "source": "Spec 26 27 26"})).unwrap()["change"].as_u64().unwrap();
    let rem = s.execute(FIELD, &json!({"kind": "receptacle", "key": "label", "action": "remove"})).unwrap()["change"].as_u64().unwrap();
    assert!(s.execute(REVERT, &json!({"change": add})).is_err(), "a later change of the field is reverted first");
    s.execute(REVERT, &json!({"change": rem})).unwrap();
    assert_eq!(field(&element_form_of(&s, inside), "label")["status"], json!("missing"));
    s.execute(REVERT, &json!({"change": add})).unwrap();
    assert!(field(&element_form_of(&s, inside), "label").is_null());
}

#[test]
fn a_default_value_is_shown_but_never_counted_as_filled() {
    let mut d = Drawing::new_imperial();
    let r = receptacle(&mut d, (5.0, 5.0), &[]);
    let mut s = session(d);
    let f = element_form_of(&s, r);
    assert_eq!(field(&f, "mount_height")["status"], json!("default"));
    assert_eq!(field(&f, "mount_height")["value"], json!(18));
    assert!(f["missing"].as_array().unwrap().contains(&json!("mount_height")));
    let c = s.execute(COMPLETENESS, &json!({"kind": "receptacle", "field": "mount_height"})).unwrap();
    let m = &c["kinds"]["receptacle"]["fields"]["mount_height"];
    assert_eq!(m["required"], json!(1));
    assert_eq!(m["filled"], json!(0));
    assert_eq!(m["default"], json!(1));
    assert_eq!(m["pct"], json!(0.0));
    // A note gives it: documented, filled.
    s.execute(
        RECORD,
        &json!({"handle": r.hex(), "key": "mount_height", "value": 15, "unit": "in", "source": {"kind": "note", "doc": "Accessibility note 2"}}),
    )
    .unwrap();
    let c = s.execute(COMPLETENESS, &json!({"kind": "receptacle", "field": "mount_height"})).unwrap();
    assert_eq!(c["kinds"]["receptacle"]["fields"]["mount_height"]["pct"], json!(100.0), "{c}");
}

#[test]
fn a_schedule_and_a_submittal_that_disagree_become_a_conflict_review_item() {
    let mut d = Drawing::new_imperial();
    let r = receptacle(&mut d, (5.0, 5.0), &[]);
    let mut s = session(d);
    job_pass(&mut s, "job: test").unwrap();
    let a = s
        .execute(RECORD, &json!({"handle": r.hex(), "key": "rating", "value": "20A 125V", "source": {"kind": "schedule", "doc": "Device schedule"}}))
        .unwrap();
    assert_eq!(a["set"], json!(1));
    // The same value again from another document: no conflict.
    let same = s
        .execute(RECORD, &json!({"handle": r.hex(), "key": "rating", "value": "20a 125v", "source": {"kind": "spec", "doc": "Section 26 27 26"}}))
        .unwrap();
    assert!(same["conflicts"].as_array().unwrap().is_empty());
    let b = s
        .execute(
            RECORD,
            &json!({"handle": r.hex(), "key": "rating", "value": "15A 125V", "source": {"kind": "submittal", "doc": "Wiring devices product data"}}),
        )
        .unwrap();
    assert_eq!(b["set"], json!(0), "the documented value is kept: {b}");
    let conflict = &b["conflicts"][0];
    assert_eq!(conflict["key"], json!("rating"));
    let f = element_form_of(&s, r);
    assert_eq!(field(&f, "rating")["status"], json!("conflict"));
    assert_eq!(f["conflicts"], json!(["rating"]));
    let c = s.execute(COMPLETENESS, &json!({"kind": "receptacle", "field": "rating"})).unwrap();
    assert_eq!(c["kinds"]["receptacle"]["fields"]["rating"]["conflict"], json!(1));
    assert_eq!(c["kinds"]["receptacle"]["fields"]["rating"]["filled"], json!(0), "a conflict is not filled");
    // The review queue lists it with both values as candidates.
    let list = s.execute("review.list", &json!({"kind": "conflict"})).unwrap();
    let items = list["items"].as_array().unwrap();
    assert_eq!(items.len(), 1, "{list}");
    let id = items[0]["id"].as_str().unwrap().to_string();
    assert!(items[0]["title"].as_str().unwrap().contains("rating"));
    // Settled with a citation: the value holds, the conflict closes.
    let res = s.execute("review.resolve", &json!({"id": id, "answer": "20A 125V", "citation": {"kind": "submittal", "document": "Wiring devices product data rev 2"}, "author": "user"}));
    assert!(res.is_ok(), "{res:?}");
    let f = element_form_of(&s, r);
    assert_eq!(field(&f, "rating")["status"], json!("documented"));
    assert_eq!(field(&f, "rating")["value"], json!("20A 125V"));
    assert!(open_conflicts(s.doc().unwrap()).is_empty());
}

#[test]
fn completeness_counts_documented_and_derived_over_required_by_kind_field_and_level() {
    let mut d = Drawing::new_imperial();
    // Two receptacles: circuit on both (print), color on one (submittal), room computed on both.
    receptacle(&mut d, (5.0, 5.0), &[("color", json!("white"), SourceKind::Submittal, "Product data")]);
    receptacle(&mut d, (8.0, 5.0), &[]);
    let mut s = session(d);
    job_pass(&mut s, "job: test").unwrap();
    let c = s.execute(COMPLETENESS, &json!({"kind": "receptacle"})).unwrap();
    let k = &c["kinds"]["receptacle"];
    assert_eq!(k["count"], json!(2));
    let f = |key: &str| &k["fields"][key];
    assert_eq!(f("circuit")["filled"], json!(2));
    assert_eq!(f("circuit")["pct"], json!(100.0));
    assert_eq!(f("color")["filled"], json!(1));
    assert_eq!(f("color")["pct"], json!(50.0));
    assert_eq!(f("color")["by_level"]["Level 1"], json!(50.0));
    assert_eq!(f("room")["derived"], json!(2));
    assert_eq!(f("room")["documented_pct"], json!(0.0), "derived fills but isn't documented");
    assert_eq!(f("room")["pct"], json!(100.0));
    // protected_by: n/a on both (no gfci given), so not required.
    assert_eq!(f("protected_by")["required"], json!(0));
    assert_eq!(f("protected_by")["na"], json!(2));
    // The kind's number is the sum over its required fields.
    let req = k["required"].as_u64().unwrap();
    let fil = k["filled"].as_u64().unwrap();
    let sum_req: u64 = k["fields"].as_object().unwrap().values().map(|v| v["required"].as_u64().unwrap()).sum();
    let sum_fil: u64 = k["fields"].as_object().unwrap().values().map(|v| v["filled"].as_u64().unwrap()).sum();
    assert_eq!((req, fil), (sum_req, sum_fil));
    assert_eq!(c["overall"]["required"].as_u64(), Some(req));
    // The next work lists the lowest required fields first.
    let next = c["next_work"].as_array().unwrap();
    assert!(next.first().and_then(Value::as_str).unwrap().contains(" 0"), "{next:?}");
    // Filters.
    let lv = s.execute(COMPLETENESS, &json!({"level": "Level 9"})).unwrap();
    assert_eq!(lv["elements"], json!(0));
    let room = s.execute(COMPLETENESS, &json!({"room": "office 1"})).unwrap();
    assert_eq!(room["elements"], json!(2));
    // The gate: equal passes, a drop fails.
    let g = s.execute(COMPLETENESS, &json!({"kind": "receptacle", "baseline": c.clone()})).unwrap();
    assert_eq!(g["gate"]["pass"], json!(true));
    let mut better = c.clone();
    better["kinds"]["receptacle"]["required_fill_pct"] = json!(99.0);
    let g = s.execute(COMPLETENESS, &json!({"kind": "receptacle", "baseline": better})).unwrap();
    assert_eq!(g["gate"]["pass"], json!(false));
    assert_eq!(g["gate"]["drops"][0]["kind"], json!("receptacle"));
}

#[test]
fn a_project_form_promotes_to_a_template_that_replaces_the_shipped_one() {
    let mut d = Drawing::new_imperial();
    receptacle(&mut d, (5.0, 5.0), &[]);
    let mut s = session(d);
    job_pass(&mut s, "job: test").unwrap();
    s.execute(FIELD, &json!({"kind": "receptacle", "key": "plate", "action": "add", "type": "text"})).unwrap();
    let dir = std::env::temp_dir().join(format!("cadaddy-forms-test-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let r = s.execute(PROMOTE, &json!({"kind": "receptacle", "folder": dir.display().to_string()})).unwrap();
    assert_eq!(r["kind"], json!("receptacle"));
    let back = read_folder(&dir);
    let t = back.iter().find(|t| t.kind == "receptacle").unwrap();
    assert!(t.fields.iter().any(|f| f.key == "plate"));
    assert!(t.fields.iter().all(|f| f.origin.is_empty() && f.author.is_none()));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn hostile_form_params_never_panic() {
    let mut d = Drawing::new_imperial();
    let r = receptacle(&mut d, (5.0, 5.0), &[]);
    let mut s = session(d);
    let long = "x".repeat(100_000);
    let deep = (0..200).fold(json!({"key": "gfci", "in": ["device"]}), |acc, _| json!({"any": [acc]}));
    for (cmd, p) in [
        (FIELD, json!({})),
        (FIELD, json!({"kind": "receptacle", "key": "k", "action": "explode"})),
        (FIELD, json!({"kind": "receptacle", "key": "k", "action": "add", "when": "nonsense"})),
        (FIELD, json!({"kind": "receptacle", "key": "k", "action": "add", "type": "matrix"})),
        (FIELD, json!({"kind": long, "key": long, "action": "add", "when": deep})),
        (RECORD, json!({"handle": r.hex(), "key": "k", "value": {"a": 1}, "source": "legend"})),
        (RECORD, json!({"handle": r.hex(), "key": "k", "value": 1, "source": "default"})),
        (RECORD, json!({"handle": r.hex(), "key": "k", "value": 1, "source": {"kind": "spec", "at": [f64::MAX, "x"]}})),
        (RECORD, json!({"handle": "ZZZZ", "key": "k", "value": 1, "source": "spec"})),
        (REVERT, json!({"change": u64::MAX})),
        (REVERT, json!({"change": "x"})),
        (PROMOTE, json!({"kind": "nothing"})),
        (COMPLETENESS, json!({"baseline": [1, 2]})),
        (COMPLETENESS, json!({"baseline": {"kinds": {"receptacle": {"required_fill_pct": "lots"}}}})),
        (CREATE, json!({"kinds": "receptacle"})),
        (FORMS, json!({"kind": long})),
    ] {
        let _ = s.execute(cmd, &p);
    }
    // A deep condition evaluates (capped) without overflowing.
    let c = Cond { key: Some("a".into()), ..Cond::default() };
    let mut nested = c;
    for _ in 0..1000 {
        nested = Cond { any: vec![nested], ..Cond::default() };
    }
    assert!(!nested.holds(&|_| Some("x".into()), 0));
    assert!(!nested.describe(0).is_empty());
}

#[test]
fn a_schedule_field_on_a_few_stray_elements_stays_optional() {
    let mut d = Drawing::new_imperial();
    // One switch of three carries a panel schedule row's fields (read onto it by mistake, or a
    // switch that really has its own breaker): the kind doesn't owe them.
    let sw = |d: &mut Drawing, x: f64, extra: &[(&str, Value, SourceKind, &str)]| {
        let mut p: Vec<(&str, Value, SourceKind, &str)> = vec![("kind", json!("switch"), SourceKind::Print, "Sheet P1")];
        p.extend(extra.iter().cloned());
        element(d, "device", (x, 5.0), &p)
    };
    sw(&mut d, 1.0, &[("poles", json!(1), SourceKind::Schedule, "Panel PA schedule")]);
    sw(&mut d, 2.0, &[]);
    sw(&mut d, 3.0, &[]);
    let mut s = session(d);
    let out = job_pass(&mut s, "job: test").unwrap();
    assert_eq!(out["added"][0]["key"], json!("poles"));
    assert_eq!(out["added"][0]["required"], json!(false), "{out}");
    let c = s.execute(COMPLETENESS, &json!({"kind": "switch", "field": "poles"})).unwrap();
    assert_eq!(c["kinds"]["switch"]["fields"]["poles"]["required"], json!(0));
}
