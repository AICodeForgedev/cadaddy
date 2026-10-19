//! Data extraction in depth: object properties beyond attributes, identical rows counted,
//! settings saved to a .dxe file and used again, other drawing files, spreadsheet-ready CSV, and
//! tables that DATAEXTRACTION Update fills again (through a save and load). BATTMAN adds
//! attribute definitions and switches them to and from multiline.

use cadaddy_engine::Session;
use cadaddy_engine::cmd::file::{IoHooks, set_io};
use cadaddy_engine::doc::EntityKind;
use serde_json::{Value, json};

fn io() {
    set_io(IoHooks {
        read: |b, n| cadaddy_io::read(b, n).map_err(|e| e.to_string()),
        write: |d, n| cadaddy_io::write(d, n).map_err(|e| e.to_string()),
        plot: None,
        read_pdf: None,
    });
}

fn tmp(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("cadaddy-xd-{}-{name}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// Two equal 10-unit lines, a 20-unit line, a circle and a closed square.
fn shapes() -> Session {
    let mut s = Session::new();
    for (a, b) in [([0, 0], [10, 0]), ([0, 5], [10, 5]), ([0, 9], [20, 9])] {
        s.execute("line", &json!({"points": [a, b]})).unwrap();
    }
    s.execute("circle", &json!({"center": [50, 50], "radius": 2})).unwrap();
    s.execute("pline", &json!({"vertices": [[0, 0], [4, 0], [4, 4], [0, 4]], "closed": true})).unwrap();
    s
}

fn rows(v: &Value) -> Vec<Vec<String>> {
    v["rows"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| {
            r.as_array()
                .unwrap()
                .iter()
                .map(|c| c.as_str().map(str::to_string).or_else(|| c.as_f64().map(|v| format!("{v}"))).unwrap_or_default())
                .collect()
        })
        .collect()
}

#[test]
fn object_properties_are_columns_and_identical_rows_are_counted() {
    let mut s = shapes();
    let r = s
        .execute(
            "dataextraction",
            &json!({"includeBlocks": false, "types": ["Line", "Circle", "Polyline"], "properties": ["Layer", "Length", "Area", "Radius"]}),
        )
        .unwrap();
    assert_eq!(r["columns"], json!(["Count", "Name", "Layer", "Length", "Area", "Radius"]));
    let rs = rows(&r);
    // The two 10-unit lines are one row counted twice.
    assert!(rs.contains(&vec!["2".into(), "Line".into(), "0".into(), "10".into(), "".into(), "".into()]), "{rs:?}");
    assert!(rs.contains(&vec!["1".into(), "Line".into(), "0".into(), "20".into(), "".into(), "".into()]), "{rs:?}");
    let circle = rs.iter().find(|r| r[1] == "Circle").unwrap();
    assert_eq!(circle[5], "2");
    assert!((circle[3].parse::<f64>().unwrap() - std::f64::consts::PI * 4.0).abs() < 1e-3, "{circle:?}");
    assert!((circle[4].parse::<f64>().unwrap() - std::f64::consts::PI * 4.0).abs() < 1e-3, "{circle:?}");
    let square = rs.iter().find(|r| r[1] == "Polyline").unwrap();
    assert_eq!((square[3].as_str(), square[4].as_str()), ("16", "16"));

    // One row per object: handle, layer, position and the properties.
    let r =
        s.execute("dataextraction", &json!({"includeBlocks": false, "types": "Circle", "properties": "Color,Center X", "combine": false})).unwrap();
    assert_eq!(r["columns"], json!(["Name", "Handle", "Layer", "Position X", "Position Y", "Rotation", "Color", "Center X"]));
    assert_eq!(rows(&r)[0][7], "50");

    // Unknown properties and bad lists are refused.
    assert!(s.execute("dataextraction", &json!({"properties": ["Mass"]})).is_err());
    assert!(s.execute("dataextraction", &json!({"types": 7})).is_err());
}

#[test]
fn settings_are_saved_to_a_dxe_file_and_used_again() {
    let mut s = shapes();
    let dir = tmp("dxe");
    let dxe = dir.join("lines.dxe");
    let first = s
        .execute(
            "dataextraction",
            &json!({"includeBlocks": false, "types": ["Line"], "properties": ["Length"], "saveSettings": dxe.to_str().unwrap()}),
        )
        .unwrap();
    let text = std::fs::read_to_string(&dxe).unwrap();
    assert!(text.contains("\"format\": \"cadaddy-dataextraction\"") && text.contains("\"types\""), "{text}");
    let again = s.execute("dataextraction", &json!({"settings": dxe.to_str().unwrap()})).unwrap();
    assert_eq!(again["rows"], first["rows"]);
    // A parameter given with the settings changes them.
    let one_by_one = s.execute("dataextraction", &json!({"settings": dxe.to_str().unwrap(), "combine": false})).unwrap();
    assert_eq!(one_by_one["rows"].as_array().unwrap().len(), 3);
    // Not our format, damaged, or missing: refused.
    let bad = dir.join("bad.dxe");
    for t in ["{\"format\":\"other\"}", "{{{", "{\"properties\":[\"Weight\"]}"] {
        std::fs::write(&bad, t).unwrap();
        assert!(s.execute("dataextraction", &json!({"settings": bad.to_str().unwrap()})).is_err(), "{t}");
    }
    assert!(s.execute("dataextraction", &json!({"settings": dir.join("none.dxe").to_str().unwrap()})).is_err());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn other_drawings_are_extracted_with_a_drawing_column() {
    io();
    let other = shapes();
    let dir = tmp("drawings");
    let path = dir.join("other.dxf");
    std::fs::write(&path, cadaddy_io::write(other.doc().unwrap(), "other.dxf").unwrap()).unwrap();
    let mut s = shapes();
    let r = s.execute("dataextraction", &json!({"includeBlocks": false, "types": ["Circle"], "drawings": [path.to_str().unwrap()]})).unwrap();
    assert_eq!(r["columns"], json!(["Count", "Name", "Drawing"]));
    let rs = rows(&r);
    assert_eq!(rs.len(), 2, "one row per drawing: {rs:?}");
    assert!(rs.iter().any(|r| r[2] == "other.dxf"));
    // Only the other drawing.
    let r = s
        .execute(
            "dataextraction",
            &json!({"includeBlocks": false, "types": ["Circle"], "drawings": [path.to_str().unwrap()], "includeCurrent": false}),
        )
        .unwrap();
    assert_eq!(rows(&r).len(), 1);
    assert!(s.execute("dataextraction", &json!({"drawings": [dir.join("missing.dxf").to_str().unwrap()]})).is_err());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn csv_files_open_in_spreadsheets() {
    let mut s = shapes();
    let dir = tmp("csv");
    let csv = dir.join("lines.csv");
    s.execute("dataextraction", &json!({"includeBlocks": false, "types": ["Line"], "properties": ["Length"], "file": csv.to_str().unwrap()}))
        .unwrap();
    let bytes = std::fs::read(&csv).unwrap();
    assert_eq!(&bytes[..3], &[0xEF, 0xBB, 0xBF], "UTF-8 byte order mark");
    let text = String::from_utf8(bytes[3..].to_vec()).unwrap();
    assert_eq!(text, "Count,Name,Length\r\n2,Line,10\r\n1,Line,20\r\n");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn extraction_tables_update_in_place_and_keep_their_settings_through_save_and_load() {
    io();
    let mut s = shapes();
    let r = s
        .execute("dataextraction", &json!({"includeBlocks": false, "types": ["Line"], "properties": ["Length"], "at": [100, 100], "title": "Lines"}))
        .unwrap();
    let table = r["table"].as_str().unwrap().to_string();
    let cells = |s: &Session| -> usize {
        s.doc().unwrap().model.iter().find_map(|e| if let EntityKind::Table(t) = &e.kind { Some(t.cells.len()) } else { None }).unwrap()
    };
    // Title, header and two rows.
    assert_eq!(cells(&s), 4);
    // A line of a new length: Update adds its row, in place.
    s.execute("line", &json!({"points": [[0, 30], [7, 30]]})).unwrap();
    let u = s.execute("dataextraction", &json!({"update": true})).unwrap();
    assert_eq!(u["updated"], json!([table]));
    assert_eq!(cells(&s), 5);
    let d = s.doc().unwrap();
    let t = d.model.iter().find_map(|e| if let EntityKind::Table(t) = &e.kind { Some(t.clone()) } else { None }).unwrap();
    assert_eq!((t.insert.x, t.insert.y), (100.0, 100.0));
    // Undo takes the update back.
    s.execute("undo", &json!({})).unwrap();
    assert_eq!(cells(&s), 4);

    // Saved and opened again, the table still knows how to update.
    let bytes = cadaddy_io::write(s.doc().unwrap(), "lines.dxf").unwrap();
    let back = cadaddy_io::read(&bytes, "lines.dxf").unwrap();
    let mut s2 = Session::new();
    s2.open_drawing(back, "lines", None);
    s2.execute("line", &json!({"points": [[0, 40], [3, 40]]})).unwrap();
    let u = s2.execute("dataextraction", &json!({"update": true})).unwrap();
    assert_eq!(u["updated"].as_array().unwrap().len(), 1, "{u}");
    assert_eq!(cells(&s2), 6);

    // At the command line: the UPdate tables keyword.
    s2.execute("line", &json!({"points": [[0, 50], [1, 50]]})).unwrap();
    s2.cmdline("dataextraction").unwrap();
    s2.cmdline("up").unwrap();
    assert!(s2.running.is_none(), "{}", s2.prompt_text());
    assert_eq!(cells(&s2), 7);
    // Selecting something that isn't an extraction table is refused.
    let line = s2.execute("line", &json!({"points": [[0, 60], [1, 60]]})).unwrap();
    assert!(s2.execute("dataextraction", &json!({"update": true, "handles": [line["handles"][0]]})).is_err());
}

#[test]
fn battman_adds_attributes_and_switches_them_to_multiline() {
    let mut s = Session::new();
    s.execute("attdef", &json!({"tag": "NAME", "default": "", "at": [0, 0], "height": 0.5})).unwrap();
    s.execute("circle", &json!({"center": [0, 0], "radius": 1})).unwrap();
    let hs: Vec<String> = s.doc().unwrap().model.iter().map(|e| e.handle.hex()).collect();
    s.execute("block", &json!({"name": "TAG", "base": [0, 0], "handles": hs, "keep": "delete"})).unwrap();
    let ins = s.execute("insert", &json!({"name": "TAG", "at": [10, 10], "attribs": {"NAME": "first"}})).unwrap();

    // A new multiline definition, 6 wide: the reference gets it with its default.
    let r = s
        .execute("battman", &json!({"name": "TAG", "action": "add", "tag": "notes", "default": "line one {a}\\b", "multiline": true, "width": 6}))
        .unwrap();
    let notes = r["attributes"].as_array().unwrap().iter().find(|a| a["tag"] == "NOTES").unwrap().clone();
    assert_eq!((notes["multiline"].as_bool(), notes["width"].as_f64()), (Some(true), Some(6.0)), "{notes}");
    let d = s.doc().unwrap();
    let h = cadaddy_engine::doc::Handle(u64::from_str_radix(ins["handle"].as_str().unwrap(), 16).unwrap());
    let EntityKind::Insert(i) = &d.entity(h).unwrap().kind else { panic!() };
    let att = i.attribs.iter().find(|a| a.tag == "NOTES").expect("the reference has the new attribute");
    let ml = att.multiline.as_ref().expect("multiline");
    assert_eq!(ml.mtext.contents, "line one \\{a\\}\\\\b");
    assert!((ml.mtext.insert.x - 10.0).abs() < 1e-9, "placed with the reference");

    // NAME switches to multiline and back; its value on the reference is kept.
    s.execute("battman", &json!({"name": "TAG", "action": "edit", "tag": "name", "multiline": true, "width": 3})).unwrap();
    let EntityKind::Insert(i) = &s.doc().unwrap().entity(h).unwrap().kind else { panic!() };
    let name = i.attribs.iter().find(|a| a.tag == "NAME").unwrap();
    assert!(name.multiline.is_some() && name.text.value == "first");
    s.execute("battman", &json!({"name": "TAG", "action": "edit", "tag": "NAME", "multiline": false})).unwrap();
    let EntityKind::Insert(i) = &s.doc().unwrap().entity(h).unwrap().kind else { panic!() };
    assert!(i.attribs.iter().find(|a| a.tag == "NAME").unwrap().multiline.is_none());

    // At the command line: Edit NAME, Multiline Yes.
    s.cmdline("battman").unwrap();
    for t in ["TAG", "e", "NAME", "m", "y", "x", "x"] {
        s.cmdline(t).unwrap();
    }
    let l = s.execute("battman", &json!({"name": "TAG"})).unwrap();
    assert_eq!(l["attributes"][0]["multiline"], true, "{l}");

    // Hostile: duplicate tag, bad width, bad tag.
    assert!(s.execute("battman", &json!({"name": "TAG", "action": "add", "tag": "NOTES"})).is_err());
    assert!(s.execute("battman", &json!({"name": "TAG", "action": "add", "tag": "W", "multiline": true, "width": -1})).is_err());
    assert!(s.execute("battman", &json!({"name": "TAG", "action": "add", "tag": "has space"})).is_err());
    assert!(s.execute("battman", &json!({"name": "TAG", "action": "edit", "tag": "NAME", "width": f64::MAX})).is_err());
}
