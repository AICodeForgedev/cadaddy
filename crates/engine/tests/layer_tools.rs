//! Layer tools (0.7.7): layer notification for new xref layers (LAYEREVAL, LAYERNOTIFY,
//! reconcile), LAYWALK and LAYVPI.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use cadaddy_engine::Session;
use cadaddy_engine::cmd::file::{IoHooks, set_io};
use cadaddy_engine::doc::geom::Vec3;
use cadaddy_engine::doc::{Block, Drawing, Entity, EntityKind, Handle, Insert, Layer, Line, Space};
use serde_json::json;

fn hooks() {
    set_io(IoHooks {
        read: |b, name| cadaddy_io::read(b, name).map_err(|e| e.to_string()),
        write: |d, name| cadaddy_io::write(d, name).map_err(|e| e.to_string()),
        plot: None,
        read_pdf: None,
    });
}

fn folder(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("cadaddy-laytools-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn save(d: &Drawing, path: &Path) {
    std::fs::write(path, cadaddy_io::write(d, &path.to_string_lossy()).unwrap()).unwrap();
}

/// A site plan with a line on each of `layers`.
fn site(layers: &[&str]) -> Drawing {
    let mut d = Drawing::new_metric();
    for (i, n) in layers.iter().enumerate() {
        d.layers.push(Layer { name: (*n).into(), ..Layer::default() });
        let h = d.new_handle();
        let y = i as f64;
        let mut e = Entity::new(h, EntityKind::Line(Line { a: Vec3::new(0.0, y, 0.0), b: Vec3::new(10.0, y, 0.0) }));
        e.common.layer = (*n).into();
        d.model.push(e);
    }
    d
}

fn host() -> Drawing {
    let mut d = Drawing::new_metric();
    let mut b = Block::new("site");
    b.xref_path = Some("site.dxf".into());
    d.blocks.insert("site".into(), Arc::new(b));
    let h = d.new_handle();
    d.model.push(Entity::new(
        h,
        EntityKind::Insert(Insert {
            block: "site".into(),
            insert: Vec3::ZERO,
            scale: Vec3::new(1.0, 1.0, 1.0),
            rotation: 0.0,
            attribs: Vec::new(),
            cols: 1,
            rows: 1,
            col_spacing: 0.0,
            row_spacing: 0.0,
            clip: None,
        }),
    ));
    d
}

fn unreconciled(s: &mut Session) -> Vec<String> {
    let r = s.execute("layer.unreconciled", &json!({})).unwrap();
    r["layers"].as_array().unwrap().iter().map(|v| v.as_str().unwrap().to_string()).collect()
}

#[test]
fn new_xref_layers_are_flagged_until_reconciled_and_survive_a_save() {
    hooks();
    let dir = folder("notify");
    save(&site(&["Walls"]), &dir.join("site.dxf"));
    let path = dir.join("host.dxf");
    save(&Drawing::new_metric(), &path);
    let mut s = Session::new();
    s.execute("open", &json!({ "path": path.to_string_lossy() })).unwrap();
    // Attaching: its layers are the ones asked for.
    s.execute("xattach", &json!({ "path": "site.dxf" })).unwrap();
    assert!(unreconciled(&mut s).is_empty());

    // The site gains a layer; reloading flags it and says so.
    save(&site(&["Walls", "Doors"]), &dir.join("site.dxf"));
    s.log.clear();
    let r = s.execute("-xref", &json!({ "option": "reload", "names": "site" })).unwrap();
    assert_eq!(r["unreconciledLayers"], json!(["site|Doors"]));
    assert_eq!(unreconciled(&mut s), vec!["site|Doors"]);
    assert!(s.log.iter().any(|l| l.contains("Unreconciled new layers: site|Doors")), "{:?}", s.log);

    // Saved and reopened after another change: both are waiting.
    s.execute("qsave", &json!({})).unwrap();
    save(&site(&["Walls", "Doors", "Fixtures"]), &dir.join("site.dxf"));
    let mut t = Session::new();
    t.execute("open", &json!({ "path": path.to_string_lossy() })).unwrap();
    assert_eq!(unreconciled(&mut t), vec!["site|Doors", "site|Fixtures"]);
    assert!(t.log.iter().any(|l| l.contains("Unreconciled new layers")), "reported on open");

    // Reconcile one, then the rest with -LAYER rEconcile.
    let r = t.execute("layer.reconcile", &json!({ "names": ["SITE|DOORS"] })).unwrap();
    assert_eq!(r["reconciled"], json!(["SITE|DOORS"]));
    assert_eq!(unreconciled(&mut t), vec!["site|Fixtures"]);
    assert!(t.execute("layer.reconcile", &json!({ "names": ["nope"] })).is_err());
    t.cmdline("-layer").unwrap();
    t.cmdline("e").unwrap();
    t.cmdline("").unwrap();
    assert!(unreconciled(&mut t).is_empty());

    // LAYEREVAL 0: nothing is evaluated.
    save(&site(&["Walls", "Doors", "Fixtures", "Lights"]), &dir.join("site.dxf"));
    t.execute("setvar", &json!({ "name": "LAYEREVAL", "value": 0 })).unwrap();
    t.execute("-xref", &json!({ "option": "reload", "names": "site" })).unwrap();
    assert!(unreconciled(&mut t).is_empty());
    t.execute("setvar", &json!({ "name": "LAYEREVAL", "value": 1 })).unwrap();
    assert_eq!(unreconciled(&mut t), vec!["site|Lights"]);
    // LAYERNOTIFY without the xref bit: flagged but not announced.
    t.execute("setvar", &json!({ "name": "LAYERNOTIFY", "value": 2 })).unwrap();
    save(&site(&["Walls", "Doors", "Fixtures", "Lights", "Power"]), &dir.join("site.dxf"));
    t.log.clear();
    t.execute("-xref", &json!({ "option": "reload", "names": "site" })).unwrap();
    assert_eq!(unreconciled(&mut t), vec!["site|Lights", "site|Power"]);
    assert!(!t.log.iter().any(|l| l.contains("Unreconciled")));
    // A layer the user makes or renames is reconciled; LAYEREVAL 2 watches drawing layers too.
    t.execute("setvar", &json!({ "name": "LAYEREVAL", "value": 2 })).unwrap();
    t.execute("layer.new", &json!({ "name": "Mine" })).unwrap();
    t.execute("layer.set", &json!({ "name": "Mine", "newName": "Ours" })).unwrap();
    assert_eq!(unreconciled(&mut t), vec!["site|Lights", "site|Power"]);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_drawing_never_evaluated_has_nothing_unreconciled() {
    hooks();
    let dir = folder("fresh");
    save(&site(&["Walls"]), &dir.join("site.dxf"));
    let path = dir.join("host.dxf");
    save(&host(), &path);
    let mut s = Session::new();
    s.execute("open", &json!({ "path": path.to_string_lossy() })).unwrap();
    assert!(s.doc().unwrap().layer("site|Walls").is_some());
    assert!(unreconciled(&mut s).is_empty(), "the layers there on first open are the baseline");
    let r = s.execute("layer.unreconciled", &json!({})).unwrap();
    assert_eq!(r["evaluated"], true);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn laywalk_shows_only_the_picked_layers_and_puts_them_back() {
    let mut s = Session::new();
    for n in ["A-Walls", "A-Doors", "E-Power"] {
        s.execute("layer.new", &json!({ "name": n })).unwrap();
    }
    s.execute("layer.set", &json!({ "name": "A-Doors", "frozen": true })).unwrap();
    s.execute("layer.set", &json!({ "name": "E-Power", "on": false })).unwrap();
    let r = s.execute("laywalk", &json!({ "show": ["a-*"] })).unwrap();
    assert_eq!(r["shown"], json!(["A-Walls", "A-Doors"]));
    let d = s.doc().unwrap();
    assert!(!d.layer("0").unwrap().on && d.layer("A-Doors").unwrap().on && !d.layer("A-Doors").unwrap().frozen);
    // Walking on: the remembered state is the one before the walk.
    s.execute("laywalk", &json!({ "show": "E-Power" })).unwrap();
    assert!(s.doc().unwrap().layer("E-Power").unwrap().on);
    assert!(s.execute("laywalk", &json!({ "show": ["Z*"] })).is_err());
    let r = s.execute("laywalk", &json!({})).unwrap();
    assert_eq!((r["walking"].clone(), r["shown"].clone()), (json!(true), json!(["E-Power"])));
    s.execute("laywalk", &json!({ "end": true })).unwrap();
    let d = s.doc().unwrap();
    assert!(d.layer("0").unwrap().on && d.layer("A-Doors").unwrap().frozen && !d.layer("E-Power").unwrap().on, "restored");
    // Without restoring, what was shown stays.
    s.execute("laywalk", &json!({ "show": ["0"] })).unwrap();
    s.execute("laywalk", &json!({ "end": true, "restore": false })).unwrap();
    assert!(!s.doc().unwrap().layer("A-Walls").unwrap().on);
    assert_eq!(s.execute("laywalk", &json!({ "end": true })).unwrap()["walking"], false);
}

#[test]
fn layvpi_freezes_the_layers_in_the_other_viewports() {
    let mut s = Session::new();
    s.cmdline("line 0,0 100,0").unwrap();
    s.cmdline("").unwrap();
    s.execute("layer.new", &json!({ "name": "Walls" })).unwrap();
    s.execute("mview", &json!({ "layout": "Layout1", "count": 3 })).unwrap();
    s.execute("mview", &json!({ "layout": "Layout2", "count": 1 })).unwrap();
    let vps = |s: &Session, layout: &str| -> Vec<(Handle, Vec<String>)> {
        s.doc()
            .unwrap()
            .layout(layout)
            .unwrap()
            .entities
            .iter()
            .filter_map(|e| match &e.kind {
                EntityKind::Viewport(v) if v.id != 1 => Some((e.handle, v.frozen_layers.clone())),
                _ => None,
            })
            .collect()
    };
    let one = vps(&s, "Layout1");
    assert_eq!(one.len(), 3);
    let target = one[0].0;
    // Already frozen in the target: LAYVPI thaws it there.
    s.execute("vplayer", &json!({ "handle": target.hex(), "freeze": ["walls"] })).unwrap();
    assert!(s.execute("layvpi", &json!({ "layers": ["Walls"] })).is_err(), "no current viewport");
    s.execute("layout", &json!({ "option": "set", "name": "Layout1" })).unwrap();
    let r = s.execute("layvpi", &json!({ "layers": ["walls"], "viewport": target.hex() })).unwrap();
    assert_eq!(r["frozenIn"], 2);
    let one = vps(&s, "Layout1");
    assert!(one[0].1.is_empty());
    assert!(one[1..].iter().all(|(_, f)| f == &vec!["Walls".to_string()]));
    assert!(vps(&s, "Layout2").iter().all(|(_, f)| f.is_empty()), "other layouts untouched");
    s.execute("layvpi", &json!({ "layers": ["0"], "viewport": target.hex(), "allLayouts": true })).unwrap();
    assert!(vps(&s, "Layout2").iter().all(|(_, f)| f == &vec!["0".to_string()]));
    assert!(s.execute("layvpi", &json!({ "layers": ["nope"], "viewport": target.hex() })).is_err());
    assert!(s.execute("layvpi", &json!({ "layers": ["0"], "viewport": "ZZZZ" })).is_err());
    // From objects, inside the viewport.
    assert!(matches!(s.space(), Space::Paper(_)));
    s.execute("undo", &json!({})).unwrap();
}
