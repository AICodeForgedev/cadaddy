//! The second round of output writers (battle test 7): DWFOUT, DWFXOUT, TABLEEXPORT, RENDER and
//! PUBLISH never write over the open drawing, never replace an existing unrelated file without
//! `force: true`; and PLOT / EXPORTPDF / RENDER / ETRANSMIT accept the `force` their refusal offers.

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
        write_dwfx: Some(cadaddy_io::write_dwfx_sheets),
    });
}

fn dir(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("cadaddy-outguard3-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn text(p: &Path) -> String {
    p.to_string_lossy().into_owned()
}

/// A session whose drawing (a line and a table) is saved at `d/name`; returns the table's handle.
fn saved(d: &Path, name: &str) -> (Session, String, String) {
    hooks();
    let mut s = Session::new();
    s.execute("line", &json!({"points": [[0, 0], [100, 50]]})).unwrap();
    let t = s.execute("table", &json!({ "at": [50, 0], "rows": 1, "cols": 2, "cells": [["Door", "Window"]] })).unwrap();
    let h = t["handle"].as_str().or_else(|| t["table"].as_str()).unwrap_or_default().to_string();
    let path = text(&d.join(name));
    s.execute("saveas", &json!({ "path": path })).unwrap();
    (s, path, h)
}

/// Writers that take their output from `path`, with the extension that suits them.
fn writers(table: &str) -> Vec<(&'static str, Value, &'static str)> {
    vec![
        ("dwfout", json!({}), "dwf"),
        ("dwfxout", json!({}), "dwfx"),
        ("tableexport", json!({ "handle": table }), "csv"),
        ("tableexport", json!({ "handle": table }), "xlsx"),
        ("render", json!({ "width": 32, "height": 32 }), "png"),
        ("publish", json!({ "layouts": ["Model"] }), "pdf"),
        ("publish", json!({ "layouts": ["Model"], "format": "dwfx" }), "dwfx"),
    ]
}

fn with_path(mut p: Value, path: &str) -> Value {
    p["path"] = json!(path);
    p
}

#[test]
fn the_writers_never_write_over_the_open_drawing() {
    let d = dir("open");
    let (mut s, path, table) = saved(&d, "victim.dxf");
    let before = std::fs::read(&path).unwrap();
    for (cmd, p, ext) in writers(&table) {
        if cmd == "publish" && ext == "pdf" {
            continue; // writes NAME-<sheet>.pdf beside the name given: never the drawing itself
        }
        for force in [false, true] {
            let mut q = with_path(p.clone(), &path);
            q["force"] = json!(force);
            s.execute(cmd, &q).expect_err(cmd);
            assert_eq!(std::fs::read(&path).unwrap(), before, "{cmd} (force {force}) changed the open drawing's file");
        }
    }
}

#[test]
fn the_writers_refuse_an_existing_file_without_force_and_replace_it_with() {
    let d = dir("existing");
    let (mut s, _, table) = saved(&d, "plan.dxf");
    for (i, (cmd, p, ext)) in writers(&table).into_iter().enumerate() {
        if cmd == "render" {
            continue; // a plain drawing has no surfaces to render: covered below
        }
        // publish writes NAME-<sheet>.pdf when it is given a .pdf name and not `single`
        let victim = d.join(format!("out{i}.{ext}"));
        let written = if cmd == "publish" && ext == "pdf" { d.join(format!("out{i}-Model.pdf")) } else { victim.clone() };
        std::fs::write(&written, b"precious").unwrap();
        let e = s.execute(cmd, &with_path(p.clone(), &text(&victim))).expect_err(cmd).to_string();
        assert!(e.contains("force:true"), "{cmd}: {e}");
        assert_eq!(std::fs::read(&written).unwrap(), b"precious", "{cmd} replaced an existing file");
        let mut q = with_path(p.clone(), &text(&victim));
        q["force"] = json!(true);
        s.execute(cmd, &q).unwrap_or_else(|e| panic!("{cmd} force: {e}"));
        assert_ne!(std::fs::read(&written).unwrap(), b"precious", "{cmd} with force kept the old file");
    }
}

#[test]
fn a_fresh_path_still_works() {
    let d = dir("fresh");
    let (mut s, _, table) = saved(&d, "plan.dxf");
    for (i, (cmd, p, ext)) in writers(&table).into_iter().enumerate() {
        if cmd == "render" {
            continue;
        }
        let out = d.join(format!("fresh{i}.{ext}"));
        s.execute(cmd, &with_path(p, &text(&out))).unwrap_or_else(|e| panic!("{cmd}: {e}"));
    }
}

#[test]
fn render_is_vetted_before_it_renders_and_force_is_a_parameter() {
    let d = dir("render");
    let (mut s, path, _) = saved(&d, "plan.dxf");
    let victim = d.join("view.png");
    std::fs::write(&victim, b"precious").unwrap();
    let e = s.execute("render", &json!({ "path": text(&victim) })).expect_err("existing").to_string();
    assert!(e.contains("force:true"), "{e}");
    // with force the file check passes (a line has no surface, so the render itself has nothing to draw)
    let e = s.execute("render", &json!({ "path": text(&victim), "force": true })).expect_err("nothing to render").to_string();
    assert!(e.contains("nothing to render") || e.contains("There is nothing"), "{e}");
    assert_eq!(std::fs::read(&victim).unwrap(), b"precious");
    let before = std::fs::read(&path).unwrap();
    s.execute("render", &json!({ "output": path, "force": true })).expect_err("open drawing");
    assert_eq!(std::fs::read(&path).unwrap(), before);
}

#[test]
fn plot_exportpdf_and_etransmit_accept_the_force_their_refusal_offers() {
    let d = dir("force");
    let (mut s, _, _) = saved(&d, "plan.dxf");
    s.agent.confirm = false;
    for (cmd, ext) in [("plot", "pdf"), ("exportpdf", "pdf"), ("etransmit", "zip")] {
        let victim = d.join(format!("sheet-{cmd}.{ext}"));
        std::fs::write(&victim, b"precious").unwrap();
        let e = s.execute_agent(cmd, &json!({ "path": text(&victim) })).expect_err(cmd).to_string();
        assert!(e.contains("force:true"), "{cmd}: {e}");
        assert!(!e.contains("is not a parameter"), "{cmd}: {e}");
        assert_eq!(std::fs::read(&victim).unwrap(), b"precious", "{cmd} replaced an existing file");
        s.execute_agent(cmd, &json!({ "path": text(&victim), "force": true })).unwrap_or_else(|e| panic!("{cmd} force: {e}"));
        assert_ne!(std::fs::read(&victim).unwrap(), b"precious", "{cmd} with force kept the old file");
    }
}

#[test]
fn publish_checks_every_sheet_before_writing_any() {
    let d = dir("sheets");
    let (mut s, _, _) = saved(&d, "plan.dxf");
    s.execute("layout.new", &json!({ "name": "Sheet2" })).unwrap();
    let layouts = ["Model", "Sheet2"];
    let victim = d.join("set-Sheet2.pdf");
    std::fs::write(&victim, b"precious").unwrap();
    let e = s.execute("publish", &json!({ "path": text(&d.join("set.pdf")), "layouts": layouts })).expect_err("existing sheet").to_string();
    assert!(e.contains("force:true"), "{e}");
    assert!(!d.join("set-Model.pdf").exists(), "the first sheet was written before the second was vetted");
    assert_eq!(std::fs::read(&victim).unwrap(), b"precious");
}

#[test]
fn the_folder_form_of_publish_keeps_existing_pdfs_without_force() {
    let d = dir("folder");
    let (mut s, path, _) = saved(&d, "plan.dxf");
    let victim = d.join("plan.pdf");
    std::fs::write(&victim, b"precious").unwrap();
    let r = s.execute("publish", &json!({ "folder": text(&d) })).unwrap();
    assert_eq!(std::fs::read(&victim).unwrap(), b"precious");
    assert!(r["failed"][0]["error"].as_str().is_some_and(|e| e.contains("force:true")), "{r}");
    s.execute("publish", &json!({ "folder": text(&d), "force": true })).unwrap();
    assert_ne!(std::fs::read(&victim).unwrap(), b"precious");
    assert!(Path::new(&path).exists());
}
