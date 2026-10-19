//! ADCENTER (DesignCenter): folders and another drawing listed without opening it, and its
//! layers, styles, blocks and layouts brought into the current drawing without overwriting what
//! the drawing has.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use cadaddy_engine::Session;
use cadaddy_engine::cmd::file::{IoHooks, set_io};
use cadaddy_engine::doc::color::Color;
use cadaddy_engine::doc::geom::Vec3;
use cadaddy_engine::doc::{Block, Circle, Drawing, Entity, EntityKind, Handle, Insert, Layer, Line, Linetype, TextStyle};
use serde_json::{Value, json};

fn hooks() {
    set_io(IoHooks {
        read: |b, name| cadaddy_io::read(b, name).map_err(|e| e.to_string()),
        write: |d, name| cadaddy_io::write(d, name).map_err(|e| e.to_string()),
        plot: None,
        read_pdf: None,
    });
}

fn folder(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("cadaddy-adcenter-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("sub")).unwrap();
    dir
}

fn insert(block: &str) -> Insert {
    Insert {
        block: block.into(),
        insert: Vec3::ZERO,
        scale: Vec3::new(1.0, 1.0, 1.0),
        rotation: 0.0,
        attribs: Vec::new(),
        cols: 1,
        rows: 1,
        col_spacing: 0.0,
        row_spacing: 0.0,
        clip: None,
    }
}

/// The library drawing: layer ELEC (red, DASHED), text style NOTES, dimension style ARCH (using
/// NOTES), block OUTLET (a circle on DEVICES and a nested DOT), a layout "Sheet A".
fn library(dir: &Path) -> PathBuf {
    let mut d = Drawing::new_imperial();
    d.linetypes.push(Linetype {
        name: "DASHED".into(),
        description: "Dashed".into(),
        pattern: serde_json::from_value(json!([{"length": 0.5}, {"length": -0.25}])).unwrap(),
    });
    let mut elec = Layer::new("ELEC");
    elec.color = Color::Index(1);
    elec.linetype = "DASHED".into();
    d.layers.push(elec);
    d.layers.push(Layer::new("DEVICES"));
    d.text_styles.push(TextStyle { name: "NOTES".into(), font: "romans.shx".into(), height: 0.125, ..TextStyle::default() });
    let mut arch = d.dim_style("Standard").cloned().unwrap_or_default();
    arch.name = "ARCH".into();
    arch.text_style = "NOTES".into();
    d.dim_styles.push(arch);
    let mut dot = Block::new("DOT");
    dot.entities.push(Entity::new(Handle(0x60), EntityKind::Circle(Circle { center: Vec3::ZERO, radius: 0.1 })));
    d.blocks.insert("DOT".into(), Arc::new(dot));
    let mut outlet = Block::new("OUTLET");
    let mut c = Entity::new(Handle(0x61), EntityKind::Circle(Circle { center: Vec3::ZERO, radius: 1.0 }));
    c.common.layer = "DEVICES".into();
    outlet.entities.push(c);
    outlet.entities.push(Entity::new(Handle(0x62), EntityKind::Insert(insert("DOT"))));
    d.blocks.insert("OUTLET".into(), Arc::new(outlet));
    let h = d.new_handle();
    d.model.push(Entity::new(h, EntityKind::Line(Line { a: Vec3::ZERO, b: Vec3::new(10.0, 0.0, 0.0) })));
    if let Some(l) = d.layouts.first_mut() {
        l.name = "Sheet A".into();
    }
    let p = dir.join("library.dxf");
    std::fs::write(&p, cadaddy_io::write(&d, "library.dxf").unwrap()).unwrap();
    p
}

fn names(v: &Value) -> Vec<String> {
    v.as_array().unwrap().iter().map(|n| n.as_str().unwrap().to_string()).collect()
}

#[test]
fn a_folder_and_a_drawing_are_listed_without_opening_them() {
    hooks();
    let dir = folder("list");
    let lib = library(&dir);
    std::fs::write(dir.join("notes.txt"), "x").unwrap();
    let mut s = Session::new();
    let f = s.execute("adcenter", &json!({ "action": "folder", "path": dir.to_string_lossy() })).unwrap();
    assert_eq!(names(&f["folders"]), ["sub"]);
    assert_eq!(names(&f["drawings"]), ["library.dxf"]);

    let docs = s.docs.len();
    let l = s.execute("adcenter", &json!({ "path": lib.to_string_lossy() })).unwrap();
    assert_eq!(s.docs.len(), docs, "nothing opened");
    assert!(names(&l["layers"]).contains(&"ELEC".to_string()));
    assert!(names(&l["textStyles"]).contains(&"NOTES".to_string()));
    assert!(names(&l["dimStyles"]).contains(&"ARCH".to_string()));
    assert!(names(&l["layouts"]).contains(&"Sheet A".to_string()));
    let outlet = l["blocks"].as_array().unwrap().iter().find(|b| b["name"] == "OUTLET").unwrap();
    assert_eq!(outlet["objects"], 2);
    let preview = outlet["preview"].as_array().unwrap();
    assert!(!preview.is_empty(), "a preview");
    assert!(preview.iter().flat_map(|l| l.as_array().unwrap()).all(|p| (-0.01..=1.01).contains(&p[0].as_f64().unwrap())));
    assert!(s.execute("adcenter", &json!({ "path": dir.join("missing.dxf").to_string_lossy() })).is_err());
}

#[test]
fn layers_styles_blocks_and_layouts_come_in_without_overwriting() {
    hooks();
    let dir = folder("import");
    let lib = library(&dir);
    let path = lib.to_string_lossy().into_owned();
    let mut s = Session::new();
    // The drawing has its own DEVICES layer (blue): it stays.
    let mut devices = Layer::new("DEVICES");
    devices.color = Color::Index(5);
    s.doc_mut().unwrap().layers.push(devices);

    let r = s.execute("adcenter", &json!({ "action": "import", "path": path, "kind": "layers", "names": ["ELEC"] })).unwrap();
    assert_eq!(r["added"], json!(["ELEC"]));
    let d = s.doc().unwrap();
    assert_eq!(d.layer("ELEC").unwrap().color, Color::Index(1));
    assert!(d.linetype("DASHED").is_some(), "with its linetype");

    s.execute("adcenter", &json!({ "action": "import", "path": path, "kind": "dimStyles", "names": ["ARCH"] })).unwrap();
    let d = s.doc().unwrap();
    assert!(d.dim_style("ARCH").is_some() && d.text_style("NOTES").is_some(), "the style with its text style");

    let r = s.execute("adcenter", &json!({ "action": "import", "path": path, "kind": "blocks", "names": ["OUTLET"], "at": [5, 5] })).unwrap();
    let d = s.doc().unwrap();
    assert!(d.block("OUTLET").is_some() && d.block("DOT").is_some(), "with its nested block");
    assert_eq!(d.layer("DEVICES").unwrap().color, Color::Index(5), "the drawing's layer kept");
    let h = r["handle"].as_str().unwrap();
    let placed = d.model.iter().find(|e| e.handle.hex() == h).unwrap();
    let EntityKind::Insert(i) = &placed.kind else { panic!("not a block reference") };
    assert_eq!((i.block.as_str(), i.insert.x, i.insert.y), ("OUTLET", 5.0, 5.0));

    // Again: kept, not overwritten.
    let r = s.execute("adcenter", &json!({ "action": "import", "path": path, "kind": "layers", "names": ["ELEC"] })).unwrap();
    assert_eq!((r["added"].clone(), r["kept"].clone()), (json!([]), json!(["ELEC"])));

    let r = s.execute("adcenter", &json!({ "action": "import", "path": path, "kind": "layouts", "names": ["Sheet A"] })).unwrap();
    assert_eq!(r["added"], json!(["Sheet A"]));
    assert!(s.doc().unwrap().layouts.iter().any(|l| l.name == "Sheet A"));

    // Undo takes the last import back.
    s.execute("undo", &json!({})).unwrap();
    assert!(!s.doc().unwrap().layouts.iter().any(|l| l.name == "Sheet A"));

    for bad in [json!({ "kind": "layers", "names": ["NOPE"] }), json!({ "kind": "widgets", "names": ["X"] }), json!({ "kind": "layers" })] {
        let mut p = bad.clone();
        p["action"] = json!("import");
        p["path"] = json!(path);
        assert!(s.execute("adcenter", &p).is_err(), "{bad}");
    }
}
