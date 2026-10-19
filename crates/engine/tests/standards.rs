//! CAD standards: STANDARDS associates standards drawings, CHECKSTANDARDS finds layers,
//! linetypes, text and dimension styles that differ from them or that they don't define and fixes
//! them, LAYTRANS translates layers through a mapping table.

use std::path::{Path, PathBuf};

use cadaddy_engine::Session;
use cadaddy_engine::cmd::file::{IoHooks, set_io};
use cadaddy_engine::doc::color::Color;
use cadaddy_engine::doc::geom::Vec3;
use cadaddy_engine::doc::{Drawing, Entity, EntityKind, Layer, Line, TextStyle};
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
    let dir = std::env::temp_dir().join(format!("cadaddy-standards-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn save(d: &Drawing, path: &Path) {
    std::fs::write(path, cadaddy_io::write(d, &path.to_string_lossy()).unwrap()).unwrap();
}

fn layer(d: &mut Drawing, name: &str, color: u8) {
    let mut l = Layer::new(name);
    l.color = Color::Index(color);
    d.layers.push(l);
}

fn line_on(d: &mut Drawing, layer: &str) {
    let h = d.new_handle();
    let mut e = Entity::new(h, EntityKind::Line(Line { a: Vec3::ZERO, b: Vec3::new(10.0, 0.0, 0.0) }));
    e.common.layer = layer.into();
    d.model.push(e);
}

/// The standards drawing: layers WALLS (red) and DOORS (blue), text style NOTES (romans 2.5).
fn standards_file(dir: &Path) -> PathBuf {
    let mut d = Drawing::new_metric();
    layer(&mut d, "WALLS", 1);
    layer(&mut d, "DOORS", 5);
    d.text_styles.push(TextStyle { name: "NOTES".into(), font: "romans.shx".into(), height: 2.5, ..TextStyle::default() });
    let p = dir.join("office.dws");
    save(&d, &p);
    p
}

fn session(d: &Drawing, dir: &Path) -> Session {
    let p = dir.join("job.dxf");
    save(d, &p);
    let mut s = Session::new();
    s.execute("open", &json!({ "path": p.to_string_lossy() })).unwrap();
    s
}

fn ids(r: &Value) -> Vec<String> {
    let mut v: Vec<String> = r["problems"].as_array().unwrap().iter().map(|p| p["id"].as_str().unwrap().to_string()).collect();
    v.sort();
    v
}

#[test]
fn a_drawing_made_from_the_standards_passes() {
    hooks();
    let dir = folder("clean");
    let std = standards_file(&dir);
    let d = cadaddy_io::read(&std::fs::read(&std).unwrap(), "office.dxf").unwrap();
    let mut s = session(&d, &dir);
    let r = s.execute("standards", &json!({ "action": "add", "path": std.to_string_lossy() })).unwrap();
    assert_eq!(r["files"].as_array().unwrap().len(), 1);
    let r = s.execute("checkstandards", &json!({})).unwrap();
    assert_eq!(ids(&r), Vec::<String>::new(), "{r}");
    assert!(r["checked"].as_u64().unwrap() >= 4);
}

#[test]
fn violations_are_found_and_fixed() {
    hooks();
    let dir = folder("fix");
    let std = standards_file(&dir);
    let mut d = Drawing::new_metric();
    layer(&mut d, "WALLS", 3); // wrong colour
    layer(&mut d, "JUNK", 2); // not in the standards
    line_on(&mut d, "JUNK");
    line_on(&mut d, "WALLS");
    d.text_styles.push(TextStyle { name: "NOTES".into(), font: "arial.ttf".into(), height: 2.5, ..TextStyle::default() });
    d.text_styles.push(TextStyle { name: "FANCY".into(), font: "arial.ttf".into(), ..TextStyle::default() });
    d.header.set_str("CLAYER", "JUNK");
    let mut s = session(&d, &dir);
    s.execute("standards", &json!({ "action": "add", "path": std.to_string_lossy() })).unwrap();

    let r = s.execute("checkstandards", &json!({})).unwrap();
    assert_eq!(ids(&r), ["layer:JUNK", "layer:WALLS", "style:FANCY", "style:NOTES"], "{r}");
    let junk = r["problems"].as_array().unwrap().iter().find(|p| p["id"] == "layer:JUNK").unwrap();
    assert_eq!(junk["issue"], "not in the standards");
    assert!(junk["replacements"].as_array().unwrap().iter().any(|n| n == "DOORS"));

    // A name the standards don't have needs a replacement.
    assert!(s.execute("checkstandards", &json!({ "fix": [{ "id": "layer:JUNK" }] })).is_err());

    let report = dir.join("report.html");
    let r = s
        .execute(
            "checkstandards",
            &json!({ "fix": [{ "id": "layer:JUNK", "replaceWith": "DOORS" }, { "id": "style:FANCY", "replaceWith": "NOTES" }], "fixAll": true, "report": report.to_string_lossy() }),
        )
        .unwrap();
    assert_eq!(r["fixed"], 4, "{r}");
    assert_eq!(ids(&r), Vec::<String>::new(), "{r}");
    let html = std::fs::read_to_string(&report).unwrap();
    assert!(html.contains("4 fixed"), "{html}");

    let d = s.doc().unwrap();
    assert!(d.layer("JUNK").is_none());
    assert_eq!(d.layer("DOORS").unwrap().color, Color::Index(5), "copied in from the standards");
    assert_eq!(d.layer("WALLS").unwrap().color, Color::Index(1), "took the standard's colour");
    let layers: Vec<&str> = d.model.iter().map(|e| e.common.layer.as_str()).collect();
    assert_eq!(layers, ["DOORS", "WALLS"]);
    assert_eq!(d.header.str("CLAYER", ""), "DOORS");
    assert_eq!(d.text_style("NOTES").unwrap().font, "romans.shx");
    assert!(d.text_style("FANCY").is_none());

    // One undo puts it all back.
    s.execute("undo", &json!({})).unwrap();
    assert!(s.doc().unwrap().layer("JUNK").is_some());
}

#[test]
fn associations_are_listed_removed_and_kept_with_the_drawing() {
    hooks();
    let dir = folder("assoc");
    let std = standards_file(&dir);
    let mut s = session(&Drawing::new_metric(), &dir);
    let path = std.to_string_lossy().into_owned();
    assert!(s.execute("checkstandards", &json!({})).is_err(), "nothing associated");
    assert!(s.execute("standards", &json!({ "action": "add", "path": dir.join("missing.dws").to_string_lossy() })).is_err());
    s.execute("standards", &json!({ "action": "add", "path": path })).unwrap();
    s.execute("standards", &json!({ "action": "add", "path": path })).unwrap();
    assert_eq!(s.execute("standards", &json!({})).unwrap()["files"], json!([path]));
    let saved = dir.join("saved.dxf");
    s.execute("saveas", &json!({ "path": saved.to_string_lossy() })).unwrap();
    let mut t = Session::new();
    t.execute("open", &json!({ "path": saved.to_string_lossy() })).unwrap();
    assert_eq!(t.execute("standards", &json!({})).unwrap()["files"], json!([path]));
    assert!(t.execute("standards", &json!({ "action": "remove", "path": "other.dws" })).is_err());
    t.execute("standards", &json!({ "action": "remove", "path": path })).unwrap();
    assert_eq!(t.execute("standards", &json!({})).unwrap()["files"], json!([]));
    assert!(t.execute("standards", &json!({ "action": "bogus" })).is_err());
}

#[test]
fn laytrans_maps_layers_and_saves_the_map() {
    hooks();
    let dir = folder("laytrans");
    let std = standards_file(&dir);
    let mut d = Drawing::new_metric();
    layer(&mut d, "A-WALL", 7);
    layer(&mut d, "A-DOOR", 7);
    line_on(&mut d, "A-WALL");
    line_on(&mut d, "A-DOOR");
    let mut s = session(&d, &dir);
    let map = dir.join("office.json");
    let only = dir.join("only.json");
    let r =
        s.execute("laytrans", &json!({ "map": [{ "from": "A-WALL", "to": "WALLS" }], "save": only.to_string_lossy(), "translate": false })).unwrap();
    assert_eq!(r["translated"], 0);
    assert!(only.exists() && s.doc().unwrap().layer("A-WALL").is_some(), "saving alone translates nothing");
    let r = s
        .execute(
            "laytrans",
            &json!({ "map": [{ "from": "A-WALL", "to": "WALLS" }, { "from": "A-DOOR", "to": "DOORS" }], "standards": std.to_string_lossy(), "save": map.to_string_lossy() }),
        )
        .unwrap();
    assert_eq!(r["translated"], 2);
    assert!(r["targets"].as_array().unwrap().iter().any(|n| n == "DOORS"));
    let d = s.doc().unwrap();
    assert!(d.layer("A-WALL").is_none() && d.layer("A-DOOR").is_none());
    assert_eq!(d.layer("WALLS").unwrap().color, Color::Index(1));
    let layers: Vec<&str> = d.model.iter().map(|e| e.common.layer.as_str()).collect();
    assert_eq!(layers, ["WALLS", "DOORS"]);

    let loaded = s.execute("laytrans", &json!({ "load": map.to_string_lossy() })).unwrap();
    assert_eq!(loaded["map"], json!([{ "from": "A-WALL", "to": "WALLS" }, { "from": "A-DOOR", "to": "DOORS" }]));
    assert_eq!(loaded["translated"], 0);

    // A layer the drawing lacks, a target nobody defines, layer 0.
    assert!(s.execute("laytrans", &json!({ "map": [{ "from": "NOPE", "to": "WALLS" }] })).is_err());
    assert!(s.execute("laytrans", &json!({ "map": [{ "from": "WALLS", "to": "NOWHERE" }] })).is_err());
    assert!(s.execute("laytrans", &json!({ "map": [{ "from": "0", "to": "WALLS" }] })).is_err());
    assert_eq!(s.doc().unwrap().layer("WALLS").unwrap().color, Color::Index(1));
}
