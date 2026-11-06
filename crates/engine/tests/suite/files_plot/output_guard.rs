//! Commands that write a file from a parameter never replace the open drawing, and never replace
//! another existing file unless the call says `force: true`; an agent's SAVEAS / WBLOCK / EXPORT
//! with no user to ask is refused the same way.

use cadaddy_engine::Session;
use cadaddy_engine::cmd::file::{IoHooks, set_io};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

fn hooks() {
    set_io(IoHooks {
        read: |b, name| cadaddy_io::read(b, name).map_err(|e| e.to_string()),
        write: |d, name| cadaddy_io::write(d, name).map_err(|e| e.to_string()),
        plot: Some(|d, space, opts| cadaddy_io::plotter::plot(d, space, opts).map_err(|e| e.to_string())),
        read_pdf: None,
        read_dwf: None,
        write_dwfx: None,
    });
}

fn dir(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("cadaddy-outguard-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn text(p: &Path) -> String {
    p.to_string_lossy().into_owned()
}

/// A session whose drawing is saved at `d/name`.
fn saved(d: &Path, name: &str) -> (Session, String) {
    hooks();
    let mut s = Session::new();
    s.execute("line", &json!({"points": [[0, 0], [100, 50]]})).unwrap();
    let path = text(&d.join(name));
    s.execute("saveas", &json!({ "path": path })).unwrap();
    (s, path)
}

/// Every command with the parameter that names its output file and the extension that suits it.
const COMMANDS: [(&str, &str, &str); 21] = [
    ("takeoff", "file", "csv"),
    ("automation.takeoff", "path", "csv"),
    ("validate.prescreen", "path", "json"),
    ("takeoff.layers", "file", "csv"),
    ("dataextraction", "file", "csv"),
    ("eattext", "file", "csv"),
    ("estimate.export", "path", "csv"),
    ("bim.bom", "file", "csv"),
    ("bim.takeoff", "file", "csv"),
    ("bim.estimate", "file", "csv"),
    ("pull.schedule", "file", "csv"),
    ("pull.cutlist", "file", "csv"),
    ("pull.release", "file", "csv"),
    ("model.overlay", "path", "png"),
    ("project.rules.export", "path", "json"),
    ("fieldview.export", "path", "json"),
    ("model.clashes.report", "path", "pdf"),
    ("rfi.export", "path", "csv"),
    ("attext", "file", "txt"),
    ("bim.loadcalc", "csv", "csv"),
    ("bim.export.ar", "path", "glb"),
];

fn params(cmd: &str, key: &str, path: &str) -> Value {
    let mut p = json!({ key: path });
    if cmd == "attext" {
        p["format"] = json!("dxf");
    }
    p
}

#[test]
fn data_commands_never_write_over_the_open_drawing() {
    let d = dir("open");
    let (mut s, path) = saved(&d, "victim.dxf");
    let before = std::fs::read(&path).unwrap();
    for (cmd, key, _) in COMMANDS {
        // Refused (by the open-drawing check, or first by a wrong-extension check).
        s.execute(cmd, &params(cmd, key, &path)).expect_err(cmd);
        assert_eq!(std::fs::read(&path).unwrap(), before, "{cmd} changed the drawing's file");
    }
}

#[test]
fn data_commands_refuse_an_existing_file_without_force() {
    let d = dir("existing");
    let (mut s, _) = saved(&d, "plan.dxf");
    for (cmd, key, ext) in COMMANDS {
        let victim = d.join(format!("{}.{ext}", cmd.replace('.', "_")));
        std::fs::write(&victim, b"precious").unwrap();
        let e = s.execute(cmd, &params(cmd, key, &text(&victim))).expect_err(cmd).to_string();
        assert!(e.contains("force:true") && e.contains(&text(&victim)), "{cmd}: {e}");
        assert_eq!(std::fs::read(&victim).unwrap(), b"precious", "{cmd} replaced an existing file");
    }
}

#[test]
fn a_fresh_path_still_works_and_force_replaces() {
    let d = dir("fresh");
    let (mut s, _) = saved(&d, "plan.dxf");
    for (cmd, key, ext) in
        [("takeoff", "file", "csv"), ("dataextraction", "file", "csv"), ("project.rules.export", "path", "json"), ("attext", "file", "txt")]
    {
        let out = d.join(format!("{}.{ext}", cmd.replace('.', "_")));
        s.execute(cmd, &params(cmd, key, &text(&out))).unwrap_or_else(|e| panic!("{cmd}: {e}"));
        assert!(out.exists(), "{cmd} wrote nothing");
        std::fs::write(&out, b"old").unwrap();
        let mut p = params(cmd, key, &text(&out));
        p["force"] = json!(true);
        s.execute(cmd, &p).unwrap_or_else(|e| panic!("{cmd} force: {e}"));
        assert_ne!(std::fs::read(&out).unwrap(), b"old", "{cmd} with force kept the old file");
    }
}

#[test]
fn a_data_command_is_not_given_a_drawing_file_name() {
    let d = dir("kind");
    let (mut s, _) = saved(&d, "plan.dxf");
    let other = text(&d.join("other.dwg"));
    let e = s.execute("takeoff", &json!({ "file": other })).expect_err("dwg").to_string();
    assert!(e.contains("drawing file name"), "{e}");
    assert!(!d.join("other.dwg").exists());
}

fn headless(s: &mut Session) {
    s.agent.confirm = false;
}

#[test]
fn an_agents_saveas_wblock_and_export_never_replace_another_drawing() {
    let d = dir("drawings");
    let (mut s, _) = saved(&d, "mine.dxf");
    headless(&mut s);
    for (i, ext) in ["dxf", "dwg"].into_iter().enumerate() {
        let other = d.join(format!("other{i}.{ext}"));
        std::fs::write(&other, b"someone else's drawing").unwrap();
        let path = text(&other);
        let calls = [
            ("saveas", json!({ "path": path })),
            ("export", json!({ "path": path, "format": ext })),
            ("wblock", json!({ "path": path, "handles": ["0"] })),
        ];
        for (cmd, p) in calls {
            let e = s.execute_agent(cmd, &p).expect_err(cmd).to_string();
            assert!(e.contains("force:true"), "{cmd} {ext}: {e}");
            assert_eq!(std::fs::read(&other).unwrap(), b"someone else's drawing", "{cmd} {ext} replaced it");
        }
        // With force:true the save replaces it.
        s.execute_agent("saveas", &json!({ "path": path, "force": true })).unwrap();
        assert_ne!(std::fs::read(&other).unwrap(), b"someone else's drawing");
    }
}

#[test]
fn an_agents_fresh_save_and_its_own_file_still_work() {
    let d = dir("own");
    let (mut s, own) = saved(&d, "mine.dxf");
    headless(&mut s);
    s.execute_agent("saveas", &json!({ "path": text(&d.join("fresh.dxf")) })).unwrap();
    assert!(d.join("fresh.dxf").exists());
    s.execute_agent("qsave", &json!({})).unwrap();
    s.execute_agent("saveas", &json!({ "path": text(&d.join("fresh.dxf")) })).unwrap();
    let _ = own;
}

#[test]
fn wblock_never_writes_over_an_open_drawing_even_forced() {
    let d = dir("wblock");
    let (mut s, path) = saved(&d, "mine.dwg");
    let before = std::fs::read(&path).unwrap();
    let h = s.execute("line", &json!({"points": [[0, 0], [5, 5]]})).unwrap();
    let handle = h["handle"].as_str().unwrap_or("0").to_string();
    let e = s.execute("wblock", &json!({ "path": path, "handles": [handle], "force": true })).expect_err("wblock").to_string();
    assert!(e.contains("open drawing"), "{e}");
    assert_eq!(std::fs::read(&path).unwrap(), before);
}
