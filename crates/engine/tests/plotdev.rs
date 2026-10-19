//! Binary plot style tables in the Plot Style Manager, and plotting to a system printer through
//! the host's print hooks.

use std::sync::Mutex;

use cadaddy_engine::Session;
use cadaddy_engine::cmd::file::{IoHooks, PrintHooks, set_io, set_print};
use serde_json::json;

/// What the fake printer was given: (printer, bytes, title).
static PRINTED: Mutex<Vec<(String, usize, String)>> = Mutex::new(Vec::new());

fn session() -> Session {
    set_io(IoHooks {
        read: |_, _| Err("no reader".into()),
        write: |_, _| Err("no writer".into()),
        plot: Some(|_, _, opts| Ok(format!("%PDF {}", opts["output"]).into_bytes())),
        read_pdf: None,
    });
    set_print(PrintHooks {
        list: || Ok(vec!["Office Laser".into(), "Plotter".into()]),
        print: |name, pdf, title| {
            if name == "Jammed" {
                return Err("paper jam".into());
            }
            PRINTED.lock().unwrap().push((name.to_string(), pdf.len(), title.to_string()));
            Ok(())
        },
    });
    let mut s = Session::new();
    s.execute("line", &json!({"points": [[0, 0], [100, 50]]})).unwrap();
    s
}

#[test]
fn plots_go_to_a_system_printer_as_pdf() {
    let mut s = session();
    assert_eq!(s.execute("printers", &json!({})).unwrap()["printers"], json!(["Office Laser", "Plotter"]));
    let r = s.execute("plot", &json!({"printer": "Office Laser", "device": "CADaddy PNG"})).unwrap();
    assert_eq!(r["printed"], "Office Laser");
    let printed = PRINTED.lock().unwrap().clone();
    let (name, bytes, _) = printed.last().unwrap();
    assert_eq!(name, "Office Laser");
    assert_eq!(*bytes, "%PDF \"pdf\"".len(), "printed as PDF whatever the device");
    let e = s.execute("plot", &json!({"printer": "Jammed"})).unwrap_err().to_string();
    assert!(e.contains("paper jam"), "{e}");
}

#[test]
fn binary_tables_load_and_save() {
    let mut s = session();
    let dir = std::env::temp_dir().join(format!("cadaddy-plotdev-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    // A named table of our own, with a style, saved as .stb (binary) and loaded back.
    s.execute("stylesmanager", &json!({"action": "new", "name": "office.stb", "kind": "named"})).unwrap();
    s.execute(
        "stylesmanager",
        &json!({"action": "set", "name": "office.stb", "style": {"name": "Heavy", "color": "#102030", "lineweight": 0.7, "screening": 80}}),
    )
    .unwrap();
    let path = dir.join("office.stb").display().to_string();
    let r = s.execute("stylesmanager", &json!({"action": "save", "name": "office.stb", "path": path})).unwrap();
    assert_eq!(r["format"], "binary");
    assert!(std::fs::read(&path).unwrap().starts_with(b"PIAFILEVERSION_2.0,STBVER1"));
    let mut t = Session::new();
    let r = t.execute("stylesmanager", &json!({"action": "load", "path": path})).unwrap();
    assert_eq!(r["name"], "office.stb", "{r}");
    let got = t.execute("stylesmanager", &json!({"action": "get", "name": "office.stb"})).unwrap();
    let text = got.to_string();
    assert!(text.contains("Heavy") && text.contains("#102030"), "{text}");
    // Base64 both ways, and JSON still on request.
    let r = s.execute("stylesmanager", &json!({"action": "save", "name": "office.stb", "format": "binary"})).unwrap();
    let data = r["data"].as_str().unwrap().to_string();
    let r = t.execute("stylesmanager", &json!({"action": "load", "data": data, "name": "copy.stb"})).unwrap();
    assert_eq!(r["name"], "copy.stb");
    let json_path = dir.join("office.json").display().to_string();
    assert_eq!(s.execute("stylesmanager", &json!({"action": "save", "name": "office.stb", "path": json_path})).unwrap()["format"], "json");
    assert!(t.execute("stylesmanager", &json!({"action": "load", "path": json_path, "name": "j.stb"})).is_ok());
    // Damaged binary data is an error.
    assert!(t.execute("stylesmanager", &json!({"action": "load", "data": "UElBRklMRVZFUlNJT04="})).is_err());
    assert!(s.execute("stylesmanager", &json!({"action": "save", "name": "office.stb", "format": "xml"})).is_err());
    let _ = std::fs::remove_dir_all(&dir);
}
