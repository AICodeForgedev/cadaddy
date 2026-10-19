//! Editing in place beyond plain blocks: REFEDIT on an xref writes its source drawing back (with
//! a .bak) and reloads it, nested references are edited by their path, saving a block with
//! attributes synchronises its references, and XCLIP Clipdepth sets clipping planes.

use std::path::{Path, PathBuf};

use cadaddy_engine::Session;
use cadaddy_engine::cmd::file::{IoHooks, set_io};
use cadaddy_engine::doc::geom::Vec3;
use cadaddy_engine::doc::{Common, Drawing, Entity, EntityKind, Handle, Insert, Layer, Line, Space, XrefStatus};
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
    let dir = std::env::temp_dir().join(format!("cadaddy-xrefedit-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn save(d: &Drawing, path: &Path) {
    std::fs::write(path, cadaddy_io::write(d, &path.to_string_lossy()).unwrap()).unwrap();
}

fn read(path: &Path) -> Drawing {
    cadaddy_io::read(&std::fs::read(path).unwrap(), &path.to_string_lossy()).unwrap()
}

fn line(d: &mut Drawing, layer: &str, a: (f64, f64), b: (f64, f64)) {
    let h = d.new_handle();
    let mut e = Entity::new(h, EntityKind::Line(Line { a: Vec3::new(a.0, a.1, 0.0), b: Vec3::new(b.0, b.1, 0.0) }));
    e.common.layer = layer.into();
    d.model.push(e);
}

fn lines(d: &Drawing) -> Vec<(f64, f64, f64, f64, String)> {
    let mut v: Vec<_> = d
        .model
        .iter()
        .filter_map(|e| match &e.kind {
            EntityKind::Line(l) => Some((l.a.x, l.a.y, l.b.x, l.b.y, e.common.layer.clone())),
            _ => None,
        })
        .collect();
    v.sort_by(|a, b| a.partial_cmp(b).unwrap());
    v
}

/// A site plan with two lines on layer Walls, and a host drawing that attaches it at 100,0.
fn attached(tag: &str) -> (Session, PathBuf, String) {
    hooks();
    let dir = folder(tag);
    let mut site = Drawing::new_metric();
    site.layers.push(Layer { name: "Walls".into(), ..Layer::default() });
    line(&mut site, "Walls", (0.0, 0.0), (10.0, 0.0));
    line(&mut site, "Walls", (0.0, 5.0), (10.0, 5.0));
    let file = dir.join("site.dxf");
    save(&site, &file);
    let host = dir.join("host.dxf");
    save(&Drawing::new_metric(), &host);
    let mut s = Session::new();
    s.execute("open", &json!({ "path": host.to_string_lossy() })).unwrap();
    s.execute("xattach", &json!({ "path": "site.dxf", "at": [100, 0] })).unwrap();
    let x = s.doc().unwrap().model.handles().last().unwrap().hex();
    (s, file, x)
}

fn working(v: &Value) -> Vec<String> {
    v["working"].as_array().unwrap().iter().map(|x| x.as_str().unwrap().to_string()).collect()
}

#[test]
fn an_xref_edited_in_place_writes_its_source_drawing_and_reloads() {
    let (mut s, file, x) = attached("write");
    let r = s.execute("refedit", &json!({ "handle": x })).unwrap();
    assert_eq!((r["block"].clone(), r["xref"].clone()), (json!("site"), json!(true)));
    let w = working(&r);
    // Move the first line up 1 (drawing coordinates: the xref sits at 100,0), erase the second,
    // and draw a new one on a layer only the host has.
    let first = w
        .iter()
        .find(|h| matches!(&s.doc().unwrap().entity(Handle::parse_hex(h).unwrap()).unwrap().kind, EntityKind::Line(l) if l.a.y == 0.0))
        .unwrap()
        .clone();
    let second = w.iter().find(|h| **h != first).unwrap().clone();
    s.execute("move", &json!({ "handles": [first], "from": [0, 0], "to": [0, 1] })).unwrap();
    s.execute("erase", &json!({ "handles": [second] })).unwrap();
    s.execute("layer", &json!({ "action": "new", "name": "Fence" })).ok();
    if s.doc().unwrap().layer("Fence").is_none() {
        s.doc_mut().unwrap().layers.push(Layer { name: "Fence".into(), ..Layer::default() });
    }
    let n = s.execute("line", &json!({ "points": [[100, 20], [110, 20]] })).unwrap()["handles"][0].as_str().unwrap().to_string();
    s.execute("chprop", &json!({ "handles": [n], "layer": "Fence" })).unwrap();
    let r = s.execute("refclose", &json!({ "option": "save" })).unwrap();
    assert_eq!(r["reloaded"], json!(true), "{r}");

    // The source drawing has the edit, in its own coordinates and layer names.
    let back = read(&file);
    assert_eq!(lines(&back), vec![(0.0, 1.0, 10.0, 1.0, "Walls".to_string()), (0.0, 20.0, 10.0, 20.0, "Fence".to_string())]);
    assert!(back.layer("Fence").is_some(), "the host layer the new line uses comes along");
    // The previous version is kept.
    let bak = read(&file.with_extension("bak"));
    assert_eq!(lines(&bak).len(), 2);
    // The host shows the reloaded xref.
    let d = s.doc().unwrap();
    assert!(d.refedit.is_none());
    let b = d.block("site").unwrap();
    assert_eq!((b.xref_status, b.entities.len()), (XrefStatus::Loaded, 2));
    assert!(b.entities.iter().any(|e| e.common.layer == "site|Fence"));
    assert_eq!(d.model.len(), 1, "only the reference is left in the drawing");
}

#[test]
fn an_unchanged_object_is_written_back_as_the_file_had_it() {
    let (mut s, file, x) = attached("same");
    let before = read(&file);
    let keep: Vec<Handle> = before.model.handles();
    let r = s.execute("refedit", &json!({ "handle": x })).unwrap();
    let w = working(&r);
    s.execute("move", &json!({ "handles": [w[0]], "from": [0, 0], "to": [0, 3] })).unwrap();
    s.execute("refclose", &json!({})).unwrap();
    let after = read(&file);
    // Both objects keep their handles.
    let mut a = after.model.handles();
    a.sort();
    let mut k = keep.clone();
    k.sort();
    assert_eq!(a, k);
}

#[test]
fn a_source_drawing_changed_since_loading_is_not_overwritten() {
    let (mut s, file, x) = attached("changed");
    let r = s.execute("refedit", &json!({ "handle": x })).unwrap();
    s.execute("erase", &json!({ "handles": [working(&r)[0]] })).unwrap();
    // Someone else changes the file meanwhile.
    let mut other = read(&file);
    line(&mut other, "Walls", (50.0, 50.0), (60.0, 60.0));
    save(&other, &file);
    let e = s.execute("refclose", &json!({ "option": "save" })).unwrap_err();
    assert!(e.to_string().contains("changed since"), "{e}");
    assert_eq!(lines(&read(&file)).len(), 3, "their change is kept");
    assert!(s.doc().unwrap().refedit.is_some(), "the edit stays open");
    // Discarding still works.
    s.execute("refclose", &json!({ "option": "discard" })).unwrap();
    assert!(s.doc().unwrap().refedit.is_none());
}

#[test]
fn an_xref_without_its_file_is_refused_up_front() {
    let (mut s, file, x) = attached("gone");
    std::fs::remove_file(&file).unwrap();
    let e = s.execute("refedit", &json!({ "handle": x })).unwrap_err();
    assert!(e.to_string().contains("not found"), "{e}");
    assert!(s.doc().unwrap().refedit.is_none());
}

fn insert(s: &mut Session, block: &str, at: (f64, f64)) -> String {
    let kind = EntityKind::Insert(Insert {
        block: block.into(),
        insert: Vec3::new(at.0, at.1, 0.0),
        scale: Vec3::new(1.0, 1.0, 1.0),
        rotation: 0.0,
        attribs: Vec::new(),
        cols: 1,
        rows: 1,
        col_spacing: 0.0,
        row_spacing: 0.0,
        clip: None,
    });
    s.doc_mut().unwrap().add(&Space::Model, Common::default(), kind).unwrap().hex()
}

#[test]
fn a_nested_reference_is_edited_by_its_path() {
    let mut s = Session::new();
    let l = s.execute("line", &json!({ "points": [[0, 0], [4, 0]] })).unwrap()["handles"][0].clone();
    s.execute("block", &json!({ "name": "Inner", "base": [0, 0], "handles": [l], "keep": "delete" })).unwrap();
    let r = s.execute("circle", &json!({ "center": [0, 0], "radius": 1 })).unwrap();
    let c = r.get("handle").cloned().unwrap_or_else(|| r["handles"][0].clone());
    let i = insert(&mut s, "Inner", (10.0, 0.0));
    s.execute("block", &json!({ "name": "Outer", "base": [0, 0], "handles": [c, i], "keep": "delete" })).unwrap();
    let o = insert(&mut s, "Outer", (100.0, 0.0));
    let tree = s.execute("refedit.tree", &json!({ "handle": o })).unwrap();
    let nested = tree["nested"][0]["handle"].clone();
    assert_eq!(tree["nested"][0]["block"], "Inner");
    let r = s.execute("refedit", &json!({ "handle": o, "nested": [nested] })).unwrap();
    assert_eq!((r["block"].clone(), r["faded"].clone()), (json!("Inner"), json!(1)), "the circle around it is faded");
    let w = working(&r);
    // The nested line shows where it is drawn: 100 + 10.
    match &s.doc().unwrap().entity(Handle::parse_hex(&w[0]).unwrap()).unwrap().kind {
        EntityKind::Line(l) => assert_eq!((l.a.x, l.b.x), (110.0, 114.0)),
        k => panic!("{k:?}"),
    }
    // The faded circle can't be edited or added.
    let faded = s.doc().unwrap().refedit.as_ref().unwrap().context[0].hex();
    assert!(s.execute("refset", &json!({ "add": [faded] })).is_err());
    s.execute("move", &json!({ "handles": [w[0]], "from": [0, 0], "to": [0, 2] })).unwrap();
    s.execute("refclose", &json!({})).unwrap();
    let d = s.doc().unwrap();
    let inner: Vec<f64> =
        d.block("Inner").unwrap().entities.iter().filter_map(|e| if let EntityKind::Line(l) = &e.kind { Some(l.a.y) } else { None }).collect();
    assert_eq!(inner, vec![2.0]);
    assert_eq!(d.block("Outer").unwrap().entities.len(), 2, "the outer block is as it was");
    assert_eq!(d.model.len(), 1, "the faded copies are gone");
    // A path that doesn't lead through references is refused.
    let o2 = insert(&mut s, "Outer", (0.0, 0.0));
    assert!(s.execute("refedit", &json!({ "handle": o2, "nested": ["FFFF"] })).is_err());
    assert!(s.execute("refedit", &json!({ "handle": o2, "nested": "x" })).is_err());
}

#[test]
fn saving_a_block_with_attributes_synchronises_its_references() {
    let mut s = Session::new();
    let l = s.execute("line", &json!({ "points": [[0, 0], [4, 0]] })).unwrap()["handles"][0].clone();
    s.execute("block", &json!({ "name": "Tag", "base": [0, 0], "handles": [l], "keep": "delete" })).unwrap();
    let a = insert(&mut s, "Tag", (0.0, 0.0));
    let b = insert(&mut s, "Tag", (50.0, 0.0));
    s.execute("refedit", &json!({ "handle": a })).unwrap();
    s.execute("attdef", &json!({ "tag": "ROOM", "default": "101", "at": [0, 2], "height": 1 })).unwrap();
    let r = s.execute("refclose", &json!({})).unwrap();
    assert_eq!(r["synced"], json!(2), "{r}");
    let d = s.doc().unwrap();
    match &d.entity(Handle::parse_hex(&b).unwrap()).unwrap().kind {
        EntityKind::Insert(i) => {
            assert_eq!(i.attribs.iter().map(|a| (a.tag.as_str(), a.text.value.as_str())).collect::<Vec<_>>(), vec![("ROOM", "101")])
        }
        k => panic!("{k:?}"),
    }
}

#[test]
fn xclip_clipdepth_sets_and_removes_the_planes() {
    let mut s = Session::new();
    let l = s.execute("line", &json!({ "points": [[0, 0], [4, 0]] })).unwrap()["handles"][0].clone();
    s.execute("block", &json!({ "name": "P", "base": [0, 0], "handles": [l], "keep": "delete" })).unwrap();
    let i = insert(&mut s, "P", (0.0, 0.0));
    // Only clipped references take clipping planes.
    assert_eq!(s.execute("xclip", &json!({ "handles": [i], "option": "clipdepth", "front": 5 })).unwrap()["changed"], 0);
    s.execute("xclip", &json!({ "handles": [i], "boundary": [[-1, -1], [5, 1]] })).unwrap();
    s.execute("xclip", &json!({ "handles": [i], "option": "clipdepth", "front": 5, "back": 2 })).unwrap();
    let clip = |s: &Session| match &s.doc().unwrap().entity(Handle::parse_hex(&i).unwrap()).unwrap().kind {
        EntityKind::Insert(ins) => ins.clip.clone().unwrap(),
        k => panic!("{k:?}"),
    };
    assert_eq!((clip(&s).front, clip(&s).back), (Some(5.0), Some(-2.0)));
    // An object above the front plane is beyond it; one at the boundary's height isn't.
    let high = EntityKind::Line(Line { a: Vec3::new(0.0, 0.0, 6.0), b: Vec3::new(1.0, 0.0, 6.0) });
    let flat = EntityKind::Line(Line { a: Vec3::ZERO, b: Vec3::new(1.0, 0.0, 0.0) });
    let low = EntityKind::Line(Line { a: Vec3::new(0.0, 0.0, -3.0), b: Vec3::new(1.0, 0.0, -3.0) });
    assert_eq!((clip(&s).beyond_depth(&high), clip(&s).beyond_depth(&flat), clip(&s).beyond_depth(&low)), (true, false, true));
    // Saved to DXF and read back, the planes stay.
    let back = cadaddy_io::read(cadaddy_io::write_dxf(s.doc().unwrap()).as_bytes(), "p.dxf").unwrap();
    match &back.entity(Handle::parse_hex(&i).unwrap()).unwrap().kind {
        EntityKind::Insert(ins) => {
            let c = ins.clip.clone().unwrap();
            assert_eq!((c.front, c.back, c.boundary.len()), (Some(5.0), Some(-2.0), 4));
        }
        k => panic!("{k:?}"),
    }
    // Typed: Clipdepth, front distance, Remove.
    s.set_selection(vec![Handle::parse_hex(&i).unwrap()]);
    s.execute("xclip", &json!({ "handles": [i], "option": "clipdepth", "remove": true })).unwrap();
    assert_eq!((clip(&s).front, clip(&s).back), (None, None));
    for p in
        [json!({ "handles": [i], "option": "clipdepth", "front": "x" }), json!({ "handles": [i], "option": "clipdepth", "front": -5, "back": 1 })]
    {
        assert!(s.execute("xclip", &p).is_err(), "{p}");
    }
}
