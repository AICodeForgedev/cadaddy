//! A DWG/DXF save never names a category with nothing listed under it (`notKept` non-empty ⇒
//! `notKeptDetail` non-empty), and an edit that moves a placeholder (a solid fill's pattern angle,
//! a style-placed dimension text middle) loses nothing.

use cadaddy_engine::Session;
use serde_json::{Value, json};

fn hooks() {
    cadaddy_engine::cmd::file::set_io(cadaddy_engine::cmd::file::IoHooks {
        read: |b, n| cadaddy_io::read(b, n).map_err(|e| e.to_string()),
        write: |d, n| cadaddy_io::write(d, n).map_err(|e| e.to_string()),
        plot: None,
        read_pdf: None,
        read_dwf: None,
        write_dwfx: None,
    });
    cadaddy_engine::cmd::file::set_write_as(|d, n, o| {
        cadaddy_io::write_with(d, n, &cadaddy_io::SaveOptions { version: o.version, binary: o.binary }).map_err(|e| e.to_string())
    });
}

fn handle(r: &Value) -> String {
    r["handle"].as_str().or_else(|| r["handles"][0].as_str()).unwrap().to_string()
}

/// `s` saved as a DXF and opened again: its dimensions then carry the block the file gave them
/// (the state the soak's corpus files are in), which is when the writer keeps the stored text middle.
fn reopened(mut s: Session, tag: &str) -> Session {
    let dir = std::env::temp_dir().join(format!("cadaddy-keptnotices2-re-{}-{tag}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("src.dxf").display().to_string();
    s.execute("saveas", &json!({"path": path})).unwrap();
    let mut t = Session::new();
    t.execute("open", &json!({"path": path})).unwrap();
    let _ = std::fs::remove_dir_all(&dir);
    t
}

/// Saves `s` as `name` in a private folder and checks the notice is clean and consistent.
fn assert_clean_save(s: &mut Session, name: &str, what: &str) {
    let dir = std::env::temp_dir().join(format!("cadaddy-keptnotices2-{}-{}", std::process::id(), name.replace('.', "-")));
    std::fs::create_dir_all(&dir).unwrap();
    let r = s.execute("saveas", &json!({"path": dir.join(name).display().to_string()})).unwrap();
    let (nk, det) = (r.get("notKept"), r.get("notKeptDetail"));
    let consistent = nk.is_none_or(|n| n.as_array().is_some_and(Vec::is_empty)) || det.and_then(Value::as_array).is_some_and(|d| !d.is_empty());
    assert!(consistent, "{what}: notKept {nk:?} with notKeptDetail {det:?}");
    assert!(nk.is_none() && det.is_none(), "{what}: {r}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_solid_fill_with_an_unused_angle_and_scale_saves_clean() {
    hooks();
    for f in ["dxf", "dwg"] {
        let mut s = Session::new();
        let r = s.execute("rectang", &json!({"p1": [0, 0], "p2": [10, 8]})).unwrap();
        s.execute("hatch", &json!({"handles": [handle(&r)], "pattern": "SOLID", "scale": 0.526, "angle": 88.433})).unwrap();
        assert_clean_save(&mut s, &format!("solid.{f}"), "solid hatch");
    }
}

#[test]
fn a_moved_or_rotated_linear_dimension_saves_clean() {
    hooks();
    for f in ["dxf", "dwg"] {
        for (cmd, params) in
            [("rotate", json!({"base": [0, 0], "angle": 91.27, "copy": false})), ("move", json!({"from": [0, 0], "to": [3.3, -1.7]}))]
        {
            let mut s = Session::new();
            let d = handle(&s.execute("dimlinear", &json!({"p1": [0, 0], "p2": [10, 0], "at": [5, -2]})).unwrap());
            let mut s = reopened(s, &format!("lin{cmd}{f}"));
            let d = s
                .state()
                .unwrap()
                .drawing()
                .model
                .iter()
                .find(|e| matches!(e.kind, cadaddy_doc::EntityKind::Dimension(_)))
                .map_or(d, |e| e.handle.hex());
            let mut p = params.clone();
            p["handles"] = json!([d]);
            s.execute(cmd, &p).unwrap();
            assert_clean_save(&mut s, &format!("lin.{f}"), &format!("linear dimension {cmd}"));
        }
    }
}

#[test]
fn a_transformed_radius_or_diameter_dimension_saves_clean() {
    hooks();
    let transforms = [
        ("move", json!({"from": [0, 0], "to": [3.3, -1.7]})),
        ("rotate", json!({"base": [1, 2], "angle": 30, "copy": false})),
        ("scale", json!({"base": [1, 2], "factor": 1.8, "copy": false})),
    ];
    for dim in ["dimradius", "dimdiameter"] {
        for (cmd, params) in &transforms {
            for f in ["dxf", "dwg"] {
                let mut s = Session::new();
                let c = s.execute("circle", &json!({"center": [5, 5], "radius": 2})).unwrap();
                let d = handle(&s.execute(dim, &json!({"handle": handle(&c)})).unwrap());
                let mut s = reopened(s, &format!("rad{dim}{cmd}{f}"));
                let d = s
                    .state()
                    .unwrap()
                    .drawing()
                    .model
                    .iter()
                    .find(|e| matches!(e.kind, cadaddy_doc::EntityKind::Dimension(_)))
                    .map_or(d, |e| e.handle.hex());
                let mut p = params.clone();
                p["handles"] = json!([d]);
                s.execute(cmd, &p).unwrap();
                assert_clean_save(&mut s, &format!("rad.{f}"), &format!("{dim} {cmd}"));
            }
        }
    }
}

#[test]
fn a_drawing_with_two_layouts_saves_without_a_layouts_notice() {
    hooks();
    for f in ["dxf", "dwg"] {
        let mut s = Session::new();
        s.execute("line", &json!({"points": [[0, 0], [10, 0]]})).unwrap();
        s.execute("layout.new", &json!({})).unwrap();
        s.execute("layout.set", &json!({"name": "Layout1"})).unwrap();
        s.execute("mview", &json!({"layout": "Layout1", "p1": [0, 0], "p2": [8, 6]})).unwrap();
        s.execute("dimlinear", &json!({"p1": [0, 0], "p2": [5, 0], "at": [2, -2]})).unwrap();
        assert_clean_save(&mut s, &format!("lay.{f}"), "two layouts");
    }
}
