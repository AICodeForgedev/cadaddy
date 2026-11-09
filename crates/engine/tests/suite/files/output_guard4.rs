//! The fourth round of output writers (battle test 8): ATTOUT, DRYWALL.TAKEOFF, COBIE.EXPORT,
//! HANDOVER.SCHEDULES, COORDINATION.BCF and BCF.EXPORT wrote over the open drawing; CONCRETE.QTY,
//! SCHEDULE.EXPORT and SAVEIMG replaced an existing file without `force: true`. Every one goes
//! through the output guard now: never the open drawing, never an existing file without `force`,
//! and the file's extension matches what is written. A sweep runs every command whose documented
//! parameters name an output file against the open drawing's own path, so a new writer that skips
//! the guard fails here too.

use crate::helper::{hooks, scratch, serial};
use cadaddy_engine::{Session, command_specs, documented_params, params_doc};
use serde_json::{Value, json};
use std::path::Path;

fn layer(s: &mut Session, name: &str) {
    if s.execute("layer.new", &json!({"name": name, "current": true})).is_err() {
        s.execute("layer.current", &json!({"name": name})).unwrap();
    }
}

fn line(s: &mut Session, a: [f64; 2], b: [f64; 2]) {
    s.execute("line", &json!({"points": [a, b]})).unwrap();
}

fn rect(s: &mut Session, x0: f64, y0: f64, x1: f64, y1: f64) {
    line(s, [x0, y0], [x1, y0]);
    line(s, [x1, y0], [x1, y1]);
    line(s, [x1, y1], [x0, y1]);
    line(s, [x0, y1], [x0, y0]);
}

fn text(p: &Path) -> String {
    p.to_string_lossy().into_owned()
}

/// A made-up two-room plan (6" walls, a door, room tags, two tagged receptacle blocks), its
/// model built, a BCF topic, a construction sequence and a render, saved as `d/name`.
pub(crate) fn built(d: &Path, name: &str) -> (Session, String) {
    hooks();
    let mut s = Session::new();
    layer(&mut s, "A-WALL");
    rect(&mut s, 0.0, 0.0, 480.0, 360.0);
    rect(&mut s, 6.0, 6.0, 474.0, 354.0);
    for x in [238.0, 242.0] {
        line(&mut s, [x, 6.0], [x, 150.0]);
        line(&mut s, [x, 186.0], [x, 354.0]);
    }
    layer(&mut s, "A-DOOR");
    s.execute("arc", &json!({"center": [240.0, 150.0], "radius": 36.0, "start": 0.0, "end": 90.0})).unwrap();
    layer(&mut s, "A-AREA-IDEN");
    for (x, room, n) in [(100.0, "OFFICE", "101"), (340.0, "LOBBY", "102")] {
        s.execute("text", &json!({"at": [x, 180.0], "text": room, "height": 8.0})).unwrap();
        s.execute("text", &json!({"at": [x, 160.0], "text": n, "height": 6.0})).unwrap();
    }
    layer(&mut s, "E-POWR");
    s.execute("attdef", &json!({"tag": "TAG", "default": "R", "at": [0, 4], "height": 2})).unwrap();
    s.execute("circle", &json!({"center": [0, 0], "radius": 3})).unwrap();
    let all: Vec<String> = s.doc().unwrap().model.iter().map(|e| e.handle.hex()).collect();
    let parts = all.get(all.len() - 2..).unwrap().to_vec();
    s.execute("block", &json!({"name": "RCPT", "base": [0, 0], "handles": parts, "keep": "delete"})).unwrap();
    for (x, tag) in [(60.0, "A"), (300.0, "B")] {
        s.execute("insert", &json!({"name": "RCPT", "at": [x, 9.0], "attribs": {"TAG": tag}})).unwrap();
    }
    s.execute("bspec.draft", &json!({"store": true})).unwrap();
    s.execute("model.build", &json!({})).unwrap();
    s.execute("bcf.create", &json!({"title": "t"})).unwrap();
    s.execute("schedule.build", &json!({"start": "2026-12-01"})).unwrap();
    s.execute("render", &json!({"width": 32, "height": 32, "samples": 1})).unwrap();
    s.set_selection(Vec::new());
    let path = text(&d.join(name));
    s.execute("saveas", &json!({ "path": path })).unwrap();
    (s, path)
}

/// The nine writers, each with the parameter that names its output and an extension that suits it.
fn writers() -> Vec<(&'static str, &'static str, Value, &'static str)> {
    vec![
        ("attout", "path", json!({}), "txt"),
        ("drywall.takeoff", "file", json!({}), "csv"),
        ("cobie.export", "path", json!({}), "xlsx"),
        ("handover.schedules", "path", json!({}), "xlsx"),
        ("coordination.bcf", "path", json!({}), "bcf"),
        ("bcf.export", "path", json!({}), "bcfzip"),
        ("concrete.qty", "file", json!({}), "csv"),
        ("schedule.export", "path", json!({}), "csv"),
        ("saveimg", "path", json!({}), "png"),
    ]
}

fn with(mut p: Value, key: &str, path: &str) -> Value {
    p[key] = json!(path);
    p
}

#[test]
fn the_nine_writers_never_write_over_the_open_drawing() {
    let _turn = serial();
    let d = scratch("outguard4-open");
    for name in ["m.cdy", "m.dxf"] {
        let (mut s, path) = built(&d, name);
        let before = std::fs::read(&path).unwrap();
        for (cmd, key, p, _) in writers() {
            for force in [false, true] {
                let mut q = with(p.clone(), key, &path);
                q["force"] = json!(force);
                let r = s.execute(cmd, &q);
                assert!(r.is_err(), "{cmd} {key}={name} (force {force}) ran: {r:?}");
                assert_eq!(std::fs::read(&path).unwrap(), before, "{cmd} (force {force}) changed the open drawing {name}");
            }
        }
    }
}

#[test]
fn the_nine_writers_refuse_an_existing_file_without_force_and_replace_it_with() {
    let _turn = serial();
    let d = scratch("outguard4-existing");
    let (mut s, _) = built(&d, "plan.cdy");
    for (i, (cmd, key, p, ext)) in writers().into_iter().enumerate() {
        let victim = d.join(format!("out{i}.{ext}"));
        std::fs::write(&victim, b"PRECIOUS").unwrap();
        let e = s.execute(cmd, &with(p.clone(), key, &text(&victim))).expect_err(cmd).to_string();
        assert!(e.contains("force:true"), "{cmd}: {e}");
        assert_eq!(std::fs::read(&victim).unwrap(), b"PRECIOUS", "{cmd} replaced an existing file without force");
        let mut q = with(p.clone(), key, &text(&victim));
        q["force"] = json!(true);
        s.execute(cmd, &q).unwrap_or_else(|e| panic!("{cmd} force: {e}"));
        assert_ne!(std::fs::read(&victim).unwrap(), b"PRECIOUS", "{cmd} with force kept the old file");
    }
}

#[test]
fn a_fresh_path_still_works_for_each_writer() {
    let _turn = serial();
    let d = scratch("outguard4-fresh");
    let (mut s, _) = built(&d, "plan.cdy");
    for (i, (cmd, key, p, ext)) in writers().into_iter().enumerate() {
        let out = d.join(format!("fresh{i}.{ext}"));
        s.execute(cmd, &with(p, key, &text(&out))).unwrap_or_else(|e| panic!("{cmd}: {e}"));
        assert!(out.exists(), "{cmd} wrote {}", out.display());
    }
}

#[test]
fn a_workbook_or_archive_is_never_written_under_another_kind_of_name() {
    let _turn = serial();
    let d = scratch("outguard4-ext");
    let (mut s, _) = built(&d, "plan.cdy");
    for (cmd, key, ext) in [
        ("cobie.export", "path", "pdf"),
        ("handover.schedules", "path", "csv"),
        ("coordination.bcf", "path", "xlsx"),
        ("bcf.export", "path", "txt"),
        ("concrete.qty", "file", "png"),
        ("saveimg", "path", "pdf"),
    ] {
        let out = d.join(format!("wrong-{}.{ext}", cmd.replace('.', "-")));
        let e = s.execute(cmd, &json!({ key: text(&out) })).expect_err(cmd).to_string();
        assert!(e.contains(&format!(".{ext}")), "{cmd}: {e}");
        assert!(!out.exists(), "{cmd} wrote {}", out.display());
    }
    // A format the call names must agree with the name's extension.
    let out = d.join("q.xlsx");
    s.execute("concrete.qty", &json!({ "file": text(&out), "format": "csv" })).expect_err("csv bytes in an .xlsx name");
    assert!(!out.exists());
}

/// Parameter names that hold the file an output command writes.
pub(crate) const OUTPUT_KEYS: &[&str] = &["path", "file", "output", "out", "png", "csv", "xlsx", "pdf", "dest", "target", "to", "report"];

/// Commands that write a DRAWING to the path they are given (saving it is their job, the drawing's
/// own path included), or that don't write their path at all but read it or act on the drawing.
pub(crate) const NOT_OUTPUT_WRITERS: &[&str] = &[
    // they save the drawing itself: the open drawing's own path is a plain save
    "save",
    "saveas",
    "qsave",
    "saveall",
    "-saveas",
    // they open, insert, attach or read the path (the guard is for writers)
    "open",
    "-open",
    "close",
    "quit",
    "exit",
    "recover",
    "drawingrecovery",
];

#[test]
fn no_command_writes_over_the_open_drawing() {
    let _turn = serial();
    let d = scratch("outguard4-sweep");
    let (_, path) = built(&d, "victim.cdy");
    let before = std::fs::read(&path).unwrap();
    let mut changed = Vec::new();
    let mut tried = 0;
    for spec in command_specs() {
        if NOT_OUTPUT_WRITERS.contains(&spec.id) {
            continue;
        }
        let names = documented_params(params_doc(spec));
        let keys: Vec<&str> = OUTPUT_KEYS.iter().copied().filter(|k| names.contains(k)).collect();
        if keys.is_empty() {
            continue;
        }
        let mut s = Session::new();
        s.execute("open", &json!({ "path": path })).unwrap();
        for key in keys {
            tried += 1;
            let _ = s.execute(spec.id, &json!({ key: path, "force": true }));
            if std::fs::read(&path).map_or(true, |now| now != before) {
                changed.push(format!("{} {{{key}: <the open drawing>}}", spec.id));
                std::fs::write(&path, &before).unwrap();
            }
        }
    }
    eprintln!("open-drawing sweep: {tried} output parameters probed, {} wrote over it", changed.len());
    assert!(tried > 50, "probed {tried} output parameters");
    assert!(changed.is_empty(), "wrote over the open drawing:\n{}", changed.join("\n"));
}

/// The kinds of file the output commands write: each probe names an existing file of that kind.
pub(crate) const DATA_EXTS: &[&str] =
    &["csv", "xlsx", "txt", "tsv", "json", "xml", "png", "jpg", "pdf", "svg", "bcf", "bcfzip", "zip", "ifc", "html", "md", "dwf", "dwfx"];

/// Drawing and package writers whose plain call replaces the file it names, as a save does (the
/// app's own file dialogs ask first): an agent's call (MCP, the control channel) is held for the
/// user, or refused when there is nobody to ask, by the overwrite rail in `agent.rs`.
const AGENT_RAILED: &[&str] = &["export", "wblock", "etransmit"];

#[test]
fn no_command_replaces_an_existing_file_without_force() {
    let _turn = serial();
    let d = scratch("outguard4-sweep-existing");
    let (_, path) = built(&d, "plan.cdy");
    let mut replaced = Vec::new();
    let mut tried = 0;
    for spec in command_specs() {
        if NOT_OUTPUT_WRITERS.contains(&spec.id) {
            continue;
        }
        let names = documented_params(params_doc(spec));
        let keys: Vec<&str> = OUTPUT_KEYS.iter().copied().filter(|k| names.contains(k)).collect();
        if keys.is_empty() {
            continue;
        }
        let mut s = Session::new();
        s.execute("open", &json!({ "path": path })).unwrap();
        for key in keys {
            for ext in DATA_EXTS {
                tried += 1;
                let victim = d.join(format!("existing.{ext}"));
                std::fs::write(&victim, b"PRECIOUS").unwrap();
                let q = json!({ key: text(&victim) });
                let _ = if AGENT_RAILED.contains(&spec.id) {
                    s.agent.confirm = false;
                    s.execute_agent(spec.id, &q)
                } else {
                    s.execute(spec.id, &q)
                };
                if std::fs::read(&victim).map_or(true, |now| now != b"PRECIOUS") {
                    replaced.push(format!("{} {{{key}: <an existing .{ext}>}}", spec.id));
                }
            }
        }
    }
    eprintln!("existing-file sweep: {tried} probes, {} replaced the file", replaced.len());
    assert!(tried > 500, "probed {tried} output parameters");
    assert!(replaced.is_empty(), "replaced an existing file without force:true:\n{}", replaced.join("\n"));
}
