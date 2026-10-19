//! Agent safety rails: a dry run leaves the drawing identical, one undo reverts a multi-command
//! request, the change journal says what each request did, and saving over a file or closing
//! with changes waits for the user.

use cadaddy_engine::Session;
use serde_json::{Value, json};

fn hooks() {
    cadaddy_engine::cmd::file::set_io(cadaddy_engine::cmd::file::IoHooks {
        read: |b, n| cadaddy_io::read(b, n).map_err(|e| e.to_string()),
        write: |d, n| cadaddy_io::write(d, n).map_err(|e| e.to_string()),
        plot: None,
        read_pdf: None,
    });
}

fn drawing(s: &Session) -> Vec<Value> {
    s.doc().unwrap().model.iter().map(|e| serde_json::to_value(e.as_ref()).unwrap()).collect()
}

#[test]
fn a_dry_run_leaves_the_drawing_identical_and_says_what_would_change() {
    hooks();
    let mut s = Session::new();
    let l = s.execute("line", &json!({"points": [[0, 0], [10, 0]]})).unwrap()["handles"][0].as_str().unwrap().to_string();
    let before = drawing(&s);
    let undo = s.state().unwrap().undo.len();
    s.execute_agent("agent.dryrun", &json!({"on": true})).unwrap();
    let r = s.execute_agent("circle", &json!({"center": [0, 0], "radius": 3})).unwrap();
    assert_eq!(r["dryRun"], true);
    assert_eq!(r["wouldChange"]["counts"], json!([1, 0, 0]), "{r}");
    let r = s.execute_agent("move", &json!({"handles": [l], "from": [0, 0], "to": [5, 5]})).unwrap();
    assert_eq!(r["wouldChange"]["changed"], json!([l]));
    let r = s.execute_agent("erase", &json!({"handles": [l]})).unwrap();
    assert_eq!(r["wouldChange"]["removed"], json!([l]));
    // Nothing is written either.
    let dir = std::env::temp_dir().join(format!("cadaddy-agentguard-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("dry.dxf");
    let saved = s.execute_agent("saveas", &json!({"path": path.display().to_string()}));
    assert!(saved.is_err() || saved.as_ref().is_ok_and(|v| v["dryRun"] == true), "{saved:?}");
    assert!(!path.exists(), "a dry run writes no file");
    // The drawing, its undo and its file name are as they were.
    assert_eq!(drawing(&s), before);
    assert_eq!(s.state().unwrap().undo.len(), undo);
    assert!(s.state().unwrap().path.is_none());
    s.execute_agent("agent.dryrun", &json!({"on": false})).unwrap();
    s.execute_agent("circle", &json!({"center": [0, 0], "radius": 3})).unwrap();
    assert_eq!(drawing(&s).len(), 2, "and off again, commands change the drawing");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn one_undo_reverts_a_multi_command_agent_request_and_the_journal_says_what_it_did() {
    let mut s = Session::new();
    let keep = s.execute("line", &json!({"points": [[0, 0], [10, 0]]})).unwrap()["handles"][0].as_str().unwrap().to_string();
    let before = drawing(&s);
    let r = s.execute_agent("agent.begin", &json!({"label": "Draw a column grid"})).unwrap();
    assert_eq!(r["request"], 1);
    let c = s.execute_agent("circle", &json!({"center": [0, 0], "radius": 3})).unwrap()["handle"].as_str().unwrap().to_string();
    s.execute_agent("rectang", &json!({"p1": [20, 20], "p2": [30, 30]})).unwrap();
    s.execute_agent("move", &json!({"handles": [keep], "from": [0, 0], "to": [0, 5]})).unwrap();
    let e = s.execute_agent("agent.end", &json!({})).unwrap();
    assert_eq!(e["label"], "Draw a column grid");
    assert_eq!(e["commands"], 3);
    assert_eq!(e["counts"], json!([2, 0, 1]), "{e}");
    assert!(e["added"].as_array().unwrap().iter().any(|h| *h == json!(c)));
    assert_eq!(e["changed"], json!([keep]));
    assert_eq!(s.state().unwrap().undo.last().map(|u| u.label.clone()), Some("Agent: Draw a column grid".into()));
    // One undo: as before the request.
    s.execute("undo", &json!({})).unwrap();
    assert_eq!(drawing(&s), before);
    let j = s.execute("agent.journal", &json!({})).unwrap();
    assert_eq!(j["entries"][0]["id"], 1);
    assert!(s.execute("agent.end", &json!({})).is_err(), "no request open");
}

#[test]
fn saving_over_a_file_waits_for_the_user_and_only_the_user_turns_that_off() {
    hooks();
    let dir = std::env::temp_dir().join(format!("cadaddy-agentconfirm-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("plan.dxf");
    let mut s = Session::new();
    s.execute("line", &json!({"points": [[0, 0], [10, 0]]})).unwrap();
    // A new file: no question.
    s.execute_agent("saveas", &json!({"path": path.display().to_string()})).unwrap();
    s.execute("circle", &json!({"center": [0, 0], "radius": 3})).unwrap();
    // Over it: the agent waits for the user.
    let e = s.execute_agent("qsave", &json!({})).unwrap_err();
    assert!(e.to_string().contains("Waiting for the user"), "{e}");
    let p = s.agent.pending.clone().unwrap();
    assert_eq!(p.command, "qsave");
    assert!(p.question.contains("save over"));
    // The agent can't skip the question by turning it off.
    assert!(s.execute_agent("agent.confirm", &json!({"on": false})).is_err());
    assert!(s.agent.confirm);
    // The user can (and their own saves never ask).
    s.execute("qsave", &json!({})).unwrap();
    s.execute("agent.confirm", &json!({"on": false})).unwrap();
    s.execute_agent("qsave", &json!({})).unwrap();
    let _ = std::fs::remove_dir_all(&dir);
}
