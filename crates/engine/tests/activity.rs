//! Activity history (ACTIVITY): every step of a short script logged with its command and how many
//! objects it added, changed and erased; a step's objects selected; the log saved beside the
//! drawing and read back; ACTIVITYLOG off records nothing. One test: ACTIVITYLOG is process-wide.

use cadaddy_engine::Session;
use cadaddy_engine::cmd::file::{IoHooks, set_io};
use serde_json::{Value, json};

fn entries(s: &mut Session) -> Vec<Value> {
    s.execute("activity", &json!({})).unwrap()["entries"].as_array().unwrap().clone()
}

#[test]
fn a_short_script_is_logged_saved_and_read_back() {
    set_io(IoHooks {
        read: |b, name| cadaddy_io::read(b, name).map_err(|e| e.to_string()),
        write: |d, name| cadaddy_io::write(d, name).map_err(|e| e.to_string()),
        plot: None,
        read_pdf: None,
    });
    let mut s = Session::new();
    s.script("LINE 0,0 10,0 10,10\n\nCIRCLE 5,5 2\nCOPY L  0,0 20,0\n\n").unwrap();
    let copied = s.doc().unwrap().model.iter().last().unwrap().handle.hex();
    s.execute("erase", &json!({ "handles": [copied] })).unwrap();
    let log = entries(&mut s);
    let cmds: Vec<&str> = log.iter().map(|e| e["command"].as_str().unwrap()).collect();
    assert_eq!(cmds.len(), 4, "{log:?}");
    // Newest first: ERASE took the copied circle, COPY added one, CIRCLE one, LINE two.
    assert_eq!(
        (log[0]["erased"].as_u64(), log[1]["added"].as_u64(), log[2]["added"].as_u64(), log[3]["added"].as_u64()),
        (Some(1), Some(1), Some(1), Some(2))
    );
    assert!(log.iter().all(|e| e["time"].as_u64().unwrap() > 1_700_000_000));
    // Filter by command, and the line step's objects selected.
    let lines = s.execute("activity", &json!({ "command": "line" })).unwrap();
    assert_eq!(lines["count"], 1);
    let seq = lines["entries"][0]["seq"].as_u64().unwrap();
    let r = s.execute("activity", &json!({ "action": "show", "seq": seq })).unwrap();
    assert_eq!((r["handles"].as_array().unwrap().len(), r["missing"].as_u64()), (2, Some(0)));
    assert_eq!(s.selection().len(), 2);
    // The circle copied then erased: its step's object is gone.
    let copy = log[1]["seq"].as_u64().unwrap();
    assert_eq!(s.execute("activity", &json!({ "action": "show", "seq": copy })).unwrap()["missing"], 1);
    // Undo and redo are not steps of their own.
    s.execute("undo", &json!({})).unwrap();
    assert_eq!(entries(&mut s).len(), 4);
    assert!(s.execute("activity", &json!({ "action": "show", "seq": 999 })).is_err());

    // Saved beside the drawing, read back on open.
    let dir = std::env::temp_dir().join(format!("cadaddy-activity-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("plan.dxf");
    let p = path.to_string_lossy().to_string();
    s.execute("saveas", &json!({ "path": p })).unwrap();
    let side = format!("{p}.activity.json");
    let text = std::fs::read_to_string(&side).unwrap();
    assert!(text.contains("\"command\"") && !text.to_lowercase().contains("user"), "no user names: {text}");
    let mut t = Session::new();
    t.execute("open", &json!({ "path": p })).unwrap();
    assert_eq!(entries(&mut t).len(), 4);
    t.execute("activity", &json!({ "action": "clear" })).unwrap();
    assert!(entries(&mut t).is_empty());

    // ACTIVITYLOG 0 (Options): nothing recorded or written.
    t.execute("options", &json!({ "ACTIVITYLOG": 0 })).unwrap();
    t.execute("line", &json!({ "points": [[0, 0], [1, 1]] })).unwrap();
    assert!(entries(&mut t).is_empty());
    assert_eq!(t.execute("activity", &json!({})).unwrap()["enabled"], false);
    t.execute("setvar", &json!({ "name": "ACTIVITYLOG", "value": 1 })).unwrap();
    t.execute("line", &json!({ "points": [[0, 0], [2, 2]] })).unwrap();
    assert_eq!(entries(&mut t).len(), 1);
    std::fs::remove_dir_all(&dir).ok();
}
