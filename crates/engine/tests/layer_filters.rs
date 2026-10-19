//! Layer filters and layer states: property filters (rules, nested), group filters, Invert,
//! filters saved with the drawing (DXF round trip, and other programs' LAYER_FILTER group filters
//! read in); layer states with the properties to restore, "turn off layers not in the state",
//! edited entries and .las files.

use cadaddy_engine::Session;
use serde_json::{Value, json};

fn layers(s: &mut Session, p: Value) -> Vec<String> {
    let r = s.execute("layer.filter.layers", &p).unwrap();
    r["layers"].as_array().unwrap().iter().map(|x| x.as_str().unwrap().to_string()).collect()
}

/// WALLS (red), WALL-EXT (red, off), DOORS (yellow, frozen), A-ANNO (white, locked), E-POWER (blue).
fn drawing() -> Session {
    let mut s = Session::new();
    for (n, c) in [("WALLS", "red"), ("WALL-EXT", "red"), ("DOORS", "yellow"), ("A-ANNO", "white"), ("E-POWER", "blue")] {
        s.execute("layer.new", &json!({"name": n, "color": c})).unwrap();
    }
    s.execute("layer.set", &json!({"name": "WALL-EXT", "on": false})).unwrap();
    s.execute("layer.set", &json!({"name": "DOORS", "frozen": true})).unwrap();
    s.execute("layer.set", &json!({"name": "A-ANNO", "locked": true})).unwrap();
    s
}

#[test]
fn property_filters_match_rules_and_nest() {
    let mut s = drawing();
    s.execute("layer.filter.new", &json!({"name": "Walls", "rules": {"name": "WALL*", "color": "red"}})).unwrap();
    assert_eq!(layers(&mut s, json!({"name": "Walls"})), ["WALLS", "WALL-EXT"]);
    // Nested: the walls that are on.
    s.execute("layer.filter.new", &json!({"name": "Walls on", "parent": "walls", "rules": {"on": true}})).unwrap();
    assert_eq!(layers(&mut s, json!({"name": "Walls on"})), ["WALLS"]);
    // Exclusions and several patterns; states.
    s.execute("layer.filter.new", &json!({"name": "Arch", "rules": {"name": "A-*,DO*,~*ANNO"}})).unwrap();
    assert_eq!(layers(&mut s, json!({"name": "Arch"})), ["DOORS"]);
    s.execute("layer.filter.new", &json!({"name": "Frozen", "rules": {"frozen": true}})).unwrap();
    assert_eq!(layers(&mut s, json!({"name": "Frozen"})), ["DOORS"]);
    // Invert: every layer the filter doesn't show (layer 0 and Defpoints included).
    let inv = layers(&mut s, json!({"name": "Walls", "invert": true}));
    assert!(inv.contains(&"0".to_string()) && inv.contains(&"E-POWER".to_string()) && !inv.contains(&"WALLS".to_string()), "{inv:?}");
    // Rules can be edited; a renamed parent keeps its children.
    s.execute("layer.filter.edit", &json!({"name": "Walls", "newName": "Red walls", "rules": {"color": "1"}})).unwrap();
    assert_eq!(layers(&mut s, json!({"name": "Walls on"})), ["WALLS"]);
    let list = s.execute("layer.filter.list", &json!({})).unwrap();
    assert!(list["filters"].as_array().unwrap().iter().any(|f| f["name"] == "Walls on" && f["parent"] == "Red walls"), "{list}");
    // Deleting a filter deletes the filters nested in it.
    let r = s.execute("layer.filter.delete", &json!({"name": "red walls"})).unwrap();
    assert_eq!(r["deleted"], json!(["Red walls", "Walls on"]));
    // Undo brings them back.
    s.execute("undo", &json!({})).unwrap();
    assert_eq!(s.execute("layer.filter.list", &json!({})).unwrap()["filters"].as_array().unwrap().len(), 4);
}

#[test]
fn group_filters_list_chosen_layers() {
    let mut s = drawing();
    s.execute("layer.filter.new", &json!({"name": "Plot set", "type": "group", "layers": ["walls", "E-POWER"]})).unwrap();
    assert_eq!(layers(&mut s, json!({"name": "Plot set"})), ["WALLS", "E-POWER"]);
    s.execute("layer.filter.edit", &json!({"name": "Plot set", "addLayers": ["DOORS"], "removeLayers": ["WALLS"]})).unwrap();
    assert_eq!(layers(&mut s, json!({"name": "Plot set"})), ["DOORS", "E-POWER"]);
    // A renamed layer: still listed? It is listed by name, as AutoCAD keeps it.
    // Errors.
    for bad in [
        json!({"name": "Plot set", "type": "group"}),
        json!({"name": "All", "rules": {}}),
        json!({"name": "x<y"}),
        json!({"name": "G2", "type": "group", "layers": ["NOPE"]}),
        json!({"name": "G3", "type": "weird"}),
        json!({"name": "G4", "parent": "missing"}),
        json!({"name": "G5", "rules": {"on": "yes"}}),
    ] {
        assert!(s.execute("layer.filter.new", &bad).is_err(), "{bad}");
    }
    assert!(s.execute("layer.filter.layers", &json!({"name": "missing"})).is_err());
    assert_eq!(layers(&mut s, json!({})).len(), 7, "All: every layer");
}

#[test]
fn filters_and_states_survive_a_dxf_round_trip() {
    let mut s = drawing();
    s.execute("layer.filter.new", &json!({"name": "Walls", "rules": {"name": "WALL*"}})).unwrap();
    s.execute("layer.filter.new", &json!({"name": "Mine", "type": "group", "layers": ["DOORS"], "parent": "Walls"})).unwrap();
    s.execute("layerstate.save", &json!({"name": "Plan", "description": "for plotting", "restore": {"color": false}, "offOthers": true})).unwrap();
    let d = s.doc().unwrap();
    let bytes = cadaddy_io::write(d, "f.dxf").unwrap();
    let back = cadaddy_io::read(&bytes, "f.dxf").unwrap();
    assert_eq!(back.layer_filters, d.layer_filters);
    assert_eq!(back.layer_states, d.layer_states);
    assert!(!back.layer_states[0].restore.color && back.layer_states[0].off_others && back.layer_states[0].description == "for plotting");
}

#[test]
fn layer_filter_objects_from_other_programs_become_group_filters() {
    let s = drawing();
    let text = String::from_utf8(cadaddy_io::write(s.doc().unwrap(), "g.dxf").unwrap()).unwrap();
    // A dictionary entry naming a LAYER_FILTER object that lists layers by group 8.
    let extra = "0\nDICTIONARY\n5\nFFF0\n330\n0\n100\nAcDbDictionary\n3\nWalls and doors\n350\nFFF1\n0\nLAYER_FILTER\n5\nFFF1\n330\nFFF0\n100\nAcDbFilter\n100\nAcDbLayerFilter\n8\nWALLS\n8\nDOORS\n8\nGONE\n";
    // In the writer's own layout: right-aligned group codes, CRLF.
    let extra: String = extra.lines().enumerate().map(|(i, l)| if i % 2 == 0 { format!("{l:>3}\r\n") } else { format!("{l}\r\n") }).collect();
    let at = text.rfind("  0\r\nENDSEC").unwrap();
    let dxf = format!("{}{}{}", &text[..at], extra, &text[at..]);
    let d = cadaddy_io::read(dxf.as_bytes(), "g.dxf").unwrap();
    assert_eq!(d.layer_filters.len(), 1, "{:?}", d.layer_filters);
    assert_eq!(d.layer_filters[0].name, "Walls and doors");
    assert_eq!(d.layer_filters[0].kind, cadaddy_engine::doc::LayerFilterKind::Group { layers: vec!["WALLS".into(), "DOORS".into()] });
}

#[test]
fn layer_states_restore_chosen_properties_and_turn_off_the_rest() {
    let mut s = drawing();
    s.execute("layerstate.save", &json!({"name": "Before", "restore": {"color": false}, "offOthers": true})).unwrap();
    // Changes after saving: colours, on/off, and a new layer.
    s.execute("layer.set", &json!({"name": "WALLS", "color": "green", "on": false})).unwrap();
    s.execute("layer.set", &json!({"name": "WALL-EXT", "on": true})).unwrap();
    s.execute("layer.new", &json!({"name": "NEW"})).unwrap();
    s.execute("layerstate.restore", &json!({"name": "Before"})).unwrap();
    let d = s.doc().unwrap();
    let l = |n: &str| d.layer(n).unwrap().clone();
    assert!(l("WALLS").on && !l("WALL-EXT").on, "on/off restored");
    assert_eq!(l("WALLS").color.name(), "Green", "colour not restored");
    assert!(!l("NEW").on, "a layer not in the state is turned off");
    // A restore can choose for itself.
    s.execute("layer.set", &json!({"name": "NEW", "on": true})).unwrap();
    s.execute("layerstate.restore", &json!({"name": "Before", "restore": {"on": true, "color": true}, "offOthers": false})).unwrap();
    let d = s.doc().unwrap();
    assert_eq!(d.layer("WALLS").unwrap().color.name(), "Red");
    assert!(d.layer("NEW").unwrap().on);
    let list = s.execute("layerstate.list", &json!({})).unwrap();
    assert_eq!(list["states"][0]["offOthers"], true);
    assert_eq!(list["states"][0]["restore"]["color"], false);
}

#[test]
fn layer_state_entries_are_edited_and_exported_and_imported() {
    let mut s = drawing();
    s.execute("layerstate.save", &json!({"name": "Plot"})).unwrap();
    // Edit an entry, remove one, add one.
    s.execute("layerstate.edit", &json!({"name": "Plot", "description": "edited", "layers": [{"name": "doors", "frozen": false, "color": "magenta"}], "removeLayers": ["A-ANNO"]})).unwrap();
    s.execute("layer.new", &json!({"name": "LATER"})).unwrap();
    s.execute("layerstate.edit", &json!({"name": "Plot", "addLayers": ["LATER"]})).unwrap();
    let st = s.doc().unwrap().layer_states[0].clone();
    assert_eq!(st.description, "edited");
    assert!(st.layers.iter().any(|l| l.name == "LATER") && !st.layers.iter().any(|l| l.name == "A-ANNO"));
    let doors = st.layers.iter().find(|l| l.name == "DOORS").unwrap();
    assert!(!doors.frozen && doors.color.name() == "Magenta");
    assert!(s.execute("layerstate.edit", &json!({"name": "Plot", "layers": [{"name": "NOPE"}]})).is_err());
    assert!(s.execute("layerstate.edit", &json!({"name": "Plot", "restore": "all"})).is_err());

    // Export, then import into another drawing: its missing layers come along.
    let dir = std::env::temp_dir().join(format!("cadaddy-las-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let las = dir.join("plot.las");
    s.execute("layerstate.export", &json!({"name": "Plot", "path": las.to_str().unwrap()})).unwrap();
    let text = std::fs::read_to_string(&las).unwrap();
    assert!(text.contains("\"format\": \"cadaddy-layerstate\""), "{text}");
    let mut other = Session::new();
    let r = other.execute("layerstate.import", &json!({"path": las.to_str().unwrap()})).unwrap();
    assert_eq!(r["name"], "Plot");
    assert!(other.doc().unwrap().layer("DOORS").is_some(), "missing layers are added");
    assert!(other.execute("layerstate.import", &json!({"path": las.to_str().unwrap()})).is_err(), "exists: needs replace or a name");
    other.execute("layerstate.import", &json!({"path": las.to_str().unwrap(), "name": "Plot 2"})).unwrap();
    assert_eq!(other.doc().unwrap().layer_states.len(), 2);
    // Not ours, damaged, missing.
    let bad = dir.join("bad.las");
    for t in ["{\"format\":\"x\",\"version\":1,\"name\":\"a\",\"layers\":[]}", "nonsense"] {
        std::fs::write(&bad, t).unwrap();
        assert!(other.execute("layerstate.import", &json!({"path": bad.to_str().unwrap()})).is_err(), "{t}");
    }
    assert!(other.execute("layerstate.import", &json!({"path": dir.join("none.las").to_str().unwrap()})).is_err());
    let _ = std::fs::remove_dir_all(&dir);
}
