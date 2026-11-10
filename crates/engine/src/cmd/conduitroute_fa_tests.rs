//! Q6503: the fire alarm circuits routed and built as cable, each device walking to its panel.
//! Made-up buildings, marks, tags and notes only.

use super::*;
use cadaddy_doc::{Entity, EntityKind, Line};
use serde_json::{Value, json};

/// A line on `layer`, feet.
fn line(d: &mut Drawing, layer: &str, a: (f64, f64), b: (f64, f64)) {
    let p = |(x, y): (f64, f64)| Vec3 { x: x * 12.0, y: y * 12.0, z: 0.0 };
    let h = Handle(d.handseed.max(0x100));
    d.handseed = h.0 + 1;
    let mut e = Entity::new(h, EntityKind::Line(Line { a: p(a), b: p(b) }));
    e.common.layer = layer.into();
    d.model.push(e);
}

fn text(s: &mut crate::Session, layer: &str, at: [f64; 2], t: &str) {
    if s.execute("layer.new", &json!({"name": layer, "current": true})).is_err() {
        s.execute("layer.current", &json!({"name": layer})).expect("layer");
    }
    s.execute("text", &json!({"at": at, "text": t, "height": 6})).expect("text");
}

/// A fire alarm device the readers put in the spec at `(x, y)` feet of `level`.
fn fa(level: &str, x: f64, y: f64, mark: &str, fa_kind: &str, extra: Value) -> Value {
    let mut fields = json!({
        "type": {"value": mark, "source": "plan"},
        "device_type": {"value": format!("fire alarm device {mark}"), "source": "plan"},
        "fa_kind": {"value": fa_kind, "source": "legend"},
        "sheet": {"value": "FA-901", "source": "plan"},
        "symbol": {"value": format!("{:X}", 0x2A0 + (x * 10.0 + y * 1000.0) as u64), "source": "plan"},
    });
    if let (Some(f), Some(x)) = (fields.as_object_mut(), extra.as_object()) {
        for (k, v) in x {
            f.insert(k.clone(), json!({"value": v, "source": "plan"}));
        }
    }
    json!({"id": format!("device:{level}:{x:.1},{y:.1}"), "kind": "fire_alarm_device", "level": level, "at_ft": [x, y], "fields": fields})
}

/// A piece of fire alarm equipment the documents place (inches).
fn equipment(tag: &str, kind: &str, fa_kind: &str, level: &str, min: [f64; 2], max: [f64; 2]) -> Value {
    json!({"tag": tag, "kind": kind, "level": level, "min": min, "max": max, "bottom_ft": 3.5, "height_ft": 2.0, "title": fa_kind.to_ascii_uppercase(),
           "properties": {"FA_KIND": fa_kind}})
}

/// A made-up one-storey building `w` ft long and 30 ft deep: an ELECTRICAL room (101) at its
/// west end and an OFFICE (102); `levels` more storeys over it (devices only); the notes as
/// text on the sheet; built.
fn building(w: f64, levels: &[(&str, f64)], tags: Vec<Value>, equipment: Vec<Value>, notes: &[&str]) -> crate::Session {
    let mut d = Drawing::new_imperial();
    let outer = [((0., 0.), (w, 0.)), ((w, 0.), (w, 30.)), ((w, 30.), (0., 30.)), ((0., 30.), (0., 0.))];
    let west = [((0.5, 0.5), (9.75, 0.5)), ((9.75, 0.5), (9.75, 29.5)), ((9.75, 29.5), (0.5, 29.5)), ((0.5, 29.5), (0.5, 0.5))];
    let e = w - 0.5;
    let east = [((10.25, 0.5), (e, 0.5)), ((e, 0.5), (e, 29.5)), ((e, 29.5), (10.25, 29.5)), ((10.25, 29.5), (10.25, 0.5))];
    for (a, b) in outer.into_iter().chain(west).chain(east) {
        line(&mut d, "A-WALL", a, b);
    }
    let mut s = crate::Session::empty();
    s.open_drawing(d, "Sample Building", None);
    for (at, t) in [([40.0, 190.0], "ELECTRICAL"), ([46.0, 176.0], "101"), ([300.0, 190.0], "OFFICE"), ([306.0, 176.0], "102")] {
        text(&mut s, "A-ROOM-TAG", at, t);
    }
    for (i, n) in notes.iter().enumerate() {
        text(&mut s, "E-ANNO", [4000.0, 900.0 - 20.0 * i as f64], n);
    }
    let mut lv = vec![json!({"name": "L1", "elevation_ft": 0, "height_ft": 12, "ceiling_ft": 9, "base": [0, 0]})];
    lv.extend(levels.iter().map(|(n, e)| json!({"name": n, "elevation_ft": e, "height_ft": 12, "ceiling_ft": 9, "base": [0, 0]})));
    let sp = json!({
        "version": 1, "units_per_foot": 12,
        "levels": lv,
        "layers": [{"layer": "A-WALL", "role": "wall", "wall_type": "Partition"}, {"layer": "A-ROOM-TAG", "role": "room_tag"}],
        "wall_types": [{"name": "Partition", "thickness_in": 6}],
        "tags": tags,
        "mep": {"equipment": equipment},
    });
    s.execute("bspec.set", &json!({"replace": true, "spec": sp})).expect("spec");
    s.execute("model.build", &json!({})).expect("build");
    s
}

/// The built fire alarm devices' handles.
fn fa_devices(s: &crate::Session) -> Vec<String> {
    let d = s.doc().expect("doc");
    d.model
        .iter()
        .filter(|e| super::super::super::wallgen::made_as(e).as_deref() == Some("device"))
        .filter(|e| props::read(&e.common.xdata).get("kind").map(Prop::text).as_deref() == Some("fire_alarm_device"))
        .map(|e| e.handle.hex())
        .collect()
}

/// The handle of the built equipment tagged `tag`.
fn gear(s: &crate::Session, tag: &str) -> Option<String> {
    let d = s.doc().ok()?;
    d.model.iter().find(|e| super::super::super::bimlook::made_tag(e, "tag") == Some(tag)).map(|e| e.handle.hex())
}

fn report(s: &mut crate::Session) -> Value {
    s.execute("conduit.route", &json!({"section": "fire_alarm"})).expect("report")
}

/// The fire alarm cable the takeoff lists, feet, by its type.
fn takeoff_cable(s: &mut crate::Session) -> BTreeMap<String, f64> {
    let tk = s.execute("conduit.takeoff", &json!({"price": false})).expect("takeoff");
    let mut out = BTreeMap::new();
    for l in tk["lines"].as_array().into_iter().flatten().filter(|l| l["kind"] == json!("cable")) {
        if l["item"].as_str().is_some_and(|i| i.starts_with("Fire alarm cable")) {
            *out.entry(l["type"].as_str().unwrap_or_default().to_string()).or_default() += l["qty"].as_f64().unwrap_or(0.0);
        }
    }
    out
}

/// The finding's building: ten 177 cd horn/strobes, six smoke detectors and two pull stations,
/// the control panel the documents place in the electrical room.
fn sample() -> crate::Session {
    let mut tags = Vec::new();
    for i in 0..10 {
        tags.push(fa("L1", 15.0 + 4.0 * f64::from(i), 15.0, "HS", "horn/strobe", json!({"mount": "ceiling", "candela": "177cd"})));
    }
    for i in 0..6 {
        tags.push(fa("L1", 15.0 + 8.0 * f64::from(i), 25.0, "S", "smoke detector", json!({"mount": "ceiling"})));
    }
    tags.push(fa("L1", 10.8, 2.0, "P", "pull station", json!({"mount_height_in": "48"})));
    tags.push(fa("L1", 59.2, 15.0, "P", "pull station", json!({"mount_height_in": "48"})));
    building(60.0, &[], tags, vec![equipment("FACP", "fire alarm panel", "control panel", "L1", [6.0, 120.0], [30.0, 126.0])], &[])
}

#[test]
fn every_device_of_two_nacs_and_one_slc_walks_to_the_panel_and_its_cable_is_taken_off() {
    let mut s = sample();
    let devs = fa_devices(&s);
    assert_eq!(devs.len(), 18, "{devs:?}");
    let facp = gear(&s, "FACP").expect("panel built");
    // Every device traces to the panel; none reports an open end.
    for h in &devs {
        let c = s.execute("bim.chain", &json!({"handle": h})).expect("chain");
        assert!(c["note"].is_null(), "{h}: {c}");
        assert_eq!(c["source"]["handle"], json!(facp), "{h}: {c}");
        assert!(c["length_ft"].as_f64().is_some_and(|f| f > 0.0), "{c}");
        assert!(c["circuit"]["name"].as_str().is_some_and(|n| !n.is_empty()), "{c}");
    }
    // Two NACs (3.9 A on the panel's own 2 x 2.0 A) and one SLC.
    let r = report(&mut s);
    assert_eq!(
        (r["nacs"].clone(), r["slcs"].clone(), r["routed_devices"].clone(), r["unrouted"].clone()),
        (json!(2), json!(1), json!(18), json!(0)),
        "{r}"
    );
    let list = r["circuit_list"].as_array().cloned().unwrap_or_default();
    let nacs: Vec<&Value> = list.iter().filter(|c| c["role"] == json!("nac")).collect();
    assert!(nacs.iter().all(|c| c["amps"].as_f64().is_some_and(|a| a <= 2.0 + 1e-9) && c["source"] == json!("FACP")), "{list:?}");
    assert_eq!(nacs.iter().map(|c| c["devices"].as_u64().unwrap_or(0)).sum::<u64>(), 10);
    assert!(nacs.iter().all(|c| c["order"] == json!("nearest neighbour") && c["cable"] == json!("FPLR 14/2")), "{nacs:?}");
    let slc = list.iter().find(|c| c["role"] == json!("slc")).expect("slc");
    assert_eq!((slc["devices"].clone(), slc["cable"].clone()), (json!(8), json!("FPLR 18/2 twisted")), "{slc}");
    // No power supply: the panel's own outputs carry the storey.
    assert_eq!(r["power_supplies"], json!([]), "{r}");
    // The cable lengths appear in the takeoff, as long as the routing's.
    let tk = takeoff_cable(&mut s);
    let routed: f64 = r["ft_by_system"].as_object().into_iter().flatten().filter_map(|(_, v)| v.as_f64()).sum();
    let taken: f64 = tk.values().sum();
    assert!(tk.get("FPLR 14/2").is_some_and(|f| *f > 0.0) && tk.get("FPLR 18/2 twisted").is_some_and(|f| *f > 0.0), "{tk:?}");
    assert!((taken - routed).abs() < 0.5, "takeoff {taken} vs routed {routed}");
    // Each piece is inferred, with its rule.
    let d = s.doc().expect("doc");
    let cable =
        d.model.iter().find(|e| props::read(&e.common.xdata).get("system").map(Prop::text).as_deref() == Some(NAC_SYSTEM)).expect("a NAC cable");
    let (made, basis) = super::super::super::provenance::of_element(cable).expect("provenance");
    assert_eq!(made, Made::Inferred);
    assert!(basis.starts_with(RULE_NAC), "{basis}");
}

#[test]
fn a_storeys_load_past_the_panels_outputs_gets_a_power_supply_in_its_service_room() {
    let mut tags = Vec::new();
    for i in 0..16 {
        tags.push(fa("L1", 14.0 + 2.8 * f64::from(i), 15.0, "HS", "horn/strobe", json!({"mount": "ceiling", "candela": "177cd"})));
    }
    tags.push(fa("L1", 30.0, 25.0, "S", "smoke detector", json!({"mount": "ceiling"})));
    let mut s = building(60.0, &[], tags, vec![equipment("FACP", "fire alarm panel", "control panel", "L1", [6.0, 120.0], [30.0, 126.0])], &[]);
    // 16 x 0.39 A = 6.24 A: past the panel's 4.0 A, one 8.0 A supply, in the electrical room.
    let ps = gear(&s, "NAC-PS-1").expect("a power supply placed");
    let facp = gear(&s, "FACP").expect("panel");
    {
        let d = s.doc().expect("doc");
        let e = d.model.iter().find(|e| e.handle.hex() == ps).expect("element");
        let (made, basis) = super::super::super::provenance::of_element(e).expect("provenance");
        assert_eq!(made, Made::Inferred);
        assert!(basis.starts_with(RULE_SUPPLY) && basis.contains("101 (ELECTRICAL)"), "{basis}");
        let sp = BuildingSpec::from_json(d.building_spec.as_deref().unwrap_or_default()).expect("spec");
        assert_eq!(sp.mep.equipment.iter().filter(|e| placed_by_rule(e)).count(), 1, "stored with the spec");
    }
    let r = report(&mut s);
    assert_eq!((r["nacs"].clone(), r["unrouted"].clone()), (json!(4), json!(0)), "{r}");
    let by = r["by_source"].as_array().cloned().unwrap_or_default();
    assert!(by.iter().any(|b| b["source"] == json!("NAC-PS-1") && b["nacs"] == json!(4) && b["over_outputs"] == json!(false)), "{by:?}");
    // An appliance: its circuit to the power supply, the supply up to the panel.
    for h in fa_devices(&s) {
        let c = s.execute("bim.chain", &json!({"handle": h})).expect("chain");
        assert!(c["note"].is_null(), "{c}");
        if c["circuit"]["system"] == json!(NAC_SYSTEM) {
            assert_eq!(c["source"]["handle"], json!(ps), "{c}");
            assert_eq!(c["upstream"][0]["handle"], json!(facp), "{c}");
        } else {
            assert_eq!(c["source"]["handle"], json!(facp), "{c}");
        }
    }
    let c = s.execute("bim.chain", &json!({"handle": ps})).expect("chain");
    assert_eq!(c["source"]["handle"], json!(facp), "the supply's start circuit: {c}");
    // Built again: placed again, not doubled; routed again the same.
    s.execute("model.build", &json!({})).expect("build");
    assert!(gear(&s, "NAC-PS-1").is_some() && gear(&s, "NAC-PS-2").is_none());
    s.execute("conduit.reroute", &json!({})).expect("reroute");
    assert!(gear(&s, "NAC-PS-1").is_some() && gear(&s, "NAC-PS-2").is_none());
    assert_eq!(report(&mut s)["nacs"], json!(4));
}

#[test]
fn the_documents_circuits_and_addresses_set_the_order_and_the_loop_rises_to_the_next_storey() {
    let mut tags = Vec::new();
    // Level 2's appliances on the documents' circuits, listed out of their address order.
    for (x, c, a) in [
        (40.0, "X1.N1", "X1.N1.03"),
        (20.0, "X1.N1", "X1.N1.01"),
        (30.0, "X1.N1", "X1.N1.02"),
        (50.0, "X1.N2", "X1.N2.02"),
        (45.0, "X1.N2", "X1.N2.01"),
    ] {
        tags.push(fa("L2", x, 15.0, "HS", "horn/strobe", json!({"mount": "ceiling", "candela": "75cd", "nac_circuit": c, "address": a})));
    }
    // One loop over both storeys.
    for (lv, x, a) in [("L1", 20.0, "L1.02"), ("L1", 40.0, "L1.01"), ("L2", 25.0, "L1.03"), ("L2", 35.0, "L1.04")] {
        tags.push(fa(lv, x, 25.0, "S", "smoke detector", json!({"mount": "ceiling", "slc_circuit": "L1", "address": a})));
    }
    let eq = vec![
        equipment("FACP", "fire alarm panel", "control panel", "L1", [6.0, 120.0], [30.0, 126.0]),
        equipment("X1", "fire alarm power supply", "power supply", "L2", [6.0, 200.0], [22.0, 204.0]),
    ];
    let mut s = building(60.0, &[("L2", 12.0)], tags, eq, &[]);
    // Routed again: the spec stores its routing (its circuits' tags and runs).
    s.execute("conduit.reroute", &json!({})).expect("reroute");
    let r = report(&mut s);
    assert_eq!((r["nacs"].clone(), r["slcs"].clone(), r["unrouted"].clone()), (json!(2), json!(1), json!(0)), "{r}");
    assert_eq!(r["power_supplies"], json!([{"tag": "X1", "level": "L2"}]), "the documents' supply, none placed: {r}");
    let list = r["circuit_list"].as_array().cloned().unwrap_or_default();
    for name in ["X1.N1", "X1.N2"] {
        let c = list.iter().find(|c| c["circuit"] == json!(name)).unwrap_or_else(|| panic!("{name}: {list:?}"));
        assert_eq!((c["source"].clone(), c["order"].clone()), (json!("X1"), json!("addresses")), "{c}");
    }
    // The members in address order: X1.N1 runs 20, 30, 40 ft east.
    let d = s.doc().expect("doc");
    let sp = BuildingSpec::from_json(d.building_spec.as_deref().unwrap_or_default()).expect("spec");
    let t = sp.tags.iter().find(|t| t.id == "fa:X1.N1").expect("tag");
    let xs: Vec<f64> = t.fields["members"]
        .value
        .split(',')
        .filter_map(Handle::parse_hex)
        .filter_map(|h| d.model.get(h))
        .filter_map(|e| mesh_box(e))
        .map(|(lo, hi)| ((lo.x + hi.x) / 24.0).round())
        .collect();
    assert_eq!(xs, vec![20.0, 30.0, 40.0], "{t:?}");
    // The loop takes the riser up to Level 2: a vertical piece about a storey high.
    let riser = sp.mep.runs.iter().find(|r| r.system == SLC_SYSTEM && r.top_ft.is_some()).expect("a riser piece");
    let rise = riser.top_ft.unwrap_or(0.0) - riser.bottoms_ft.first().copied().unwrap_or(0.0);
    assert!((rise - 12.0).abs() < 2.0, "{riser:?}");
    // Level 2's detectors walk the loop down to the panel; its appliances to X1, then the panel.
    let (facp, x1) = (gear(&s, "FACP").expect("panel"), gear(&s, "X1").expect("supply"));
    for h in fa_devices(&s) {
        let c = s.execute("bim.chain", &json!({"handle": h})).expect("chain");
        let want = if c["circuit"]["system"] == json!(NAC_SYSTEM) { &x1 } else { &facp };
        assert_eq!(c["source"]["handle"], json!(want), "{c}");
    }
}

#[test]
fn notes_calling_for_conduit_below_a_height_class_a_and_plenum_cable_are_built_as_they_say() {
    let mut tags = Vec::new();
    for i in 0..3 {
        tags.push(fa("L1", 20.0 + 10.0 * f64::from(i), 15.0, "HS", "horn/strobe", json!({"mount": "ceiling"})));
    }
    tags.push(fa("L1", 59.2, 15.0, "P", "pull station", json!({"mount_height_in": "48"})));
    tags.push(fa("L1", 30.0, 25.0, "S", "smoke detector", json!({"mount": "ceiling"})));
    let notes = [
        "ALL WIRING BELOW 7' SHALL BE INSTALLED IN CONDUIT PER NFPA 70 ARTICLE 760.",
        "FIRE ALARM SIGNALING LINE AND NOTIFICATION CIRCUITS SHALL BE CLASS A.",
        "FIRE ALARM CABLE IN CEILINGS USED AS RETURN AIR PLENUMS SHALL BE PLENUM RATED (FPLP).",
    ];
    let mut s = building(60.0, &[], tags, vec![equipment("FACP", "fire alarm panel", "control panel", "L1", [6.0, 120.0], [30.0, 126.0])], &notes);
    let r = report(&mut s);
    assert_eq!(r["documents"]["raceway"]["below_ft"], json!(7.0), "{r}");
    assert!(r["documents"]["class_a"].is_string() && r["documents"]["plenum"].is_string(), "{r}");
    let list = r["circuit_list"].as_array().cloned().unwrap_or_default();
    // Class A: each NAC and SLC back to the panel (a leg more than its devices).
    for c in list.iter().filter(|c| matches!(c["role"].as_str(), Some("nac" | "slc"))) {
        assert_eq!(c["class"], json!("A"), "{c}");
        assert_eq!(c["legs"].as_u64(), c["devices"].as_u64().map(|n| n + 1), "{c}");
        assert!(c["cable"].as_str().is_some_and(|t| t.starts_with("FPLP")), "{c}");
    }
    // The pull station's drop below 7 ft in EMT; nothing over the ceiling in it.
    let d = s.doc().expect("doc");
    let sticks: Vec<(f64, f64)> = d
        .model
        .iter()
        .filter(|e| {
            let ps = props::read(&e.common.xdata);
            ps.get(super::super::super::mepatoms::ATOM_KEY).map(Prop::text).as_deref() == Some("conduit")
                && ps.get("system").map(Prop::text).is_some_and(|y| is_system(&y))
        })
        .filter_map(|e| mesh_box(e))
        .map(|(lo, hi)| (lo.z / 12.0, hi.z / 12.0))
        .collect();
    assert!(!sticks.is_empty(), "the drop below 7 ft is in conduit");
    assert!(sticks.iter().all(|(_, hi)| *hi <= 7.1), "{sticks:?}");
}

#[test]
fn a_nac_whose_drop_number_14_cant_carry_is_number_12_with_the_reason() {
    // Five 177 cd appliances 380 ft from the panel: 1.95 A over ~400 ft drops ~4.8 V on #14.
    let tags: Vec<Value> =
        (0..5).map(|i| fa("L1", 380.0 + 2.0 * f64::from(i), 15.0, "HS", "horn/strobe", json!({"mount": "ceiling", "candela": "177"}))).collect();
    let mut s = building(400.0, &[], tags, vec![equipment("FACP", "fire alarm panel", "control panel", "L1", [6.0, 120.0], [30.0, 126.0])], &[]);
    let r = report(&mut s);
    let c = r["circuit_list"].as_array().and_then(|l| l.iter().find(|c| c["role"] == json!("nac")).cloned()).expect("nac");
    assert_eq!(c["cable"], json!("FPLR 12/2"), "{c}");
    assert!(c["upsized"].as_str().is_some_and(|w| w.contains("#14") && w.contains("#12")), "{c}");
    s.execute("conduit.reroute", &json!({})).expect("reroute");
    let d = s.doc().expect("doc");
    let sp = BuildingSpec::from_json(d.building_spec.as_deref().unwrap_or_default()).expect("spec");
    let t = sp.tags.iter().find(|t| t.kind == TAG_KIND && t.fields.get("role").is_some_and(|f| f.value == "nac")).expect("tag");
    assert_eq!(t.fields.get("design:cable_size").map(|f| f.value.as_str()), Some("14/2"), "{t:?}");
}

#[test]
fn with_no_panel_in_the_documents_one_is_placed_by_rule_and_wired() {
    let tags = vec![
        fa("L1", 20.0, 15.0, "HS", "horn/strobe", json!({"mount": "ceiling"})),
        fa("L1", 30.0, 25.0, "S", "smoke detector", json!({"mount": "ceiling"})),
    ];
    let mut s = building(60.0, &[], tags, Vec::new(), &[]);
    let facp = gear(&s, "FACP").expect("a panel placed");
    let r = report(&mut s);
    assert_eq!((r["control_panel"]["tag"].clone(), r["unrouted"].clone()), (json!("FACP"), json!(0)), "{r}");
    for h in fa_devices(&s) {
        let c = s.execute("bim.chain", &json!({"handle": h})).expect("chain");
        assert_eq!(c["source"]["handle"], json!(facp), "{c}");
    }
}

#[test]
fn the_wiring_statements_are_read_by_their_words() {
    let read = |t: &str, fa_sheet: bool| read_docs(&[Statement { cite: String::new(), text: t.to_string(), fa_doc: fa_sheet }]);
    let r = read("ALL WIRING BELOW 7' SHALL BE INSTALLED IN CONDUIT PER NFPA 70 ARTICLE 760.", false);
    assert_eq!(r.raceway.as_ref().map(|x| (x.0.as_str(), x.1)), Some(("EMT", Some(7.0))));
    let r = read("FIRE ALARM WIRING BELOW 84\" AFF SHALL BE IN EMT.", false);
    assert_eq!(r.raceway.as_ref().map(|x| x.1), Some(Some(7.0)));
    let r = read("ALL FIRE ALARM WIRING SHALL BE INSTALLED IN CONDUIT.", false);
    assert_eq!(r.raceway.as_ref().map(|x| x.1), Some(None));
    // Not the fire alarm's, or saying it needs none.
    assert_eq!(read("PROVIDE 3/4\" CONDUIT FOR ALL RECEPTACLE WIRING.", false), WiringDocs::default());
    assert!(read("FIRE ALARM CABLE MAY BE INSTALLED WITHOUT CONDUIT ABOVE ACCESSIBLE CEILINGS.", false).raceway.is_none());
    // On a fire alarm sheet its subject is the fire alarm system.
    assert!(read("ALL CIRCUITS SHALL BE CLASS A.", true).class_a.is_some());
    assert!(read("ALL CIRCUITS SHALL BE CLASS A.", false).class_a.is_none());
    assert!(read("SIGNALING LINE CIRCUIT 16 AWG 2 CONDUCTOR FPLP WIRE", true).plenum.is_some());
    // Class A only where a statement requires it: not product data listing what a panel takes.
    assert!(read("FIRE ALARM PANEL FEATURES: SUPPORTS CLASS A OR CLASS B WIRING ON EVERY SLC", true).class_a.is_none());
    assert!(read("FIRE ALARM PANEL FEATURES: 1,000 ADDRESSES, CLASS A CAPABLE", true).class_a.is_none());
    assert!(read("ALL FIRE ALARM SIGNALING LINE CIRCUITS SHALL BE WIRED CLASS A.", false).class_a.is_some());
}

#[test]
fn a_wire_legend_gives_each_circuits_cable() {
    let legend = "WIRE LEGEND DESCRIPTION WIRE SIZE CIRCUIT PATH CLASS Signaling Line Circuit #1 (SLC1) 16 AWG 2 Conductor FPLP Wire B \
                  Auxiliary Power Circuit for Door Holders (FP N#) 14 AWG 2 Conductor FPLP Wire B Notification Appliance Circuit (FP N#) \
                  12 AWG 2 Conductor FPLP Wire B Annunciator Bus (RS-485) 18 AWG 4 Conductor FPLP Wire Data Circuit";
    let r = read_docs(&[Statement { cite: "Sample Submittal page 2".into(), text: legend.into(), fa_doc: true }]);
    let got: Vec<(String, u32, u32, String)> = r.cables.iter().map(|(k, c)| (k.clone(), c.awg, c.conductors, c.ty.clone())).collect();
    assert_eq!(
        got,
        vec![("annunciator".into(), 18, 4, "FPLP".into()), ("nac".into(), 12, 2, "FPLP".into()), ("slc".into(), 16, 2, "FPLP".into())],
        "{r:?}"
    );
    assert!(r.cables["slc"].words.starts_with("Sample Submittal page 2: …Signaling Line Circuit #1 (SLC1) 16 AWG"), "{:?}", r.cables["slc"]);
    // The router's cable: the legend's.
    let (c, documented) = r.cable("slc", &rules().cables.slc);
    assert_eq!((c.ty.as_str(), c.size.as_str(), c.awg, documented), ("FPLP", "16/2", 16, true));
    let (c, documented) = WiringDocs::default().cable("slc", &rules().cables.slc);
    assert_eq!((c.ty.as_str(), c.awg, documented), ("FPLR", 18, false));
}

#[test]
fn currents_drops_addresses_and_orders_by_the_rules() {
    let fr = rules();
    assert_eq!(fr.amps_of("horn/strobe", Some(177.0)).0, 0.39);
    assert_eq!(fr.amps_of("strobe", Some(20.0)).0, 0.13, "the next listed candela up");
    assert_eq!(fr.amps_of("strobe", None).0, 0.21, "taken at 75 cd");
    assert_eq!(fr.amps_of("horn", Some(177.0)).0, 0.05);
    assert!((fr.drop(2.0, 100.0, 14) - 2.0 * 2.0 * 100.0 * 0.00307).abs() < 1e-9);
    assert_eq!((address_number("BP7.N1.01"), address_number("L7.42"), address_number("X")), (Some(1), Some(42), None));
    assert_eq!(numbers("15/75cd"), vec![15.0, 75.0]);
    let pts = [(0, Vec2::new(30.0, 0.0)), (1, Vec2::new(10.0, 0.0)), (2, Vec2::new(20.0, 0.0))];
    assert_eq!(nn_order(Vec2::new(0.0, 0.0), &pts), vec![1, 2, 0]);
    let path = [Vec3::new(0.0, 0.0, 48.0), Vec3::new(0.0, 0.0, 110.0), Vec3::new(100.0, 0.0, 110.0)];
    let low = below(&path, 84.0);
    assert_eq!(low.len(), 1);
    assert!(low[0].iter().all(|p| p.z <= 84.0 + 1e-9) && low[0].last().is_some_and(|p| (p.z - 84.0).abs() < 1e-9), "{low:?}");
}

#[test]
fn hostile_fire_alarm_wiring_never_panics() {
    let st = |t: &str, fa_doc: bool| Statement { cite: "\u{1F525}".into(), text: t.into(), fa_doc };
    let _ = read_docs(&[
        st("", true),
        st("BELOW", true),
        st("FIRE ALARM WIRING BELOW 9999999999999999999999' IN CONDUIT", false),
        st("SIGNALING LINE CIRCUIT 99999999999999 AWG 99999999999 CONDUCTOR \u{e9}\u{e9}", true),
        st("NOTIFICATION APPLIANCE CIRCUIT", true),
    ]);
    let _ = numbers(&"9".repeat(500));
    let _ = address_number("....");
    let _ = below(&[Vec3::new(f64::NAN, 0.0, 0.0), Vec3::new(0.0, f64::INFINITY, 1.0)], 5.0);
    let _ = nn_order(Vec2::new(f64::NAN, 0.0), &[(0, Vec2::new(1.0, 1.0))]);
    let tags = vec![
        fa("L1", 20.0, 15.0, "HS", "horn/strobe", json!({"candela": "lots", "nac_circuit": "#.#", "address": "##"})),
        fa("L1", 25.0, 15.0, "S", "smoke detector", json!({"slc_circuit": "L#9", "address": ".."})),
    ];
    let mut s = building(60.0, &[], tags, Vec::new(), &[]);
    let _ = report(&mut s);
    s.execute("conduit.reroute", &json!({})).expect("reroute");
}

#[test]
fn a_fire_alarm_submittals_notes_are_read_as_its_wiring_method_and_kept_for_the_router() {
    let doc = |name: &str, trade: Option<&str>, text: &str| scope::Doc {
        name: name.into(),
        tier: cadaddy_doc::projrules::SourceTier::Submittal,
        trade: trade.map(str::to_string),
        paras: vec![scope::Para { at: "note 7".into(), heading: "GENERAL NOTES".into(), text: text.into(), cells: Vec::new() }],
    };
    let docs = [
        doc(
            "Sample Submittal 2831",
            Some("fire alarm"),
            "Ceiling height is 10' or less. All wiring below 7' shall be installed in conduit. All circuits shall be Class A.",
        ),
        doc("Sample Spec 2605", Some("electrical"), "All receptacle wiring shall be in EMT conduit."),
    ];
    let r = read_documents(&docs);
    let (m, below, words) = r.raceway.clone().expect("raceway");
    assert_eq!((m.as_str(), below), ("EMT", Some(7.0)));
    assert!(words.starts_with("Sample Submittal 2831 note 7:"), "{words}");
    assert!(r.class_a.is_some_and(|w| w.contains("Class A")));
    // Kept with the drawing; a later reading saying nothing keeps it.
    let mut d = Drawing::new_imperial();
    read_documents(&docs).store(&mut d);
    let kept = read_documents(&[]).or(WiringDocs::stored(&d));
    assert_eq!(kept.raceway.map(|x| x.1), Some(Some(7.0)));
}

#[test]
fn a_power_supply_goes_in_a_service_room_else_on_the_corridor_never_in_a_unit_or_stair() {
    use super::super::super::gearbuild::RoomBox;
    let room = |number: &str, name: &str, x: f64| RoomBox {
        level: "L2".into(),
        number: number.into(),
        name: name.into(),
        outline: vec![Vec2::new(x, 0.0), Vec2::new(x + 120.0, 0.0), Vec2::new(x + 120.0, 120.0), Vec2::new(x, 120.0)],
        offset: Vec2::ZERO,
        doors: Vec::new(),
        building: false,
    };
    let fr = rules();
    let units: BTreeSet<(String, String)> = [("l2".to_string(), "201".to_string())].into();
    let pick =
        |rooms: &[RoomBox]| place_in_room(rooms, "L2", Vec2::new(0.0, 0.0), &fr.power_supply, 1, 12.0, &[], &units).first().map(|(_, n)| n.clone());
    // The guest unit and the stair nearest the riser are passed over for the back-of-house room.
    let rooms = [room("201", "SUITE", 0.0), room("S1", "STAIR 1", 150.0), room("C2", "CORRIDOR", 300.0), room("B2", "BOH", 600.0)];
    assert_eq!(pick(&rooms).as_deref(), Some("B2 (BOH)"));
    // No service room: the corridor wall.
    assert_eq!(pick(&rooms[..3]).as_deref(), Some("C2 (CORRIDOR)"));
    // Only a unit and a stair: not placed.
    assert_eq!(pick(&rooms[..2]), None);
}
