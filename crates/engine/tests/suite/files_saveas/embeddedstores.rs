//! The project data a drawing keeps inside itself (object forms and their change log, the BIM work
//! state with estimate revisions and clash status, reports) survives saving as DXF or DWG and
//! opening again, as it does through .cdy; a format that can't hold it (Release 12 DXF) names
//! each store it leaves out in the save's `notKept`.

use std::sync::Arc;

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
    cadaddy_engine::cmd::file::set_write_streamed(|d, n, o, open| {
        cadaddy_io::write_streamed(d, n, &cadaddy_io::SaveOptions { version: o.version, binary: o.binary }, open).map_err(|e| e.to_string())
    });
}

/// The BIM work state a made-up project keeps: an estimate revision and a clash marked active.
const WORK: &str = r#"{"revisions":{"bid1":{"at":"2026-01-01","total":100.0}},"clash_status":{"c-1":{"status":"active"}}}"#;

/// A drawing with project object forms (two kinds, a field added: three logged changes) and a
/// BIM work state.
fn project() -> Session {
    let mut s = Session::new();
    s.execute("line", &json!({"points": [[0, 0], [10, 0]]})).unwrap();
    s.execute("bim.form.create", &json!({"kinds": ["door", "wall"], "grow": false})).unwrap();
    s.execute("bim.form.field", &json!({"kind": "door", "key": "lock_type", "action": "add", "type": "text"})).unwrap();
    s.doc_mut().unwrap().embedded.insert("bimwork.json".into(), Arc::new(WORK.as_bytes().to_vec()));
    s
}

fn forms(s: &mut Session) -> (usize, usize, bool) {
    let f = s.execute("bim.forms", &json!({})).unwrap();
    let door_lock =
        f["forms"].as_array().unwrap().iter().any(|f| f["kind"] == "door" && f["fields"].as_array().unwrap().iter().any(|x| x["key"] == "lock_type"));
    let log = s.execute("bim.form.log", &json!({})).unwrap();
    let n = if f["project"] == true { f["forms"].as_array().unwrap().len() } else { 0 };
    (n, log["total"].as_u64().unwrap() as usize, door_lock)
}

#[test]
fn project_data_survives_dxf_and_dwg() {
    hooks();
    let dir = std::env::temp_dir().join(format!("cadaddy-embeddedstores-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let mut s = project();
    let before = forms(&mut s);
    assert_eq!(before.0, 2, "two project forms");
    assert!(before.1 >= 1 && before.2, "{before:?}");
    for (name, version) in [("p.dxf", None), ("p.dwg", None), ("p2004.dwg", Some(2004)), ("p2000.dxf", Some(2000)), ("p.cdy", None)] {
        let path = dir.join(name).display().to_string();
        let mut p = json!({"path": path});
        if let Some(v) = version {
            p["version"] = json!(v);
        }
        let r = s.execute("saveas", &p).unwrap();
        let lost: Vec<Value> = r.get("notKept").and_then(Value::as_array).cloned().unwrap_or_default();
        assert!(!lost.iter().any(|l| l == "embedded project data"), "{name}: {r}");
        let mut t = Session::new();
        t.execute("open", &json!({"path": path})).unwrap();
        assert_eq!(forms(&mut t), before, "{name}: the forms, their change log and the added field after reopening");
        let work = t.doc().unwrap().embedded.get("bimwork.json").map(|b| String::from_utf8_lossy(b).into_owned());
        assert_eq!(work.as_deref(), Some(WORK), "{name}: the estimate revision and clash status after reopening");
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn release_12_dxf_names_each_store_it_leaves_out() {
    hooks();
    let dir = std::env::temp_dir().join(format!("cadaddy-embeddedstores-r12-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let mut s = project();
    let r = s.execute("saveas", &json!({"path": dir.join("p12.dxf").display().to_string(), "version": 12})).unwrap();
    let lost = r["notKept"].as_array().cloned().unwrap_or_default();
    assert!(lost.iter().any(|l| l == "embedded project data"), "{r}");
    let detail = r["notKeptDetail"].as_array().cloned().unwrap_or_default();
    let store = detail.iter().find(|l| l["feature"] == "embedded project data").unwrap_or_else(|| panic!("{r}"));
    assert_eq!(store["what"], "dropped");
    let names: Vec<&str> = store["names"].as_array().unwrap().iter().filter_map(Value::as_str).collect();
    assert_eq!(names, ["bimwork.json", "object-forms.json"], "{r}");
    let _ = std::fs::remove_dir_all(&dir);
}
