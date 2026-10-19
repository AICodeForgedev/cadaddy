//! SAVEAS as Release 12 DXF: newer objects come down to R12 ones (text lines, polylines,
//! exploded dimensions, hatch lines), what R12 has no form for is reported, and the typed SAVEAS
//! asks the format, the version and the file name as AutoCAD does with FILEDIA 0.

use cadaddy_doc::{EntityKind, HVal};
use cadaddy_engine::cmd::file::{IoHooks, SaveAs, set_io, set_write_as};
use cadaddy_engine::{Input, Session};
use serde_json::json;

fn hooks() {
    set_io(IoHooks {
        read: |b, name| cadaddy_io::read(b, name).map_err(|e| e.to_string()),
        write: |d, name| cadaddy_io::write(d, name).map_err(|e| e.to_string()),
        plot: None,
        read_pdf: None,
    });
    set_write_as(|d, name, o: &SaveAs| {
        cadaddy_io::write_with(d, name, &cadaddy_io::SaveOptions { version: o.version, binary: o.binary }).map_err(|e| e.to_string())
    });
}

fn folder(tag: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("cadaddy-r12-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn kinds(d: &cadaddy_doc::Drawing) -> Vec<&'static str> {
    let mut v: Vec<&'static str> = d.model.iter().map(|e| e.kind.type_name()).collect();
    v.sort();
    v.dedup();
    v
}

#[test]
fn newer_objects_come_down_to_r12_ones() {
    hooks();
    let dir = folder("down");
    let mut s = Session::new();
    s.execute("mtext", &json!({ "at": [0, 20], "text": "FIRST\\PSECOND" })).unwrap();
    s.execute("ellipse", &json!({ "center": [0, 0], "major": [5, 0], "ratio": 0.4 })).unwrap();
    s.execute("spline", &json!({ "fit": [[0, 0], [4, 1], [5, 4], [1, 5]] })).unwrap();
    s.execute("dimlinear", &json!({ "p1": [0, 0], "p2": [10, 0], "at": [5, 5] })).unwrap();
    s.execute("rectang", &json!({ "p1": [20, 0], "p2": [30, 10] })).unwrap();
    s.execute("hatch", &json!({ "points": [[25, 5]], "pattern": "ANSI31" })).unwrap();
    s.execute("xline", &json!({ "base": [0, 0], "angle": 45 })).unwrap();
    let path = dir.join("old.dxf");
    let r = s.execute("saveas", &json!({ "path": path.to_string_lossy(), "version": "12" })).unwrap();
    assert_eq!(r["version"], "12");
    assert_eq!(r["notSaved"]["Xline"], 1, "{r}");

    let bytes = std::fs::read(&path).unwrap();
    let back = cadaddy_io::read(&bytes, "old.dxf").unwrap();
    assert_eq!(back.header.get("ACADVER").and_then(HVal::as_str), Some("AC1009"));
    let k = kinds(&back);
    for gone in ["MText", "Ellipse", "Spline", "Dimension", "Hatch", "Xline"] {
        assert!(!k.contains(&gone), "{gone} in an R12 file: {k:?}");
    }
    let texts: Vec<String> = back.model.iter().filter_map(|e| if let EntityKind::Text(t) = &e.kind { Some(t.value.clone()) } else { None }).collect();
    assert!(texts.contains(&"FIRST".to_string()) && texts.contains(&"SECOND".to_string()), "{texts:?}");
    assert!(texts.iter().any(|t| t.starts_with("10")), "the dimension's text: {texts:?}");
    assert!(k.contains(&"Line") && k.contains(&"2D Solid"), "the dimension's lines and arrows: {k:?}");
    let polys = back.model.iter().filter(|e| matches!(e.kind, EntityKind::LwPolyline(_))).count();
    // Ellipse, spline, the rectangle and the hatch's lines.
    assert!(polys >= 4, "{polys} polylines");
    // The session's drawing itself is untouched.
    let mine = kinds(s.doc().unwrap());
    assert!(mine.contains(&"MText") && mine.contains(&"Xline"));
    // Release 12 is DXF only.
    assert!(s.execute("saveas", &json!({ "path": dir.join("old.dwg").to_string_lossy(), "version": "R12" })).is_err());
}

#[test]
fn typed_saveas_asks_format_version_and_name() {
    hooks();
    let dir = folder("typed");
    let mut s = Session::new();
    s.execute("line", &json!({ "points": [[0, 0], [10, 10]] })).unwrap();
    s.start("saveas").unwrap();
    s.input(Input::Keyword("dXf".into())).unwrap();
    s.input(Input::Text("R12".into())).unwrap();
    let base = dir.join("typed");
    s.input(Input::Text(base.to_string_lossy().into_owned())).unwrap();
    let text = std::fs::read_to_string(dir.join("typed.dxf")).unwrap();
    assert!(text.contains("AC1009"));

    // Enter keeps each: the drawing's format (DXF now) and version, and its own file.
    s.execute("line", &json!({ "points": [[0, 0], [5, 0]] })).unwrap();
    s.start("saveas").unwrap();
    s.input(Input::Enter).unwrap();
    s.input(Input::Text("2004".into())).unwrap();
    s.input(Input::Enter).unwrap();
    let text = std::fs::read_to_string(dir.join("typed.dxf")).unwrap();
    assert!(text.contains("AC1018"), "saved over at 2004");

    // CDY asks no version; a bad format or version is refused at its prompt.
    s.start("saveas").unwrap();
    let _ = s.input(Input::Text("pdf".into()));
    assert!(s.current_prompt().unwrap().message.contains("File format"), "asked again");
    s.input(Input::Text("cdy".into())).unwrap();
    s.input(Input::Text(dir.join("own").to_string_lossy().into_owned())).unwrap();
    assert!(dir.join("own.cdy").exists());
    s.start("saveas").unwrap();
    s.input(Input::Text("dwg".into())).unwrap();
    let _ = s.input(Input::Text("R12".into()));
    assert_eq!(s.current_prompt().unwrap().message, "Version", "R12 is DXF only");
    s.input(Input::Cancel).unwrap();
}
