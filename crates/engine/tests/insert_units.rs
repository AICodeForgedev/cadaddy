//! INSERT of a drawing file and XATTACH scale the reference by the INSUNITS ratio (source over
//! target) when both drawings declare units; a unitless side keeps 1.0 (battle test 5, B7-013).

use std::path::PathBuf;

use cadaddy_engine::Session;
use cadaddy_engine::cmd::file::{IoHooks, set_io};
use cadaddy_engine::doc::geom::Vec3;
use cadaddy_engine::doc::{Drawing, Entity, EntityKind, HVal, Line};
use serde_json::{Value, json};

fn hooks() {
    set_io(IoHooks {
        read: |b, name| cadaddy_io::read(b, name).map_err(|e| e.to_string()),
        write: |d, name| cadaddy_io::write(d, name).map_err(|e| e.to_string()),
        plot: None,
        read_pdf: None,
        read_dwf: None,
        write_dwfx: None,
    });
}

fn folder(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("cadaddy-insunits-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// A file with one line of `length` drawing units, declaring INSUNITS `units`.
fn part(dir: &std::path::Path, name: &str, length: f64, units: i64) -> String {
    let mut d = Drawing::new_metric();
    d.header.set("INSUNITS", HVal::Int(units));
    let h = d.new_handle();
    d.model.push(Entity::new(h, EntityKind::Line(Line { a: Vec3::ZERO, b: Vec3::new(length, 0.0, 0.0) })));
    let path = dir.join(name);
    std::fs::write(&path, cadaddy_io::write(&d, &path.to_string_lossy()).unwrap()).unwrap();
    path.to_string_lossy().into_owned()
}

fn target(units: i64) -> Session {
    let mut s = Session::new();
    s.doc_mut().unwrap().header.set("INSUNITS", HVal::Int(units));
    s
}

/// The length of the one line in model space after exploding the inserted drawing.
fn exploded_length(s: &Session) -> f64 {
    let lens: Vec<f64> = s
        .doc()
        .unwrap()
        .model
        .iter()
        .filter_map(|e| match &e.kind {
            EntityKind::Line(l) => Some((l.b.x - l.a.x).abs()),
            _ => None,
        })
        .collect();
    assert_eq!(lens.len(), 1, "{lens:?}");
    lens[0]
}

fn insert_exploded(s: &mut Session, file: &str) -> Value {
    s.execute("insert", &json!({ "file": file, "at": [0, 0], "explode": true })).unwrap()
}

fn close(a: f64, b: f64) {
    assert!((a - b).abs() < 1e-6, "{a} != {b}");
}

#[test]
fn millimetres_into_inches_scale_by_one_over_25_4() {
    hooks();
    let dir = folder("mm-in");
    let f = part(&dir, "mm.dxf", 254.0, 4);
    let mut s = target(1);
    let r = insert_exploded(&mut s, &f);
    close(exploded_length(&s), 10.0);
    close(r["unitFactor"].as_f64().unwrap(), 1.0 / 25.4);
    assert!(r["note"].as_str().unwrap().contains("scaled by"));
}

#[test]
fn millimetres_into_feet_and_inches_into_millimetres() {
    hooks();
    let dir = folder("mm-ft");
    let f = part(&dir, "mm.dxf", 3048.0, 4);
    let mut s = target(2);
    insert_exploded(&mut s, &f);
    close(exploded_length(&s), 10.0);

    let g = part(&dir, "in.dxf", 10.0, 1);
    let mut s = target(4);
    insert_exploded(&mut s, &g);
    close(exploded_length(&s), 254.0);
}

#[test]
fn unitless_or_equal_units_keep_scale_one() {
    hooks();
    let dir = folder("none");
    for (src, dst) in [(0, 1), (4, 0), (4, 4), (0, 0)] {
        let f = part(&dir, "p.dxf", 254.0, src);
        let mut s = target(dst);
        let r = insert_exploded(&mut s, &f);
        close(exploded_length(&s), 254.0);
        close(r["unitFactor"].as_f64().unwrap(), 1.0);
        assert!(r.get("note").is_none());
    }
}

#[test]
fn a_given_scale_multiplies_the_unit_factor() {
    hooks();
    let dir = folder("scaled");
    let f = part(&dir, "mm.dxf", 254.0, 4);
    let mut s = target(1);
    s.execute("insert", &json!({ "file": f, "at": [0, 0], "scale": 2, "explode": true })).unwrap();
    close(exploded_length(&s), 20.0);
}

#[test]
fn xattach_scales_the_reference_by_the_unit_factor() {
    hooks();
    let dir = folder("xattach");
    let f = part(&dir, "mm.dxf", 254.0, 4);
    let mut s = target(1);
    let r = s.execute("xattach", &json!({ "path": f, "at": [0, 0] })).unwrap();
    close(r["unitFactor"].as_f64().unwrap(), 1.0 / 25.4);
    let scale = s.doc().unwrap().model.iter().find_map(|e| if let EntityKind::Insert(i) = &e.kind { Some(i.scale.x) } else { None }).unwrap();
    close(scale, 1.0 / 25.4);

    let mut u = target(0);
    let r = u.execute("xattach", &json!({ "path": f, "at": [0, 0] })).unwrap();
    close(r["unitFactor"].as_f64().unwrap(), 1.0);
}

#[test]
fn typed_insert_of_a_file_scales_and_says_so() {
    hooks();
    let dir = folder("typed");
    let f = part(&dir, "mm.dxf", 254.0, 4);
    let mut s = target(1);
    s.script(&format!("-INSERT {f} 0,0 1 1 0\n")).unwrap();
    assert!(s.running.is_none(), "INSERT finished: {}", s.prompt_text());
    let scale = s.doc().unwrap().model.iter().find_map(|e| if let EntityKind::Insert(i) = &e.kind { Some(i.scale.x) } else { None }).unwrap();
    close(scale, 1.0 / 25.4);
    assert!(s.log.iter().any(|l| l.contains("scaled by")), "{:?}", s.log);
}
