//! Materials: the library, material.new/edit/delete, MATERIALATTACH (layers), MATERIALASSIGN and
//! CMATERIAL (objects), MATERIALMAP, and what the drawing file keeps.

use cadaddy_engine::Session;
use cadaddy_engine::doc::Handle;
use cadaddy_engine::doc::material::{self, Projection, Texture};
use serde_json::json;

fn drawn() -> (Session, Handle) {
    let mut s = Session::new();
    s.execute("layer.new", &json!({"name": "Walls"})).unwrap();
    s.execute("line", &json!({"points": [[0, 0], [10, 0]]})).unwrap();
    let h = s.doc().unwrap().model.handles()[0];
    (s, h)
}

#[test]
fn library_and_custom_materials() {
    let (mut s, _) = drawn();
    let list = s.execute("materials", &json!({})).unwrap();
    assert!(list["library"].as_array().unwrap().len() >= 15);
    assert_eq!(list["materials"], json!([]));
    // From the library, sized for the drawing's units (an imperial drawing: inches).
    let r = s.execute("material.new", &json!({"library": "brick-red"})).unwrap();
    assert_eq!(r["name"], json!("Brick, red, running bond"));
    let m = s.doc().unwrap().material("brick, RED, running bond").unwrap().clone();
    assert!((m.mapping.size_u - 0.45 / 0.0254).abs() < 1e-9);
    assert_eq!(m.color_map, Some(Texture::Library("brick-red".into())));
    // A custom one with a texture file.
    s.execute("material.new", &json!({"name": "Floor", "color": "#806040", "roughness": 0.3, "colorMap": "tex/floor.png", "mapping": {"projection": "box", "sizeU": 24, "sizeV": 12}})).unwrap();
    let f = s.doc().unwrap().material("Floor").unwrap().clone();
    assert_eq!((f.color_map, f.mapping.projection, f.mapping.size_u), (Some(Texture::File("tex/floor.png".into())), Projection::Box, 24.0));
    // Names are unique; built-in names are taken; bad values refused.
    assert!(s.execute("material.new", &json!({"name": "floor"})).is_err());
    assert!(s.execute("material.new", &json!({"name": "ByLayer"})).is_err());
    assert!(s.execute("material.new", &json!({"name": "X", "color": "red"})).is_err());
    assert!(s.execute("material.new", &json!({"name": "X", "mapping": {"sizeU": 0}})).is_err());
    assert!(s.execute("material.new", &json!({"library": "nope"})).is_err());
    assert!(s.execute("material.new", &json!({"name": "", "roughness": 1})).is_err());
    // Hostile numbers are clamped, not stored raw.
    s.execute("material.new", &json!({"name": "Odd", "roughness": 7, "opacity": -3})).unwrap();
    let o = s.doc().unwrap().material("Odd").unwrap().clone();
    assert_eq!((o.roughness, o.opacity), (1.0, 0.0));
}

#[test]
fn attach_assign_edit_rename_and_delete() {
    let (mut s, line) = drawn();
    s.execute("material.new", &json!({"library": "concrete"})).unwrap();
    s.execute("material.new", &json!({"library": "oak", "name": "Oak"})).unwrap();
    // A layer's material; an object's own.
    s.execute("materialattach", &json!({"layer": "walls", "material": "Concrete, cast"})).unwrap();
    assert_eq!(s.doc().unwrap().layer("Walls").unwrap().material.as_deref(), Some("Concrete, cast"));
    assert_eq!(s.execute("materialassign", &json!({"handles": [line.hex()], "material": "oak"})).unwrap()["changed"], json!(1));
    let d = s.doc().unwrap();
    assert_eq!(d.material_of(d.entity(line).unwrap(), None).map(|m| m.name.as_str()), Some("Oak"));
    // Undo takes the assignment back.
    s.undo().unwrap();
    assert_eq!(s.doc().unwrap().entity(line).unwrap().common.material, None);
    s.execute("materialassign", &json!({"handles": [line.hex()], "material": "Oak"})).unwrap();
    // Rename follows every reference; delete clears them.
    s.execute("material.edit", &json!({"name": "Concrete, cast", "rename": "Slab", "roughness": 0.4})).unwrap();
    assert_eq!(s.doc().unwrap().layer("Walls").unwrap().material.as_deref(), Some("Slab"));
    assert!((s.doc().unwrap().material("Slab").unwrap().roughness - 0.4).abs() < 1e-12);
    let r = s.execute("material.delete", &json!({"name": "oak"})).unwrap();
    assert_eq!((r["objects"].clone(), r["layers"].clone()), (json!(1), json!(0)));
    assert_eq!(s.doc().unwrap().entity(line).unwrap().common.material, None);
    // Errors: unknown names and layers.
    assert!(s.execute("materialassign", &json!({"handles": [line.hex()], "material": "nope"})).is_err());
    assert!(s.execute("materialattach", &json!({"layer": "nope", "material": "Slab"})).is_err());
    assert!(s.execute("material.edit", &json!({"name": "nope"})).is_err());
    // Detach with Global or null.
    s.execute("materialattach", &json!({"attach": [{"layer": "Walls", "material": null}]})).unwrap();
    assert_eq!(s.doc().unwrap().layer("Walls").unwrap().material, None);
    // At the command line: layer, then material (Enter: Global).
    s.script("materialattach\nWalls\nSlab\n").unwrap();
    assert_eq!(s.doc().unwrap().layer("Walls").unwrap().material.as_deref(), Some("Slab"));
    s.script("materialattach\nWalls\n\n").unwrap();
    assert_eq!(s.doc().unwrap().layer("Walls").unwrap().material, None);
}

#[test]
fn cmaterial_gives_new_objects_their_material() {
    let (mut s, _) = drawn();
    assert_eq!(s.execute("getvar", &json!({"name": "CMATERIAL"})).unwrap()["value"], json!("ByLayer"));
    s.execute("material.new", &json!({"library": "steel-brushed", "name": "Steel"})).unwrap();
    assert!(s.execute("setvar", &json!({"name": "CMATERIAL", "value": "nothing"})).is_err());
    s.execute("setvar", &json!({"name": "CMATERIAL", "value": "steel"})).unwrap();
    s.execute("circle", &json!({"center": [0, 0], "radius": 1})).unwrap();
    let last = s.doc().unwrap().model.iter().last().unwrap().clone();
    assert_eq!(last.common.material.as_deref(), Some("Steel"));
    // A CMATERIAL a file kept as a handle names nothing: new objects stay ByLayer.
    s.doc_mut().unwrap().header.set_str("CMATERIAL", "1F");
    s.execute("circle", &json!({"center": [5, 0], "radius": 1})).unwrap();
    assert_eq!(s.doc().unwrap().model.iter().last().unwrap().common.material, None);
}

#[test]
fn materialmap_sets_and_resets_an_objects_mapping() {
    let (mut s, line) = drawn();
    s.execute("material.new", &json!({"library": "tile-white"})).unwrap();
    s.execute("materialassign", &json!({"handles": [line.hex()], "material": "Ceramic tile, white 300"})).unwrap();
    s.execute("materialmap", &json!({"handles": [line.hex()], "projection": "cylindrical", "rotation": 0.5})).unwrap();
    let m = material::object_mapping(&s.doc().unwrap().entity(line).unwrap().common.xdata).unwrap();
    // The material's tile size is kept; projection and rotation are the object's.
    assert_eq!((m.projection, m.rotation), (Projection::Cylindrical, 0.5));
    assert!((m.size_u - 0.6 / 0.0254).abs() < 1e-9);
    s.execute("materialmap", &json!({"handles": [line.hex()], "reset": true})).unwrap();
    assert_eq!(material::object_mapping(&s.doc().unwrap().entity(line).unwrap().common.xdata), None);
    assert!(s.execute("materialmap", &json!({"handles": [line.hex()], "projection": "cone"})).is_err());
    assert!(s.execute("materialmap", &json!({"handles": [line.hex()]})).is_err());
    // Typed: option, then objects (the selection).
    s.set_selection(vec![line]);
    s.script("materialmap\nplanar\n").unwrap();
    assert_eq!(material::object_mapping(&s.doc().unwrap().entity(line).unwrap().common.xdata).map(|m| m.projection), Some(Projection::Planar));
    s.set_selection(vec![line]);
    s.script("materialassign\nByBlock\n").unwrap();
    assert_eq!(s.doc().unwrap().entity(line).unwrap().common.material.as_deref(), Some("ByBlock"));
}

#[test]
fn materials_are_saved_with_the_drawing() {
    let (mut s, line) = drawn();
    s.execute("material.new", &json!({"library": "carpet-blue", "name": "Carpet"})).unwrap();
    s.execute("materialattach", &json!({"layer": "Walls", "material": "Carpet"})).unwrap();
    s.execute("materialassign", &json!({"handles": [line.hex()], "material": "Carpet"})).unwrap();
    s.execute("materialmap", &json!({"handles": [line.hex()], "projection": "box", "sizeU": 30, "sizeV": 30})).unwrap();
    let d = s.doc().unwrap().clone();
    let back = cadaddy_io::read(&cadaddy_io::write(&d, "m.dxf").unwrap(), "m.dxf").unwrap();
    assert_eq!(back.materials, d.materials);
    assert_eq!(back.layer("Walls").unwrap().material.as_deref(), Some("Carpet"));
    let e = back.entity(line).unwrap();
    assert_eq!(e.common.material.as_deref(), Some("Carpet"));
    assert_eq!(material::object_mapping(&e.common.xdata).map(|m| (m.projection, m.size_u)), Some((Projection::Box, 30.0)));
}
