//! Every model element is an object with properties (`bim.props.*`): one store per element
//! (key → value, unit, source, confidence, note), standard keys per kind, saved in .cdy / DXF,
//! exported to IFC as property sets and read back, edits by a person kept through rebuilds.

use cadaddy_engine::Session;
use cadaddy_engine::cmd::elemprops::element_kind;
use cadaddy_engine::cmd::wallgen::made_as;
use cadaddy_engine::doc::props::{self, SourceKind};
use cadaddy_engine::doc::{Drawing, Entity, EntityKind, Handle, Line, RawTag};
use cadaddy_engine::geom::Vec3;
use serde_json::{Value, json};

/// Room A at x 0–20', room B at x 100–120' (feet; the drawing is in inches).
const B: f64 = 100.0;

fn line(d: &mut Drawing, layer: &str, a: (f64, f64), b: (f64, f64)) -> Handle {
    let p = |(x, y): (f64, f64)| Vec3 { x: x * 12.0, y: y * 12.0, z: 0.0 };
    let h = Handle(d.handseed.max(0x100));
    d.handseed = h.0 + 1;
    let mut e = Entity::new(h, EntityKind::Line(Line { a: p(a), b: p(b) }));
    e.common.layer = layer.into();
    d.model.push(e);
    h
}

/// A 20' × 10' room of 6" walls with its south-west corner at (x0, 0).
fn room(d: &mut Drawing, x0: f64) {
    let o = [((0., 0.), (20., 0.)), ((20., 0.), (20., 10.)), ((20., 10.), (0., 10.)), ((0., 10.), (0., 0.))];
    let i = [((0.5, 0.5), (19.5, 0.5)), ((19.5, 0.5), (19.5, 9.5)), ((19.5, 9.5), (0.5, 9.5)), ((0.5, 9.5), (0.5, 0.5))];
    for ((ax, ay), (bx, by)) in o.into_iter().chain(i) {
        line(d, "A-WALL", (ax + x0, ay), (bx + x0, by));
    }
}

fn rules() -> Value {
    json!({
        "units_per_foot": 12,
        "levels": [{"name": "L1", "elevation_ft": 0, "height_ft": 9.5, "ceiling_ft": 8, "base": [0, 0]}],
        "layers": [{"layer": "A-WALL", "kind": "wall"}]
    })
}

fn build(s: &mut Session) {
    s.execute("wallgen", &json!({"rules": rules(), "auto": true})).unwrap();
}

fn built() -> Session {
    let mut d = Drawing::new_imperial();
    room(&mut d, 0.0);
    room(&mut d, B);
    let mut s = Session::empty();
    s.open_drawing(d, "Sample Floor Plan", None);
    build(&mut s);
    s
}

/// The generated walls, west to east then south to north (by their centre).
fn walls(s: &Session) -> Vec<Handle> {
    let d = s.doc().unwrap();
    let mut v: Vec<(f64, f64, Handle)> = d
        .model
        .iter()
        .filter(|e| made_as(e).as_deref() == Some("wall"))
        .map(|e| {
            let b = cadaddy_engine::doc::entity_bounds(d, e, 0);
            (b.center().x, b.center().y, e.handle)
        })
        .collect();
    v.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.total_cmp(&b.1)));
    v.into_iter().map(|x| x.2).collect()
}

/// The wall of room A whose centre is nearest (x, y) feet.
fn wall_near(s: &Session, x: f64, y: f64) -> Handle {
    let d = s.doc().unwrap();
    d.model
        .iter()
        .filter(|e| made_as(e).as_deref() == Some("wall"))
        .map(|e| (cadaddy_engine::doc::entity_bounds(d, e, 0).center().dist(cadaddy_engine::geom::Vec2::new(x * 12.0, y * 12.0)), e.handle))
        .min_by(|a, b| a.0.total_cmp(&b.0))
        .unwrap()
        .1
}

/// A generated element of another kind, made like the build makes them: a copy of a wall's body
/// with a build tag `[part, level, source, tags…]` on `layer`.
fn element(s: &mut Session, layer: &str, tags: &[&str]) -> Handle {
    let w = walls(s)[0];
    let d = s.doc_mut().unwrap();
    let mut e = (**d.entity(w).unwrap()).clone();
    e.handle = d.new_handle();
    e.common.layer = layer.into();
    let mut x = vec![RawTag { code: 1001, value: "CADADDY_WALLGEN".into() }];
    x.extend(tags.iter().map(|t| RawTag { code: 1000, value: (*t).into() }));
    e.common.xdata = x.into();
    let h = e.handle;
    d.ensure_layer(layer);
    d.model.push(e);
    h
}

fn get(s: &mut Session, h: Handle) -> Value {
    s.execute("bim.props.get", &json!({"handle": h.hex()})).unwrap()["elements"][0].clone()
}

fn stored(s: &Session, h: Handle) -> props::Props {
    props::read(&s.doc().unwrap().entity(h).unwrap().common.xdata)
}

#[test]
fn every_kind_of_element_holds_one_store_with_its_standard_keys() {
    let mut s = built();
    let wall = walls(&s)[0];
    let dev = element(&mut s, "E-POWER", &["device", "L1", "0", "type:Duplex"]);
    let light = element(&mut s, "E-LIGHTING", &["device", "L1", "0"]);
    let duct = element(&mut s, "M-DUCT", &["mep", "L1", "0", "kind:duct", "system:Supply Air"]);
    let pipe = element(&mut s, "P-PIPE", &["mep", "L1", "0", "kind:pipe", "system:CW"]);
    let conduit = element(&mut s, "E-COND", &["mep", "L1", "0", "kind:conduit"]);
    let ahu = element(&mut s, "M-EQPM", &["mep", "L1", "0", "kind:duct", "part:equipment:ahu"]);
    let door = element(&mut s, "3D-Doors", &["door", "L1", "0"]);
    let d = s.doc().unwrap();
    let kind = |h: Handle| element_kind(d.entity(h).unwrap()).unwrap();
    assert_eq!(
        [wall, dev, light, duct, pipe, conduit, ahu, door].map(kind),
        ["wall", "device", "light_fixture", "duct", "pipe", "conduit", "equipment", "door"].map(String::from)
    );
    // The sheet's own lines are not elements.
    let plan_line = d.model.iter().find(|e| made_as(e).is_none()).unwrap().handle;
    assert!(element_kind(d.entity(plan_line).unwrap()).is_none());
    assert!(s.execute("bim.props.get", &json!({"handle": plan_line.hex()})).is_err());

    // A device's standard set, all missing but its level (computed from the build's tag).
    let g = get(&mut s, dev);
    assert_eq!(g["kind"], "device");
    assert_eq!(g["standard"], json!(["type", "type_mark", "mount_height", "circuit", "panel", "voltage", "room", "level", "host_wall", "sheet"]));
    assert_eq!(g["props"]["level"]["value"], "L1");
    assert_eq!(g["props"]["level"]["source"]["kind"], "computed");
    assert_eq!(g["props"]["level"]["stored"], false);
    assert!(g["missing"].as_array().unwrap().contains(&json!("circuit")));
    assert!(!g["missing"].as_array().unwrap().contains(&json!("level")));

    // Set the outlet's values: they are the user's, stored on the element, in order.
    let r = s
        .execute("bim.props.set", &json!({"handle": dev.hex(), "props": {"type": "duplex", "mount_height": 18, "circuit": "G-12", "room": "131"}}))
        .unwrap();
    assert_eq!(r["set"], 1);
    let g = get(&mut s, dev);
    assert_eq!(g["props"]["mount_height"]["value"], 18);
    assert_eq!(g["props"]["circuit"]["source"]["kind"], "user");
    assert_eq!(g["props"]["circuit"]["stored"], true);
    // In the order they were set (one call's `props` object comes in key order); a later key goes last.
    s.execute("bim.props.set", &json!({"handle": dev.hex(), "key": "panel", "value": "G"})).unwrap();
    s.execute("bim.props.set", &json!({"handle": dev.hex(), "key": "circuit", "value": "G-12"})).unwrap();
    let g = get(&mut s, dev);
    let order: Vec<&str> = g["order"].as_array().unwrap().iter().filter_map(Value::as_str).collect();
    assert_eq!(order, ["circuit", "mount_height", "room", "type", "panel", "level"]);
    let st = stored(&s, dev);
    assert_eq!(st.get("circuit").map(|p| p.source.kind), Some(SourceKind::User));
    // A unit and a note ride along; null clears (the user says it has none).
    s.execute("bim.props.set", &json!({"handle": dev.hex(), "key": "mount_height", "value": 48, "unit": "in", "note": "above counter"})).unwrap();
    let g = get(&mut s, dev);
    assert_eq!((g["props"]["mount_height"]["value"].clone(), g["props"]["mount_height"]["unit"].clone()), (json!(48), json!("in")));
    assert_eq!(g["props"]["mount_height"]["note"], "above counter");
    s.execute("bim.props.set", &json!({"handle": dev.hex(), "key": "room", "value": null})).unwrap();
    assert!(get(&mut s, dev)["missing"].as_array().unwrap().contains(&json!("room")));
}

#[test]
fn find_and_summary_count_values_and_sources_per_kind() {
    let mut s = built();
    let ws = walls(&s);
    assert_eq!(ws.len(), 8);
    let devs: Vec<Handle> = (0..3).map(|_| element(&mut s, "E-POWER", &["device", "L1", "0"])).collect();
    s.execute("bim.props.set", &json!({"handles": [ws[0].hex(), ws[1].hex()], "key": "fire_rating", "value": "1 HR"})).unwrap();
    s.execute("bim.props.set", &json!({"handle": devs[0].hex(), "props": {"circuit": "G-12", "type": "duplex"}})).unwrap();
    s.execute("bim.props.set", &json!({"handle": devs[1].hex(), "key": "circuit", "value": "G-14"})).unwrap();
    // A reader's value (as propfill writes it): source print with the sheet and the place.
    s.doc_mut()
        .unwrap()
        .modify_entity(devs[2], |e| {
            let mut p = props::read(&e.common.xdata);
            p.set("circuit", props::Prop::new("G-12", props::Source::print("E-101", Some([10.0, 20.0]))));
            p.set("mount_height", props::Prop::new(18, props::Source::of(SourceKind::Default)).with_unit("in"));
            e.common.xdata = props::write(&e.common.xdata, &p).into();
        })
        .unwrap();

    let f = |s: &mut Session, p: Value| s.execute("bim.props.find", &p).unwrap();
    assert_eq!(f(&mut s, json!({"kind": "wall", "where": {"fire_rating": "exists"}}))["count"], 2);
    assert_eq!(f(&mut s, json!({"kind": "wall", "where": {"fire_rating": "missing"}}))["count"], 6);
    assert_eq!(f(&mut s, json!({"where": {"fire_rating": "1 hr"}}))["count"], 2, "values match without regard to case");
    let g12 = f(&mut s, json!({"kind": "device", "where": {"circuit": "G-12"}}));
    let mut hs: Vec<String> = g12["handles"].as_array().unwrap().iter().map(|h| h.as_str().unwrap().to_string()).collect();
    hs.sort();
    let mut want = vec![devs[0].hex(), devs[2].hex()];
    want.sort();
    assert_eq!(hs, want);
    assert_eq!(f(&mut s, json!({"kind": "device", "where": {"mount_height": 18}}))["count"], 1, "numbers match numbers");
    assert_eq!(f(&mut s, json!({"kind": "device", "where": {"level": "L1", "circuit": {"missing": true}}}))["count"], 0);
    f(&mut s, json!({"kind": "device", "select": true}));
    assert_eq!(s.selection().len(), 3);
    assert!(s.execute("bim.props.find", &json!({"where": [1]})).is_err());

    let sum = s.execute("bim.props.summary", &json!({})).unwrap();
    let dv = &sum["kinds"]["device"];
    assert_eq!(dv["count"], 3);
    assert_eq!(dv["filled"]["circuit"], 3);
    assert_eq!(dv["filled"]["panel"], 0, "standard keys are listed even with no value");
    assert_eq!(dv["filled"]["level"], 3);
    assert_eq!(dv["sources"]["circuit"], json!({"user": 2, "print": 1}));
    assert_eq!(dv["sources"]["mount_height"], json!({"default": 1}));
    assert_eq!(dv["sources"]["level"], json!({"computed": 3}));
    let wl = &sum["kinds"]["wall"];
    assert_eq!((wl["count"].clone(), wl["filled"]["fire_rating"].clone()), (json!(8), json!(2)));
    assert!(sum["kinds"]["floor"]["count"].as_u64().unwrap() >= 1, "{sum}");
    assert_eq!(s.execute("bim.props.summary", &json!({"kind": "wall"})).unwrap()["kinds"].as_object().unwrap().len(), 1);

    // set by filter: every wall missing a rating gets the user's.
    let r = s
        .execute("bim.props.set", &json!({"filter": {"kind": "wall", "where": {"fire_rating": "missing"}}, "key": "fire_rating", "value": "0 HR"}))
        .unwrap();
    assert_eq!(r["set"], 6);
    assert_eq!(f(&mut s, json!({"kind": "wall", "where": {"fire_rating": "missing"}}))["count"], 0);
}

#[test]
fn user_edits_survive_a_rebuild_and_a_clear_and_build_and_revert_puts_back_the_build() {
    let mut s = built();
    let south = wall_near(&s, 10.0, 0.25);
    let east_b = wall_near(&s, B + 19.75, 5.0);
    s.execute("bim.props.set", &json!({"handle": south.hex(), "props": {"fire_rating": "2 HR", "type": "W3"}})).unwrap();
    s.execute("bim.props.set", &json!({"handle": east_b.hex(), "key": "finish_a", "value": "paint"})).unwrap();
    // A second edit of the same element joins its override (one per element).
    s.execute("bim.props.set", &json!({"handle": south.hex(), "key": "finish_a", "value": "tile"})).unwrap();
    let l = s.execute("override.list", &json!({"kind": "props"})).unwrap();
    assert_eq!(l["overrides"].as_array().unwrap().len(), 2, "{l}");
    let south_ov = l["overrides"].as_array().unwrap().iter().find(|o| o["element"] == south.hex()).unwrap().clone();
    assert_eq!(south_ov["props"], json!({"fire_rating": "2 HR", "type": "W3", "finish_a": "tile"}));

    // Rebuild: new elements, the same values on them.
    build(&mut s);
    let south2 = wall_near(&s, 10.0, 0.25);
    assert!(s.doc().unwrap().entity(south).is_none(), "the build made new elements");
    let g = get(&mut s, south2);
    assert_eq!((g["props"]["fire_rating"]["value"].clone(), g["props"]["fire_rating"]["source"]["kind"].clone()), (json!("2 HR"), json!("user")));
    assert_eq!(g["props"]["finish_a"]["value"], "tile");
    let eb = wall_near(&s, B + 19.75, 5.0);
    assert_eq!(get(&mut s, eb)["props"]["finish_a"]["value"], "paint");
    // Others stay without.
    assert_eq!(s.execute("bim.props.find", &json!({"kind": "wall", "where": {"fire_rating": "exists"}})).unwrap()["count"], 1);

    // The model cleared, then built again (the guided job does this when an answer changes a stage).
    s.execute("wallgen", &json!({"clear": true})).unwrap();
    assert_eq!(s.execute("bim.props.summary", &json!({})).unwrap()["elements"], 0);
    build(&mut s);
    let south3 = wall_near(&s, 10.0, 0.25);
    assert_eq!(get(&mut s, south3)["props"]["type"]["value"], "W3");
    // Building twice changes nothing more (no duplicate overrides, the same store).
    let before = stored(&s, south3);
    build(&mut s);
    assert_eq!(stored(&s, wall_near(&s, 10.0, 0.25)), before);
    assert_eq!(s.execute("override.list", &json!({"kind": "props"})).unwrap()["overrides"].as_array().unwrap().len(), 2);

    // Revert: the element is as the build made it again.
    s.execute("override.revert", &json!({"id": south_ov["id"]})).unwrap();
    let w = wall_near(&s, 10.0, 0.25);
    let g = get(&mut s, w);
    assert!(g["props"].get("fire_rating").is_none(), "{g}");
    build(&mut s);
    let w = wall_near(&s, 10.0, 0.25);
    assert!(get(&mut s, w)["props"].get("type").is_none());
}

#[test]
fn properties_are_saved_in_cdy_and_dxf() {
    let mut s = built();
    let w = walls(&s)[2];
    let dev = element(&mut s, "E-POWER", &["device", "L1", "0"]);
    s.execute("bim.props.set", &json!({"handle": w.hex(), "props": {"fire_rating": "1 HR", "assembly": "Typ. 5/8\" GWB — both sides"}})).unwrap();
    s.execute("bim.props.set", &json!({"handle": dev.hex(), "props": {"circuit": "G-12", "mount_height": 18, "gfci": true}})).unwrap();
    let d = s.doc().unwrap().clone();
    for name in ["Sample Floor Plan.cdy", "Sample Floor Plan.dxf"] {
        let bytes = cadaddy_io::write(&d, name).unwrap();
        let back = cadaddy_io::read(&bytes, name).unwrap();
        for h in [w, dev] {
            let a = props::read(&d.entity(h).unwrap().common.xdata);
            let e = back.model.iter().find(|e| e.handle == h).unwrap_or_else(|| panic!("{name}: {} lost", h.hex()));
            assert_eq!(props::read(&e.common.xdata), a, "{name}");
        }
        let mut t = Session::empty();
        t.open_drawing(back, "Sample Floor Plan", None);
        let sum = t.execute("bim.props.summary", &json!({})).unwrap();
        assert_eq!(sum["kinds"]["device"]["sources"]["circuit"], json!({"user": 1}), "{name}");
        assert_eq!(sum["kinds"]["wall"]["filled"]["fire_rating"], 1, "{name}");
    }
}

#[test]
fn ifc_export_writes_psets_and_import_reads_them_back_into_the_store() {
    let mut s = built();
    let w = wall_near(&s, 10.0, 0.25);
    s.execute("bim.props.set", &json!({"handle": w.hex(), "props": {"fire_rating": "2 HR", "type": "W3", "height": 9.5}})).unwrap();
    // A value read from a print, with its sheet and place.
    s.doc_mut()
        .unwrap()
        .modify_entity(w, |e| {
            let mut p = props::read(&e.common.xdata);
            let mut v = props::Prop::new("Room 101", props::Source::print("A-101", Some([12.0, 3.5])));
            v.confidence = Some(0.8);
            p.set("room_a", v);
            e.common.xdata = props::write(&e.common.xdata, &p).into();
        })
        .unwrap();
    let d = s.doc().unwrap().clone();
    let bytes = cadaddy_io::write(&d, "Sample Floor Plan.ifc").unwrap();
    let text = String::from_utf8_lossy(&bytes);
    assert!(text.contains("'Pset_CADaddy'"), "our set");
    assert!(text.contains("IFCPROPERTYSINGLEVALUE('FireRating',$,IFCLABEL('2 HR'),$)"), "the standard set's place for it");
    assert!(text.contains("'Pset_WallCommon'"));
    assert!(text.contains("IFCPROPERTYSINGLEVALUE('Reference',$,IFCIDENTIFIER('W3'),$)"));
    assert!(text.contains("IFCREAL(9.5)"), "numbers stay numbers");

    let back = cadaddy_io::read(&bytes, "Sample Floor Plan.ifc").unwrap();
    let with: Vec<std::sync::Arc<Entity>> =
        back.model.iter().filter(|e| props::read(&e.common.xdata).get("fire_rating").is_some()).cloned().collect();
    assert_eq!(with.len(), 1, "one wall carries the rating");
    let p = props::read(&with[0].common.xdata);
    let fr = p.get("fire_rating").unwrap();
    assert_eq!((fr.text(), fr.source.kind), ("2 HR".to_string(), SourceKind::User), "as written, not as the standard set");
    assert_eq!(p.get("height").map(|h| h.value.clone()), Some(json!(9.5)));
    let room = p.get("room_a").unwrap();
    assert_eq!(
        (room.source.kind, room.source.doc.as_deref(), room.source.at, room.confidence),
        (SourceKind::Print, Some("A-101"), Some([12.0, 3.5]), Some(0.8))
    );
    // Read back, it is an element of the model with the same summary.
    let mut t = Session::empty();
    t.open_drawing(back, "Sample Floor Plan", None);
    let sum = t.execute("bim.props.summary", &json!({"kind": "wall"})).unwrap();
    assert_eq!(sum["kinds"]["wall"]["filled"]["fire_rating"], 1, "{sum}");
    let h = with[0].handle;
    assert_eq!(t.execute("bim.props.get", &json!({"handle": h.hex()})).unwrap()["elements"][0]["props"]["type"]["value"], "W3");
}

#[test]
fn another_programs_ifc_properties_merge_into_the_same_store() {
    // A door from elsewhere: a standard set and a set of its own, no Pset_CADaddy.
    let ifc = "ISO-10303-21;\nHEADER;\nFILE_DESCRIPTION((''),'2;1');\nFILE_NAME('x.ifc','',(''),(''),'','','');\nFILE_SCHEMA(('IFC4'));\nENDSEC;\nDATA;\n\
#1=IFCCARTESIANPOINT((0.,0.,0.));\n#2=IFCAXIS2PLACEMENT3D(#1,$,$);\n#3=IFCLOCALPLACEMENT($,#2);\n\
#4=IFCCARTESIANPOINTLIST3D(((0.,0.,0.),(1.,0.,0.),(1.,0.1,0.),(0.,0.1,0.),(0.,0.,2.),(1.,0.,2.),(1.,0.1,2.),(0.,0.1,2.)));\n\
#5=IFCINDEXEDPOLYGONALFACE((1,2,3,4));\n#6=IFCINDEXEDPOLYGONALFACE((5,6,7,8));\n#7=IFCINDEXEDPOLYGONALFACE((1,2,6,5));\n\
#8=IFCPOLYGONALFACESET(#4,$,(#5,#6,#7),$);\n#9=IFCSHAPEREPRESENTATION($,'Body','Tessellation',(#8));\n#10=IFCPRODUCTDEFINITIONSHAPE($,$,(#9));\n\
#11=IFCDOOR('0Door00000000000000001',$,'Door 101',$,$,#3,#10,'101',2.1,0.9,.DOOR.,$,$);\n\
#12=IFCPROPERTYSINGLEVALUE('FireRating',$,IFCLABEL('20 MIN'),$);\n#13=IFCPROPERTYSET('0Pset0000000000000001',$,'Pset_DoorCommon',$,(#12));\n\
#14=IFCRELDEFINESBYPROPERTIES('0Rel00000000000000001',$,$,$,(#11),#13);\n\
#15=IFCPROPERTYSINGLEVALUE('Hardware',$,IFCLABEL('HW-3'),$);\n#16=IFCPROPERTYSET('0Pset0000000000000002',$,'Acme_Door',$,(#15));\n\
#17=IFCRELDEFINESBYPROPERTIES('0Rel00000000000000002',$,$,$,(#11),#16);\nENDSEC;\nEND-ISO-10303-21;\n";
    let d = cadaddy_io::read(ifc.as_bytes(), "Sample Door.ifc").unwrap();
    let door = d.model.iter().next().unwrap();
    assert_eq!(element_kind(door).as_deref(), Some("door"));
    let p = props::read(&door.common.xdata);
    let fr = p.get("fire_rating").unwrap();
    assert_eq!((fr.text(), fr.source.kind, fr.source.doc.as_deref()), ("20 MIN".into(), SourceKind::Spec, Some("IFC Pset_DoorCommon")));
    assert_eq!(p.get("Acme_Door.Hardware").map(props::Prop::text).as_deref(), Some("HW-3"));
    // The reference reader sees the same values.
    let t = cadaddy_engine::cmd::ifcref::tag(door).unwrap();
    assert!(t.props.iter().any(|(k, v)| k == "fire_rating" && v == "20 MIN"), "{:?}", t.props);
}

#[test]
fn hostile_calls_never_panic_and_refuse_clearly() {
    let mut s = built();
    let w = walls(&s)[0];
    let bad = [
        json!({}),
        json!({"handle": w.hex()}),
        json!({"handle": w.hex(), "key": ""}),
        json!({"handle": w.hex(), "key": 5, "value": 1}),
        json!({"handle": w.hex(), "key": "a"}),
        json!({"handle": w.hex(), "key": "a", "value": {"x": 1}}),
        json!({"handle": w.hex(), "key": "a", "value": [1]}),
        json!({"handle": w.hex(), "props": "x"}),
        json!({"handle": w.hex(), "props": {"": 1}}),
        json!({"handle": w.hex(), "key": "k".repeat(500), "value": 1}),
        json!({"handle": "ZZZ", "key": "a", "value": 1}),
        json!({"handle": "FFFFFF", "key": "a", "value": 1}),
        json!({"filter": 5, "key": "a", "value": 1}),
        json!({"filter": {"where": {"a": [1]}}, "key": "a", "value": 1}),
        json!({"filter": {"kind": "nothing"}, "key": "a", "value": 1}),
    ];
    for p in &bad {
        assert!(s.execute("bim.props.set", p).is_err(), "{p}");
    }
    for p in [json!({"handle": "ZZ"}), json!({"handles": 5}), json!({})] {
        assert!(s.execute("bim.props.get", &p).is_err(), "{p}");
    }
    assert!(s.execute("bim.props.find", &json!({"where": "x"})).is_err());
    assert!(s.execute("bim.props.find", &json!({"limit": -1})).is_ok());
    // A long text value is capped, not refused.
    let long = "x".repeat(50_000);
    s.execute("bim.props.set", &json!({"handle": w.hex(), "key": "note", "value": long})).unwrap();
    assert_eq!(stored(&s, w).get("note").unwrap().text().len(), props::MAX_TEXT);
    // An empty drawing: a summary that says what to do.
    let mut e = Session::empty();
    e.open_drawing(Drawing::new_imperial(), "Empty", None);
    let r = e.execute("bim.props.summary", &json!({})).unwrap();
    assert_eq!(r["elements"], 0);
    assert!(r["next_step"].as_str().unwrap().contains("model.build"));
}

#[test]
fn a_room_volume_shows_its_tag_and_area_and_keeps_user_edits_through_rooms_build() {
    let mut d = Drawing::new_imperial();
    room(&mut d, 0.0);
    let mut s = Session::empty();
    s.open_drawing(d, "Sample Floor Plan", None);
    s.execute("layer.new", &json!({"name": "A-ROOM-TAG", "current": true})).unwrap();
    s.execute("text", &json!({"at": [120, 60], "text": "OFFICE 101", "height": 6})).unwrap();
    let spec = json!({
        "version": 1, "units_per_foot": 12,
        "levels": [{"name": "L1", "elevation_ft": 0, "height_ft": 10, "ceiling_ft": 9, "base": [0, 0]}],
        "layers": [{"layer": "A-WALL", "role": "wall"}, {"layer": "A-ROOM-TAG", "role": "room_tag"}]
    });
    s.execute("bspec.set", &json!({"replace": true, "spec": spec})).unwrap();
    s.execute("model.build", &json!({})).unwrap();
    let r = s.execute("rooms.build", &json!({})).unwrap();
    let n = r["built"].as_u64().unwrap();
    assert!(n >= 1, "{r}");
    let rooms = s.execute("bim.props.find", &json!({"kind": "room"})).unwrap();
    assert_eq!(rooms["count"].as_u64(), Some(n));
    let h = Handle::parse_hex(rooms["handles"][0].as_str().unwrap()).unwrap();
    let g = get(&mut s, h);
    assert_eq!(g["props"]["area"]["source"]["kind"], "computed");
    assert_eq!(g["props"]["area"]["unit"], "sf");
    let area = g["props"]["area"]["value"].as_f64().unwrap();
    assert!(area > 100.0 && area < 250.0, "a 19' x 9' room inside: {area}");
    s.execute("bim.props.set", &json!({"handle": h.hex(), "props": {"floor_finish": "VCT-1", "ceiling_height": 9}})).unwrap();
    s.execute("rooms.build", &json!({})).unwrap();
    let again = s.execute("bim.props.find", &json!({"kind": "room", "where": {"floor_finish": "VCT-1"}})).unwrap();
    assert_eq!(again["count"], 1, "{again}");
    let h2 = Handle::parse_hex(again["handles"][0].as_str().unwrap()).unwrap();
    assert_eq!(get(&mut s, h2)["props"]["ceiling_height"]["source"]["kind"], "user");
}
