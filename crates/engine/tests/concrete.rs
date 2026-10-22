//! Concrete in the BIM (Q328): a made-up hotel's ground floor — switchboard MSB fed from below,
//! transformer TX-1, generator GEN-1, a wall panel, a water heater and a rooftop unit — on a slab
//! with a strip footing and a column footing. Planted where the field finds them: a generator
//! with no pad decision, a transformer pad nobody owns, a footing pour with no concrete-encased
//! electrode, a sleeve through a footing's rebar mat and anchor bolts longer than the concrete.
//! Quantities are checked against hand calculations.

use cadaddy_engine::Session;
use serde_json::{Value, json};

fn spec() -> Value {
    json!({
        "version": 1, "units_per_foot": 12,
        "levels": [{"name": "L1", "elevation_ft": 0, "height_ft": 14}, {"name": "Roof", "elevation_ft": 14, "height_ft": 4, "ceiling_ft": 3}],
        "mep": {
            "systems": [{"name": "E-UG", "layer": "E-UG", "kind": "conduit", "size_in": [2, 2], "bottom_ft": -1}],
            "runs": [{"system": "E-UG", "level": "L1", "points": [[24, 24], [264, 24]], "size_in": [2, 2], "bottoms_ft": [-1]}],
            "equipment": [
                {"tag": "MSB", "kind": "switchboard", "level": "L1", "min": [0, 0], "max": [72, 36], "bottom_ft": 0, "height_ft": 7.5},
                {"tag": "TX-1", "kind": "transformer", "level": "L1", "min": [120, 0], "max": [156, 30], "bottom_ft": 0, "height_ft": 5},
                {"tag": "GEN-1", "kind": "generator", "level": "L1", "min": [240, 120], "max": [360, 168], "bottom_ft": 0, "height_ft": 6},
                {"tag": "P-1", "kind": "panel", "level": "L1", "min": [200, 0], "max": [220, 6], "bottom_ft": 1, "height_ft": 4,
                 "properties": {"MOUNTING": "Surface"}},
                {"tag": "WH-1", "kind": "water_heater", "level": "L1", "min": [400, 300], "max": [424, 324], "bottom_ft": 0, "height_ft": 5, "round": true},
                {"tag": "RTU-1", "kind": "rtu", "level": "Roof", "min": [100, 100], "max": [190, 150], "bottom_ft": 0, "height_ft": 4}
            ],
            "feeders": [
                {"tag": "F-SVC", "from": "", "to": "MSB", "sets": 4, "size": "500 kcmil", "conduits": 4, "conduit_in": 4, "entry": "bottom"}
            ]
        },
        "concrete": {
            "elements": [
                {"id": "SOG-1", "kind": "slab", "level": "L1", "min": [0, 0], "max": [480, 360], "depth_in": 5, "top_ft": 0,
                 "mesh": "6x6-W2.9xW2.9", "owner": "concrete", "pour": "P-2", "source": "S-101 slab note"},
                {"id": "F-1", "kind": "footing", "level": "L1", "path": [[0, -24], [480, -24]], "width_in": 24, "depth_in": 12, "top_ft": -1,
                 "earth_formed": true, "rebar": [{"bar": "#5", "direction": "long", "count": 3}], "owner": "concrete", "pour": "P-1"},
                {"id": "F-2", "kind": "footing", "level": "L1", "min": [200, 200], "max": [248, 248], "depth_in": 18, "top_ft": -1,
                 "rebar": [{"bar": "#5", "direction": "each_way", "spacing_in": 12}], "owner": "concrete", "pour": "P-1",
                 "embeds": [{"kind": "sleeve", "size": "4\"", "depth_in": 12, "source": "S-501 detail 3"},
                            {"kind": "anchor_bolt", "size": "3/4\" dia", "count": 4, "diameter_in": 0.75, "embed_in": 18}]}
            ],
            "pads": [
                {"equipment": "MSB", "required": true, "basis": "26 24 13 2.4", "owner": "electrical", "pour": "P-2"},
                {"equipment": "TX-1", "required": true, "basis": "utility pad detail E-501/2", "owner": ""},
                {"equipment": "WH-1", "required": false, "basis": "on the slab per P-501"}
            ],
            "pours": [
                {"id": "P-1", "name": "Footings", "date": "2026-03-10"},
                {"id": "P-2", "name": "Slab on grade", "date": "2026-04-01"}
            ]
        }
    })
}

fn session() -> Session {
    let mut s = Session::new();
    s.execute("bspec.set", &json!({"replace": true, "spec": spec()})).unwrap();
    s.execute("bim.set_type", &json!({"mark": "MSB", "fields": {"dimensions": "72\"W x 36\"D x 90\"H", "reviewStatus": "approved"}})).unwrap();
    s
}

fn found<'a>(r: &'a Value, about: &str, check: &str) -> Vec<&'a Value> {
    r["findings"].as_array().unwrap().iter().filter(|f| f["about"] == about && f["check"] == check).collect()
}

fn element<'a>(r: &'a Value, id: &str) -> &'a Value {
    r["elements"].as_array().unwrap().iter().find(|e| e["id"] == id).unwrap_or_else(|| panic!("no element {id} in {r}"))
}

fn near(a: &Value, b: f64) -> bool {
    a.as_f64().is_some_and(|a| (a - b).abs() < 0.011)
}

#[test]
fn missing_pads_and_owners_are_caught() {
    let mut s = session();
    let list = s.execute("concrete.list", &json!({})).unwrap();
    let tags: Vec<&str> = list["equipment"].as_array().unwrap().iter().map(|e| e["tag"].as_str().unwrap()).collect();
    // Floor- and grade-mounted only: not the wall panel, not the rooftop unit.
    assert_eq!(tags, ["MSB", "TX-1", "GEN-1", "WH-1"], "{list}");
    let r = s.execute("concrete.check", &json!({})).unwrap();
    println!("{:#}", r["findings"]);
    let gen_pad = found(&r, "GEN-1", "pad decision");
    assert_eq!(gen_pad.len(), 1);
    assert_eq!(gen_pad[0]["severity"], "error");
    assert_eq!(found(&r, "TX-1", "pad owner").len(), 1, "the transformer's pad has no owner");
    assert!(found(&r, "MSB", "pad decision").is_empty() && found(&r, "MSB", "pad owner").is_empty());
    assert!(r["findings"].as_array().unwrap().iter().all(|f| f["about"] != "WH-1"), "decided: no pad");
    assert!(r["findings"].as_array().unwrap().iter().all(|f| f["about"] != "P-1" && f["about"] != "RTU-1"));
    // Deciding them clears both.
    s.execute(
        "concrete.set",
        &json!({"pads": [{"equipment": "GEN-1", "required": true, "basis": "generator manufacturer", "owner": "concrete"},
                         {"equipment": "TX-1", "required": true, "basis": "utility pad detail E-501/2", "owner": "utility"}]}),
    )
    .unwrap();
    let r = s.execute("concrete.check", &json!({})).unwrap();
    assert!(found(&r, "GEN-1", "pad decision").is_empty() && found(&r, "TX-1", "pad owner").is_empty(), "{r}");
    // Undo brings the failures back.
    s.execute("undo", &json!({})).unwrap();
    let r = s.execute("concrete.check", &json!({})).unwrap();
    assert_eq!(found(&r, "GEN-1", "pad decision").len(), 1);
}

#[test]
fn the_pad_follows_the_submittal_footprint() {
    let mut s = session();
    let list = s.execute("concrete.list", &json!({})).unwrap();
    let msb = list["equipment"].as_array().unwrap().iter().find(|e| e["tag"] == "MSB").unwrap();
    assert_eq!(msb["footprint_in"], json!([72.0, 36.0]));
    assert!(msb["footprint_source"].as_str().unwrap().starts_with("approved submittal"), "{msb}");
    // 4" beyond the equipment each side.
    assert_eq!(msb["pad_in"], json!([80.0, 44.0]));
    // The submittal comes back larger (and is given depth first): the pad grows with it.
    s.execute("bim.set_type", &json!({"mark": "MSB", "fields": {"dimensions": "40 x 80 x 90 in", "reviewStatus": "approved as noted"}})).unwrap();
    let list = s.execute("concrete.list", &json!({})).unwrap();
    let msb = list["equipment"].as_array().unwrap().iter().find(|e| e["tag"] == "MSB").unwrap();
    assert_eq!(msb["footprint_in"], json!([80.0, 40.0]), "longer side along the plan's");
    assert_eq!(msb["pad_in"], json!([88.0, 48.0]));
    // A rejected submittal isn't used: the plan box is.
    s.execute("bim.set_type", &json!({"mark": "MSB", "fields": {"reviewStatus": "revise and resubmit"}})).unwrap();
    let list = s.execute("concrete.list", &json!({})).unwrap();
    let msb = list["equipment"].as_array().unwrap().iter().find(|e| e["tag"] == "MSB").unwrap();
    assert_eq!((msb["footprint_source"].as_str(), &msb["pad_in"]), (Some("plan"), &json!([80.0, 44.0])));
    // A pad fixed by a detail is checked against the footprint.
    s.execute("bim.set_type", &json!({"mark": "MSB", "fields": {"dimensions": "96 x 40 x 90 in", "reviewStatus": "approved"}})).unwrap();
    s.execute(
        "concrete.set",
        &json!({"pad": {"equipment": "MSB", "required": true, "basis": "E-501/1", "owner": "electrical", "size_in": [84, 44]}}),
    )
    .unwrap();
    let r = s.execute("concrete.check", &json!({"equipment": "MSB"})).unwrap();
    let size = found(&r, "MSB", "pad size");
    assert_eq!(size.len(), 1, "{r}");
    assert!(size[0]["message"].as_str().unwrap().contains("needs 102\" x 46\""), "{}", size[0]["message"]);
}

#[test]
fn quantities_match_hand_calculations() {
    let mut s = session();
    let q = s.execute("concrete.qty", &json!({})).unwrap();
    // MSB pad: 80" x 44" x 4" = 8.148 cu ft = 0.302 cy; under a yard: 10% waste.
    let pad = element(&q, "PAD-MSB");
    assert!(near(&pad["cy"], 0.30) && near(&pad["cy_with_waste"], 0.33), "{pad}");
    assert_eq!(pad["scope"], "ours");
    // Forms: 20.67 ft round, 4" high; chamfer strip the same length.
    assert!(near(&pad["edge_lf"], 20.67) && near(&pad["forms_sf"], 6.89) && near(&pad["chamfer_lf"], 20.67), "{pad}");
    // #4 @ 12" each way, 1.5" cover: 4 bars of 6.42 ft and 7 of 3.42 ft = 49.58 ft, 33.12 lb.
    assert_eq!(pad["rebar"], json!([{"bar": "#4", "lf": 49.58, "lb": 33.12}]));
    // Anchor bolts (the rules' four) and the service's four 4" stub-ups.
    let embeds = pad["embeds"].as_array().unwrap();
    assert!(embeds.iter().any(|e| e["kind"] == "anchor_bolt" && e["count"] == 4), "{pad}");
    assert!(embeds.iter().any(|e| e["kind"] == "stub_up" && e["count"] == 4 && e["size"] == "4\""), "{pad}");
    // Slab: 40' x 30' x 5" = 500 cu ft = 18.52 cy (+5%: 19.44); edge forms 140 ft x 5"; mesh + 10% lap.
    let sog = element(&q, "SOG-1");
    assert!(near(&sog["cy"], 18.52) && near(&sog["cy_with_waste"], 19.44), "{sog}");
    assert!(near(&sog["edge_lf"], 140.0) && near(&sog["forms_sf"], 58.33) && near(&sog["mesh_sf"], 1320.0), "{sog}");
    assert_eq!(sog["scope"], "by others (concrete)");
    // Strip footing: 40' x 2' x 1' = 2.96 cy, cast against earth (no forms); 3 #5 bars of
    // 39.5 ft, each with 2 laps of 25" from 20 ft stock: 131 ft, 136.63 lb.
    let f1 = element(&q, "F-1");
    assert!(near(&f1["cy"], 2.96) && near(&f1["forms_sf"], 0.0), "{f1}");
    assert_eq!(f1["rebar"], json!([{"bar": "#5", "lf": 131.0, "lb": 136.63}]));
    // Column footing 4' x 4' x 18": 24 cu ft = 0.89 cy; formed sides 16 ft x 1.5 ft = 24 sq ft.
    let f2 = element(&q, "F-2");
    assert!(near(&f2["cy"], 0.89) && near(&f2["forms_sf"], 24.0), "{f2}");
    // Ours only: the electrical pad.
    let ours = s.execute("concrete.qty", &json!({"scope": "ours"})).unwrap();
    assert_eq!(ours["elements"].as_array().unwrap().len(), 1);
    assert!(near(&ours["totals"]["cy_with_waste"], 0.33));
    assert!(
        ours["csv"].as_str().unwrap().starts_with("Scope,Level,Element,Kind,Item,Size,Quantity,Unit\nours,L1,PAD-MSB,pad,Concrete (with waste)"),
        "{}",
        ours["csv"]
    );
    // On the MEP takeoff: ours as Concrete, the rest listed as by others.
    let m = s.execute("mep.qty", &json!({})).unwrap();
    let rows = m["concrete"].as_array().unwrap();
    assert!(rows.iter().any(|r| r["category"] == "Concrete" && r["element"] == "PAD-MSB" && near(&r["quantity"], 0.33)), "{m}");
    assert!(rows.iter().any(|r| r["category"] == "Concrete, by others (concrete)" && r["element"] == "SOG-1"));
    assert!(m["csv"].as_str().unwrap().contains("\nConcrete,L1,PAD-MSB,pad concrete (with waste)"), "{}", m["csv"]);
    // Bad scope.
    assert!(s.execute("concrete.qty", &json!({"scope": "mine"})).is_err());
}

#[test]
fn hold_points_warn_before_and_cost_after_a_pour() {
    let mut s = session();
    // Five days before the footing pour: no encased electrode anywhere, and the sleeve and anchor
    // bolts in F-2 not signed off.
    let r = s.execute("concrete.check", &json!({"today": "2026-03-05"})).unwrap();
    let ufer = found(&r, "F-1", "ufer");
    assert_eq!(ufer.len(), 1, "{:#}", r["findings"]);
    assert!(ufer[0]["message"].as_str().unwrap().contains("No concrete-encased electrode"), "{}", ufer[0]["message"]);
    let due: Vec<&Value> = found(&r, "F-2", "hold point");
    assert_eq!(due.len(), 2, "{due:?}");
    assert!(due.iter().all(|f| f["severity"] == "warning"));
    // The checklist of the footing pour.
    let h = s.execute("concrete.holds", &json!({"pour": "P-1", "today": "2026-03-05"})).unwrap();
    let p1 = &h["pours"][0];
    assert_eq!((p1["days_to_pour"].as_i64(), p1["ready"].as_bool()), (Some(5), Some(false)));
    let items = p1["items"].as_array().unwrap();
    let kinds: Vec<&str> = items.iter().map(|i| i["kind"].as_str().unwrap()).collect();
    assert_eq!(kinds, ["sleeve", "anchor_bolt", "ufer"], "{p1}");
    // The electrode isn't drawn: what replaces it is already priced (a ring round the
    // foundations: 40' x 32' = 144 ft at the default rate).
    let u = items.iter().find(|i| i["kind"] == "ufer").unwrap();
    assert_eq!(u["status"], "not drawn");
    assert!(u["alternative"].as_str().unwrap().starts_with("A ground ring"), "{u}");
    assert!(near(&u["cost"], 5040.0), "{u}");
    // The slab's checklist has the pad's anchor bolts and the under-slab conduit.
    let h2 = s.execute("concrete.holds", &json!({"pour": "P-2"})).unwrap();
    let items2 = h2["pours"][0]["items"].as_array().unwrap();
    assert!(items2.iter().any(|i| i["kind"] == "conduit" && i["source"] == "under-slab run E-UG" && near(&i["length_ft"], 20.0)), "{h2}");
    assert!(items2.iter().any(|i| i["element"] == "PAD-MSB" && i["kind"] == "anchor_bolt"));
    // Add the electrode and sign everything off: the pour is ready.
    let mut f1 = spec()["concrete"]["elements"][1].clone();
    f1["embeds"] = json!([{"kind": "ufer", "size": "#4 bar", "length_ft": 20, "depth_in": 9, "source": "E-001 grounding note"}]);
    s.execute("concrete.set", &json!({"element": f1})).unwrap();
    for item in ["F-2|sleeve|4\"", "F-2|anchor_bolt|3/4\" dia", "F-1|ufer|#4 bar"] {
        s.execute("concrete.signoff", &json!({"pour": "P-1", "item": item, "by": "Inspector A", "on": "2026-03-09"})).unwrap();
    }
    let h = s.execute("concrete.holds", &json!({"pour": "P-1", "today": "2026-03-09"})).unwrap();
    assert_eq!(h["pours"][0]["ready"], true, "{h}");
    assert!(h["pours"][0]["items"].as_array().unwrap().iter().all(|i| i["status"] == "signed" && i["signed_by"] == "Inspector A"));
    // Take one sign-off back, then pass the pour date: missed, with its alternative and cost.
    s.execute("concrete.signoff", &json!({"pour": "P-1", "item": "F-1|ufer|#4 bar", "undo": true})).unwrap();
    let r = s.execute("concrete.check", &json!({"today": "2026-03-12"})).unwrap();
    let missed: Vec<&Value> = found(&r, "F-1", "hold point");
    assert_eq!(missed.len(), 1, "{:#}", r["findings"]);
    assert_eq!(missed[0]["severity"], "error");
    assert!(missed[0]["message"].as_str().unwrap().contains("ground ring"), "{}", missed[0]["message"]);
    // Bad input.
    assert!(s.execute("concrete.check", &json!({"today": "next week"})).is_err());
    assert!(s.execute("concrete.signoff", &json!({"pour": "P-1", "item": "nothing", "by": "Inspector A"})).is_err());
    assert!(s.execute("concrete.signoff", &json!({"pour": "P-1", "item": "F-2|sleeve|4\""})).is_err(), "who signed is required");
    assert!(s.execute("concrete.set", &json!({"pour": {"id": "P-9", "date": "31/12/2026"}})).is_err());
}

#[test]
fn rebar_embeds_and_clearance_are_checked() {
    let mut s = session();
    let r = s.execute("concrete.check", &json!({})).unwrap();
    // The 4" sleeve at 12" deep runs through F-2's bottom mat (#5 each way, 3" cover).
    let clash = found(&r, "F-2", "clash");
    assert_eq!(clash.len(), 1, "{:#}", r["findings"]);
    assert!(clash[0]["message"].as_str().unwrap().contains("bottom mat (#5)"), "{}", clash[0]["message"]);
    // 18" anchor bolts in an 18" footing.
    assert_eq!(found(&r, "F-2", "embed").len(), 1);
    // A short electrode in a pad: too short and in the wrong element.
    let mut pad = json!({"id": "PAD-X", "kind": "pad", "level": "L1", "min": [400, 0], "max": [448, 24], "depth_in": 6, "top_ft": 0.5, "owner": "electrical",
        "embeds": [{"kind": "ufer", "size": "#4 bar", "length_ft": 8}, {"kind": "conduit", "size": "1\"", "at": [500, 500]}]});
    s.execute("concrete.set", &json!({"element": pad.clone()})).unwrap();
    let r = s.execute("concrete.check", &json!({})).unwrap();
    assert_eq!(found(&r, "PAD-X", "ufer").len(), 2, "{:#}", r["findings"]);
    assert_eq!(found(&r, "PAD-X", "embed").len(), 1, "drawn outside the pad");
    // Moved deeper than the cover allows.
    pad["embeds"] = json!([{"kind": "conduit", "size": "2\"", "depth_in": 5}]);
    s.execute("concrete.set", &json!({"element": pad})).unwrap();
    let r = s.execute("concrete.check", &json!({})).unwrap();
    assert_eq!(found(&r, "PAD-X", "cover").len(), 1, "{:#}", r["findings"]);
    // A handhole base in front of the switchboard: with its front given, the working space is blocked.
    s.execute(
        "concrete.set",
        &json!({"element": {"id": "HH-1", "kind": "handhole_base", "level": "L1", "min": [10, 40], "max": [30, 60], "depth_in": 6, "top_ft": 0.5, "owner": "electrical"},
                "pad": {"equipment": "MSB", "required": true, "basis": "26 24 13 2.4", "owner": "electrical", "front": "n"}}),
    )
    .unwrap();
    let r = s.execute("concrete.check", &json!({"equipment": "MSB"})).unwrap();
    let c = found(&r, "MSB", "clearance");
    assert_eq!(c.len(), 1, "{r}");
    assert!(c[0]["message"].as_str().unwrap().contains("HH-1"));
    // Worked from the south: clear.
    s.execute("concrete.set", &json!({"pad": {"equipment": "MSB", "required": true, "basis": "26 24 13 2.4", "owner": "electrical", "front": "s"}}))
        .unwrap();
    assert!(found(&s.execute("concrete.check", &json!({"equipment": "MSB"})).unwrap(), "MSB", "clearance").is_empty());
}

#[test]
fn review_raises_rfis_once() {
    let mut s = session();
    let r = s.execute("concrete.review", &json!({"today": "2026-03-05"})).unwrap();
    let ids = r["rfis"].as_array().unwrap().len();
    assert!(ids >= 5, "{r}");
    let list = s.execute("rfi.list", &json!({})).unwrap();
    let kinds: Vec<&str> = list["items"].as_array().unwrap().iter().filter_map(|i| i["kind"].as_str()).collect();
    for k in ["pad_undecided", "pour_hold", "embed_conflict"] {
        assert!(kinds.contains(&k), "{k} in {kinds:?}");
    }
    let again = s.execute("concrete.review", &json!({"today": "2026-03-05"})).unwrap();
    assert!(again["rfis"].as_array().unwrap().is_empty(), "no duplicates: {again}");
}

#[test]
fn concrete_builds_in_3d_with_the_gear_on_its_pad() {
    let mut s = session();
    let b = s.execute("model.build", &json!({})).unwrap();
    println!("{b}");
    // Slab, two footings and the switchboard's and transformer's pads (required; the
    // transformer's still needs an owner).
    assert_eq!(b["concrete"], 5, "{b}");
    assert!(b["rebarBars"].as_u64().unwrap() > 10, "{b}");
    // F-2's sleeve and anchor bolts and the pads' anchor bolts (the service's stub-ups are the
    // MEP build's raceways).
    assert_eq!(b["concreteEmbeds"], 4, "{b}");
    // The switchboard stands on its 4" pad.
    let d = s.doc().unwrap();
    let msb = d
        .model
        .iter()
        .find(|e| e.common.xdata.iter().any(|t| t.value == "tag:MSB") && e.common.xdata.iter().any(|t| t.value == "kind:equipment"))
        .unwrap();
    let cadaddy_doc::EntityKind::Mesh(m) = &msb.kind else { panic!("not a mesh") };
    let zmin = m.vertices.iter().map(|v| v.z).fold(f64::INFINITY, f64::min);
    assert!((zmin - 4.0).abs() < 1e-9, "{zmin}");
    // IFC: the slab, the footings (strip and pad), the housekeeping pads, rebar and anchor bolts.
    let ifc = String::from_utf8(cadaddy_io::ifc::write_ifc(d, "Sample Hotel").unwrap()).unwrap();
    for (class, pre) in [
        ("IFCSLAB", ".BASESLAB."),
        ("IFCFOOTING", ".STRIP_FOOTING."),
        ("IFCFOOTING", ".PAD_FOOTING."),
        ("IFCREINFORCINGBAR", ".MAIN."),
        ("IFCMECHANICALFASTENER", ".ANCHORBOLT."),
    ] {
        assert!(ifc.lines().any(|l| l.contains(&format!("={class}(")) && l.contains(pre)), "{class} {pre}");
    }
    assert!(ifc.lines().any(|l| l.contains("=IFCSLAB(") && l.contains("'PAD-MSB'")), "the pad by its id");
    if let Ok(dir) = std::env::var("CONCRETE_SHOTS") {
        for (name, style, around) in [
            ("concrete-3d.png", "shaded", json!([-24, -36, 260, 260])),
            ("concrete-footing.png", "wireframe", json!([190, 190, 260, 260])),
            ("concrete-pad.png", "wireframe", json!([-12, -12, 84, 48])),
        ] {
            let png = std::path::Path::new(&dir).join(name);
            s.execute("model.render", &json!({"path": png.display().to_string(), "style": style, "width": 900, "height": 600, "around": around}))
                .unwrap();
        }
    }
}

#[test]
fn hostile_concrete_never_panics() {
    let mut s = session();
    let mut sp = spec();
    sp["concrete"]["elements"] = json!([
        {"id": "X", "kind": "slab", "level": "L1", "min": [0, 0], "max": [1e308, -1e308], "depth_in": 1e308, "rebar": [{"bar": "#4", "spacing_in": 1e-300}],
         "mesh": "x", "depressions": [{"min": [0, 0], "max": [1e308, 1], "depth_in": f64::MAX}], "thickened_edge_in": [1e308, 1e308]},
        {"id": "Y", "kind": "grade_beam", "level": "nowhere", "path": [[0, 0]], "width_in": -5, "depth_in": -1,
         "rebar": [{"bar": "#99", "direction": "ties", "count": 4_000_000_000u64}], "embeds": [{"kind": "ufer", "count": 4_000_000_000u64, "length_ft": 1e308}]},
        {"id": "Z", "kind": "pier", "level": "L1", "min": [0, 0], "max": [24, 24], "round": true, "depth_in": 1e9, "top_ft": 0,
         "rebar": [{"bar": "#8", "direction": "long", "count": 4_000_000_000u64}, {"bar": "#4", "direction": "ties", "spacing_in": 0}],
         "embeds": [{"kind": "anchor_bolt", "count": 4_000_000_000u64, "embed_in": 1e308}, {"kind": "sleeve", "size": "1/0\""}]}
    ]);
    sp["concrete"]["pads"] =
        json!([{"equipment": "MSB", "required": true, "owner": "electrical", "size_in": [1e308, -1], "margin_in": f64::MAX, "height_in": 0}]);
    sp["concrete"]["pours"] = json!([{"id": "P-1", "date": "9999-12-31"}]);
    sp["concrete"]["rules"] =
        json!({"waste_pct": -5, "stock_ft": 0, "lap_db": 1e308, "cover_in": {"slab": "deep"}, "pad_rebar": "none", "our_scope": [1, 2]});
    s.execute("bspec.set", &json!({"replace": true, "spec": sp})).unwrap();
    for (cmd, p) in [
        ("concrete.list", json!({})),
        ("concrete.check", json!({"today": "0001-01-01"})),
        ("concrete.qty", json!({})),
        ("concrete.holds", json!({"today": "9999-12-31"})),
        ("concrete.review", json!({})),
        ("model.build", json!({})),
        ("mep.qty", json!({})),
    ] {
        let r = s.execute(cmd, &p);
        assert!(r.is_ok(), "{cmd}: {r:?}");
        if let Ok(v) = r {
            assert!(!v.to_string().contains("NaN"), "{cmd}");
        }
    }
    for p in
        [json!({"element": {"kind": "slab"}}), json!({"pad": {"required": true}}), json!({"rules": 5}), json!({"element": {"id": "Q", "rebar": "x"}})]
    {
        assert!(s.execute("concrete.set", &p).is_err(), "{p}");
    }
}
