//! DWGPROPS and the script tools (Q148): drawing properties set by DWGPROPS round-trip through
//! DXF, DWG and .cdy and show in fields; SCRIPTCALL, DELAY, RSCRIPT and RESUME in scripts (a
//! script repeating itself runs a bounded number of times); SYSVARMONITOR reports and resets.

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

fn folder(tag: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("cadaddy-dwgprops-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn props(s: &mut Session) -> Value {
    s.execute("dwgprops", &json!({})).unwrap()
}

#[test]
fn properties_are_set_kept_in_every_format_and_undone() {
    hooks();
    let dir = folder("formats");
    let mut s = Session::new();
    let mut custom = serde_json::Map::new();
    for i in 0..12 {
        custom.insert(format!("Field {i}"), json!(format!("value {i}")));
    }
    custom.insert("Phase".into(), json!("Design"));
    let r = s
        .execute(
            "dwgprops",
            &json!({
                "title": "Sample Floor Plan", "subject": "Level 1", "author": "Project A team", "keywords": "plan, level 1",
                "comments": "Made-up drawing for tests", "revisionNumber": "3", "hyperlinkBase": "https://example.com/project-a/",
                "custom": custom
            }),
        )
        .unwrap();
    assert_eq!(r["summary"]["title"], "Sample Floor Plan");
    assert_eq!(r["hyperlinkBase"], "https://example.com/project-a/");
    assert_eq!(r["custom"].as_array().unwrap().len(), 13);
    assert!(r["statistics"]["objects"].is_number() && r["general"]["name"].is_string());

    // Removing one, changing one; undo puts them back.
    s.execute("dwgprops", &json!({ "custom": { "Field 0": null, "Phase": "Construction" } })).unwrap();
    assert_eq!(props(&mut s)["custom"].as_array().unwrap().len(), 12);
    s.execute("undo", &json!({})).unwrap();
    let p = props(&mut s);
    assert_eq!(p["custom"].as_array().unwrap().len(), 13, "{p}");
    assert!(p["custom"].to_string().contains("Design"));

    for ext in ["cdy", "dxf", "dwg"] {
        let path = dir.join(format!("Sample Floor Plan.{ext}"));
        s.execute("saveas", &json!({ "path": path })).unwrap();
        let mut r = Session::new();
        r.execute("open", &json!({ "path": path })).unwrap();
        let back = props(&mut r);
        assert_eq!(back["summary"], p["summary"], "{ext}: {back}");
        assert_eq!(back["hyperlinkBase"], p["hyperlinkBase"], "{ext}");
        let n = back["custom"].as_array().unwrap().len();
        // A DXF or DWG file holds ten custom properties (groups 300–309); .cdy all.
        assert_eq!(n, if ext == "cdy" { 13 } else { 10 }, "{ext}: {back}");
        assert_eq!(back["custom"][0], p["custom"][0], "{ext}: the first ones, in order");
        if ext == "dxf" {
            let text = std::fs::read_to_string(&path).unwrap();
            let keys = text.lines().filter(|l| l.trim() == "DWGPROPS").count();
            assert!(text.contains("DWGPROPS COOKIE") && keys == 1, "one DWGPROPS entry: {keys}, cookies {}", text.matches("DWGPROPS COOKIE").count());
            // Saved again: still one.
            let again = dir.join("again.dxf");
            r.execute("saveas", &json!({ "path": again })).unwrap();
            assert_eq!(std::fs::read_to_string(&again).unwrap().matches("DWGPROPS COOKIE").count(), 1, "not kept twice");
        }
    }
    // Bad input is refused.
    assert!(s.execute("dwgprops", &json!({ "title": 5 })).is_err());
    assert!(s.execute("dwgprops", &json!({ "custom": { "a=b": "x" } })).is_err());
    assert!(s.execute("dwgprops", &json!({ "custom": [1] })).is_err());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn fields_show_the_properties() {
    hooks();
    let mut s = Session::new();
    s.execute("dwgprops", &json!({ "title": "Sample Floor Plan", "author": "Project A team", "custom": { "Phase": "Design" } })).unwrap();
    let r = s
        .execute("mtext", &json!({ "at": [0, 0], "text": "%<\\AcVar Title>% by %<\\AcVar Author>% (%<\\AcVar CustomDP.Phase>%)", "height": 2.5 }))
        .unwrap();
    let h = cadaddy_engine::doc::Handle::parse_hex(r["handle"].as_str().unwrap()).unwrap();
    let values = |s: &Session| s.doc().unwrap().field_values.get(&h).cloned().unwrap_or_default();
    assert_eq!(values(&s), vec!["Sample Floor Plan", "Project A team", "Design"]);
    s.execute("dwgprops", &json!({ "custom": { "Phase": "Construction" } })).unwrap();
    s.execute("updatefield", &json!({ "handles": [h.hex()] })).unwrap();
    assert_eq!(values(&s), vec!["Sample Floor Plan", "Project A team", "Construction"]);
}

fn lines(s: &Session) -> usize {
    s.doc().unwrap().model.iter().filter(|e| e.kind.type_name() == "Line").count()
}

#[test]
fn scripts_call_scripts_wait_and_repeat_a_bounded_number_of_times() {
    hooks();
    let dir = folder("scripts");
    let inner = dir.join("Draw one line.scr");
    std::fs::write(&inner, "LINE 0,0 10,0 \n").unwrap();
    let outer = format!("; made-up setup script\nSCRIPTCALL \"{}\"\nDELAY 1\nRSCRIPT\n", inner.display());
    // A called script is checked like any (SECURELOAD): not from an untrusted folder.
    let mut s = Session::new();
    let e = s.script(&outer).unwrap_err().to_string();
    assert!(e.contains("trusted"), "{e}");
    assert_eq!(lines(&s), 0);
    let trust = |s: &mut Session| s.execute("setvar", &json!({ "name": "TRUSTEDPATHS", "value": dir.display().to_string() })).unwrap();
    let mut s = Session::new();
    trust(&mut s);
    let t = std::time::Instant::now();
    s.script(&outer).unwrap();
    // One run, then RSCRIPT's 1000 repeats: each calls the inner script once.
    assert_eq!(lines(&s), cadaddy_engine::script_tools::MAX_REPEATS + 1);
    assert!(s.log.iter().any(|l| l.contains("RSCRIPT: stopped after 1000 repeats")), "{:?}", &s.log[s.log.len().saturating_sub(5)..]);
    assert!(t.elapsed().as_secs() < 60, "{:?}", t.elapsed());

    // A script that calls itself stops at the depth limit.
    let me = dir.join("Calls itself.scr");
    std::fs::write(&me, format!("LINE 0,5 10,5 \nSCRIPTCALL \"{}\"\n", me.display())).unwrap();
    let mut s = Session::new();
    trust(&mut s);
    let e = s.script(&format!("SCRIPTCALL \"{}\"\n", me.display())).unwrap_err().to_string();
    assert!(e.contains("more than 8 deep"), "{e}");
    assert_eq!(lines(&s), cadaddy_engine::script_tools::MAX_DEPTH, "each level drew its line");

    // RSCRIPT at the command line runs the last script again; DELAY on its own waits.
    let mut s = Session::new();
    s.script("LINE 0,0 1,1 \n").unwrap();
    s.execute("rscript", &json!({})).unwrap();
    assert_eq!(lines(&s), 2);
    let t = std::time::Instant::now();
    s.execute("delay", &json!({ "ms": 30 })).unwrap();
    assert!(t.elapsed().as_millis() >= 30);
    assert!(s.execute("delay", &json!({})).is_err());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn resume_carries_on_after_a_failed_line() {
    hooks();
    let mut s = Session::new();
    // The middle line fails (a command that doesn't exist); the rest waits.
    let e = s.script("LINE 0,0 1,0 \nnosuchcommand {}\nCIRCLE 0,0 5\n");
    assert!(e.is_err());
    assert_eq!(lines(&s), 1);
    assert_eq!(s.doc().unwrap().model.iter().filter(|e| e.kind.type_name() == "Circle").count(), 0);
    let r = s.execute("resume", &json!({})).unwrap();
    assert_eq!(r["resumed"], true);
    assert_eq!(s.doc().unwrap().model.iter().filter(|e| e.kind.type_name() == "Circle").count(), 1, "carried on with the next line");
    assert_eq!(s.execute("resume", &json!({})).unwrap()["resumed"], false, "nothing left");
}

#[test]
fn the_monitor_reports_changes_and_resets_them() {
    hooks();
    let mut s = Session::new();
    let r = s.execute("sysvarmonitor", &json!({ "variables": ["ORTHOMODE", "filletrad"], "notify": true })).unwrap();
    assert_eq!(r["variables"].as_array().unwrap().len(), 2, "{r}");
    let orig = r["variables"][1]["watched"].clone();
    s.execute("setvar", &json!({ "name": "FILLETRAD", "value": 7.5 })).unwrap();
    assert!(s.log.iter().any(|l| l.contains("SYSVARMONITOR: FILLETRAD changed")), "{:?}", &s.log[s.log.len().saturating_sub(4)..]);
    let n = s.log.len();
    s.execute("line", &json!({ "points": [[0, 0], [1, 1]] })).unwrap();
    assert!(!s.log[n..].iter().any(|l| l.contains("SYSVARMONITOR")), "reported once");
    let r = s.execute("sysvarmonitor", &json!({})).unwrap();
    assert_eq!(r["changes"].as_array().unwrap().len(), 1, "{r}");
    let r = s.execute("sysvarmonitor", &json!({ "reset": true })).unwrap();
    assert_eq!(r["variables"][1]["now"], orig, "back to the watched value: {r}");
    assert!(r["changes"].as_array().unwrap().is_empty());
    assert!(s.execute("sysvarmonitor", &json!({ "add": ["NOTAVARIABLE"] })).is_err());
    let r = s.execute("sysvarmonitor", &json!({ "remove": ["ORTHOMODE"] })).unwrap();
    assert_eq!(r["variables"].as_array().unwrap().len(), 1);
}
