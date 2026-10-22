//! Revisions of the prints (Q270): a revised sheet compared with the one the drawing holds.
//! Made-up revisions of the MEP fixtures (known changes, the reissued sheets plotted shifted):
//! aligned, every change clouded and nothing else; an unchanged sheet shows nothing. And on a
//! drawing with MEP runs: the changed runs found, updated and rebuilt alone, an edit kept where
//! the revision didn't touch it and a conflict where it did, and each run's revision log.

use cadaddy_engine::Session;
use cadaddy_engine::cmd::wallgen::made_as;
use cadaddy_engine::doc::{EntityKind, Handle};
use cadaddy_engine::geom::{Mat3, Vec2};
use cadaddy_mepfix::revision::{RevisionPlan, revise};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

fn io() {
    cadaddy_engine::cmd::file::set_io(cadaddy_engine::cmd::file::IoHooks {
        read: |b, n| cadaddy_io::read(b, n).map_err(|e| e.to_string()),
        write: |d, n| cadaddy_io::write(d, n).map_err(|e| e.to_string()),
        plot: None,
        read_pdf: None,
    });
}

fn dir(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("cadaddy-printrev-{name}-{}", std::process::id()));
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn opened(bytes: &[u8], title: &str) -> Session {
    let d = cadaddy_io::read(bytes, "sheet.dxf").unwrap();
    let mut s = Session::empty();
    s.open_drawing(d, title, None);
    s
}

fn write(path: &Path, bytes: &[u8]) -> String {
    std::fs::write(path, bytes).unwrap();
    path.display().to_string()
}

fn inside(r: &Value, p: [f64; 2], pad: f64) -> bool {
    let (min, max) = (&r["min"], &r["max"]);
    let f = |v: &Value| v.as_f64().unwrap();
    p[0] >= f(&min[0]) - pad && p[0] <= f(&max[0]) + pad && p[1] >= f(&min[1]) - pad && p[1] <= f(&max[1]) + pad
}

#[test]
fn a_revised_fixture_sheet_is_aligned_and_every_change_clouded() {
    io();
    let plan = RevisionPlan::default();
    let rev = revise(7, &plan).unwrap();
    let out = dir("fixture");
    let changed = rev.changed_sheets();
    assert!(!changed.is_empty());
    for number in &changed {
        let sh = rev.issued.truth.sheets.iter().find(|s| &s.number == number).unwrap();
        let mine: Vec<_> = rev.changes.iter().filter(|c| &c.sheet == number).collect();
        let mut s = opened(rev.issued.file(&sh.dxf).unwrap(), number);
        let against = write(&out.join(format!("rev-{}", sh.dxf)), rev.revised.file(&sh.dxf).unwrap());
        let r = s.execute("printrev.compare", &json!({"against": against, "label": plan.label})).unwrap();
        let off = r["offset"].as_array().unwrap();
        assert!(
            (off[0].as_f64().unwrap() - plan.shift_in[0]).abs() < 1e-6 && (off[1].as_f64().unwrap() - plan.shift_in[1]).abs() < 1e-6,
            "{number}: {r}"
        );
        assert!(r["aligned_by"].as_u64().unwrap() >= 3, "{number}: {}", r["aligned_by"]);
        let regions = r["regions"].as_array().unwrap();
        let ext = s.doc().unwrap().extents(&cadaddy_engine::doc::Space::Model);
        let gap = ext.width().max(ext.height()) / 50.0;
        // Every known change is in a cloud...
        for c in &mine {
            assert!(
                regions.iter().any(|g| inside(g, c.was_at_in, gap)),
                "{number}: {} {} at {:?} not clouded: {regions:?}",
                c.kind,
                c.item,
                c.was_at_in
            );
        }
        // ...and every cloud holds a known change: nothing else differs.
        for g in regions {
            assert!(mine.iter().any(|c| inside(g, c.was_at_in, gap * 3.0)), "{number}: a cloud with no change in it: {g}; changes {mine:?}");
            assert!(g["cloud"].is_string(), "{g}");
        }
        // The resized ducts' tags read the new size now.
        for c in mine.iter().filter(|c| c.kind == "resize") {
            let texts = r["changes"].as_array().unwrap().iter().filter(|x| x["kind"] == "text");
            assert!(
                texts.clone().any(|x| x["to"].as_str().is_some_and(|t| t.contains(&c.to))),
                "{number}: {} → {}: {:?}",
                c.from,
                c.to,
                r["changes"]
            );
        }
        // The drawing took the revision: compared again, the two are the same.
        let again = s.execute("printrev.compare", &json!({"against": against, "label": "check"})).unwrap();
        assert_eq!(
            (again["added"].as_u64(), again["removed"].as_u64(), again["retexted"].as_u64()),
            (Some(0), Some(0), Some(0)),
            "{number}: {again}"
        );
        assert!(again["regions"].as_array().unwrap().is_empty());
        // The clouds are on their own layer, with the revision's label.
        let d = s.doc().unwrap();
        let labels: Vec<String> = d
            .model
            .iter()
            .filter(|e| e.common.layer == "REV-CLOUD")
            .filter_map(|e| match &e.kind {
                EntityKind::Text(t) => Some(t.value.clone()),
                _ => None,
            })
            .collect();
        assert_eq!(labels.len(), regions.len());
        assert!(labels.iter().all(|l| *l == plan.label));
    }
    // A sheet the revision didn't touch: nothing.
    let same = rev.issued.truth.sheets.iter().find(|s| !changed.contains(&s.number) && s.discipline == "M").unwrap();
    let mut s = opened(rev.issued.file(&same.dxf).unwrap(), &same.number);
    let against = write(&out.join(format!("rev-{}", same.dxf)), rev.revised.file(&same.dxf).unwrap());
    let r = s.execute("printrev.compare", &json!({"against": against})).unwrap();
    assert_eq!((r["added"].as_u64(), r["removed"].as_u64(), r["retexted"].as_u64()), (Some(0), Some(0), Some(0)), "{r}");
    assert_eq!(r["offset"], json!([0.0, 0.0]));
    assert!(r["regions"].as_array().unwrap().is_empty());
    std::fs::remove_dir_all(&out).ok();
}

// ---------------- MEP runs ----------------

fn layer(s: &mut Session, name: &str) {
    if s.execute("layer.new", &json!({"name": name, "current": true})).is_err() {
        s.execute("layer.current", &json!({"name": name})).unwrap();
    }
}

fn text(s: &mut Session, at: [f64; 2], t: &str) {
    s.execute("text", &json!({"at": at, "text": t, "height": 4})).unwrap();
}

/// A floor with a supply duct (24x12 at 10'), a cold water pipe and a return duct, and notes
/// in the title area. `rev`: the revision's version — the supply duct 28x12, the cold water
/// gone, a conduit added; `rev2` also lowers the supply duct to 9'.
fn floor(rev: u8) -> Session {
    let mut s = Session::new();
    layer(&mut s, "A-WALL");
    s.execute("pline", &json!({"vertices": [[-12, -12], [492, -12], [492, 372], [-12, 372]], "closed": true})).unwrap();
    layer(&mut s, "G-ANNO");
    for (i, t) in ["LEVEL 1 MECHANICAL PLAN", "PROJECT A", "SHEET M-101", "SCALE 1/4\" = 1'-0\"", "KEYNOTES"].iter().enumerate() {
        text(&mut s, [500.0, 300.0 - 20.0 * i as f64], t);
    }
    layer(&mut s, "M-DUCT-SUPP");
    s.execute("pline", &json!({"vertices": [[24, 60], [264, 60], [264, 180]]})).unwrap();
    let size = if rev >= 1 { "28x12" } else { "24x12" };
    let height = if rev >= 2 { "BOT @ 9'-0\"" } else { "BOT @ 10'-0\"" };
    text(&mut s, [180.0, 70.0], &format!("{size} {height}"));
    layer(&mut s, "M-DUCT-RETN");
    s.execute("pline", &json!({"vertices": [[24, 300], [400, 300]]})).unwrap();
    text(&mut s, [200.0, 310.0], "20x10 BOT @ 11'-0\"");
    if rev == 0 {
        layer(&mut s, "P-DOMW-CW");
        s.execute("line", &json!({"points": [[100, 0], [100, 120]]})).unwrap();
        text(&mut s, [104.0, 20.0], "2\" CW CL @ 10'-6\"");
    } else {
        layer(&mut s, "E-COND");
        s.execute("line", &json!({"points": [[300, 200], [420, 200]]})).unwrap();
        text(&mut s, [340.0, 206.0], "1\" C BOT @ 12'-0\"");
    }
    s.execute(
        "bspec.set",
        &json!({"replace": true, "spec": {
            "levels": [{"name": "L1", "elevation_ft": 0, "height_ft": 14, "ceiling_ft": 9, "region": [-100, -100, 1000, 1000], "base": [0, 0]}],
            "layers": [{"layer": "A-WALL", "role": "wall"}]
        }}),
    )
    .unwrap();
    s
}

/// The revision `rev` of the floor as a file, plotted shifted.
fn revised(rev: u8, path: &Path) -> String {
    let mut s = floor(rev);
    let m = Mat3::translate(Vec2::new(100.0, 50.0));
    let d = s.doc_mut().unwrap();
    for h in d.model.handles() {
        d.model.modify(h, |e| e.kind.transform(&m));
    }
    write(path, cadaddy_io::write(s.doc().unwrap(), "x.dxf").unwrap().as_slice())
}

fn mep_meshes(s: &Session) -> Vec<(Handle, String)> {
    s.doc().unwrap().model.iter().filter(|e| made_as(e).as_deref() == Some("mep")).map(|e| (e.handle, e.common.layer.clone())).collect()
}

fn find<'a>(r: &'a Value, change: &str, system: &str) -> &'a Value {
    r["runs"].as_array().unwrap().iter().find(|x| x["change"] == change && x["system"] == system).unwrap_or_else(|| panic!("{change} {system}: {r}"))
}

#[test]
fn changed_runs_are_updated_rebuilt_alone_and_logged() {
    io();
    let out = dir("mep");
    let mut s = floor(0);
    s.execute("mep.read", &json!({"apply": true})).unwrap();
    s.execute("model.build", &json!({})).unwrap();
    let before = mep_meshes(&s);
    let retn: Vec<Handle> = before.iter().filter(|(_, l)| l.contains("M-DUCT-RETN")).map(|(h, _)| *h).collect();
    assert!(!retn.is_empty(), "{before:?}");
    // Someone lowers the supply duct by hand (the print said 10').
    let list = s.execute("mep.list", &json!({"system": "M-DUCT-SUPP"})).unwrap();
    let supply = list["items"][0]["id"].as_u64().unwrap();
    s.execute("mep.edit", &json!({"id": supply, "bottom_ft": 9.5})).unwrap();

    let r = s.execute("printrev.compare", &json!({"against": revised(1, &out.join("rev1.dxf")), "label": "ASI-01"})).unwrap();
    assert_eq!(r["offset"], json!([100.0, 50.0]), "{r}");
    let resized = find(&r, "resized", "M-DUCT-SUPP");
    assert_eq!((resized["from"].as_str(), resized["to"].as_str()), (Some("24x12"), Some("28x12")));
    let supply_id = resized["id"].as_str().unwrap().to_string();
    find(&r, "removed", "P-DOMW-CW");
    find(&r, "added", "E-COND");
    assert!(r["runs"].as_array().unwrap().iter().all(|x| x["system"] != "M-DUCT-RETN"), "the return duct didn't change: {r}");
    // The hand edit to the height stays: the revision changed only the size.
    assert_eq!(r["kept"], json!([{"id": supply_id, "part": "height"}]), "{r}");
    assert_eq!(r["conflicts"], json!([]));
    let spec = s.execute("mep.list", &json!({"system": "M-DUCT-SUPP"})).unwrap();
    assert_eq!(spec["items"][0]["size_in"], json!([28.0, 12.0]));
    assert_eq!(spec["items"][0]["bottoms_ft"][0], json!(9.5));
    // Only the changed runs were rebuilt; the return duct's 3D is the same objects.
    assert_eq!(r["rebuilt"], json!(2), "the supply duct and the new conduit: {r}");
    let after = mep_meshes(&s);
    for h in &retn {
        assert!(after.iter().any(|(a, _)| a == h), "the return duct's 3D stays");
    }
    assert!(!after.iter().any(|(_, l)| l.contains("P-DOMW-CW")), "the removed pipe's 3D is gone");
    assert!(after.iter().any(|(_, l)| l.contains("E-COND")), "the new conduit is built");
    let lit: Vec<String> = r["highlighted"].as_array().unwrap().iter().map(|h| h.as_str().unwrap().to_string()).collect();
    assert!(!lit.is_empty());
    assert_eq!(s.selection().len(), lit.len(), "the rebuilt runs are selected");
    assert!(lit.iter().all(|h| after.iter().any(|(a, l)| a.hex() == *h && !l.contains("M-DUCT-RETN"))));

    // The next revision lowers the duct to 9': the print and the hand edit disagree; the print wins.
    let r2 = s.execute("printrev.compare", &json!({"against": revised(2, &out.join("rev2.dxf")), "label": "ASI-02"})).unwrap();
    let raised = find(&r2, "raised", "M-DUCT-SUPP");
    assert_eq!(raised["id"].as_str(), Some(supply_id.as_str()), "the run keeps its id from one revision to the next");
    assert_eq!(r2["conflicts"], json!([{"id": supply_id, "part": "height", "edit": "9'-6\"", "print": "9'-0\""}]), "{r2}");
    let spec = s.execute("mep.list", &json!({"system": "M-DUCT-SUPP"})).unwrap();
    assert_eq!(spec["items"][0]["bottoms_ft"][0], json!(9.0));

    // The run's revision log.
    let log = s.execute("printrev.log", &json!({"item": supply_id})).unwrap();
    let entries: Vec<(&str, &str, &str, &str)> = log["entries"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| (e["revision"].as_str().unwrap(), e["change"].as_str().unwrap(), e["from"].as_str().unwrap_or(""), e["to"].as_str().unwrap_or("")))
        .collect();
    assert_eq!(entries, vec![("ASI-01", "resized", "24x12", "28x12"), ("ASI-02", "raised", "10'-0\"", "9'-0\"")]);
    assert_eq!(log["revisions"], json!(["ASI-01", "ASI-02"]));
    let all = s.execute("printrev.log", &json!({})).unwrap();
    assert!(all["entries"].as_array().unwrap().len() >= 4, "{all}");
    // A default label counts the revisions so far.
    let r3 = s.execute("printrev.compare", &json!({"against": out.join("rev2.dxf").display().to_string(), "apply": false})).unwrap();
    assert_eq!(r3["revision"], json!("Rev 3"));
    // Picked from the menu: the file, then the label, on the command line.
    let r4 = out.join("rev2.dxf").display().to_string();
    s.start("printrev.compare").unwrap();
    assert!(s.prompt_text().contains("revised sheet"), "{}", s.prompt_text());
    s.input(cadaddy_engine::Input::Text(out.join("none.dxf").display().to_string())).unwrap();
    assert_eq!(s.running.as_ref().map(|r| r.id.as_str()), Some("printrev.compare"), "a missing file asks again");
    s.input(cadaddy_engine::Input::Text(r4)).unwrap();
    assert!(s.prompt_text().contains("label"), "{}", s.prompt_text());
    s.input(cadaddy_engine::Input::Text("ASI-03".into())).unwrap();
    assert!(s.running.is_none());
    let log = s.execute("printrev.log", &json!({"revision": "ASI-03"})).unwrap();
    assert!(log["entries"].as_array().unwrap().is_empty(), "nothing changed since ASI-02: {log}");
    // Bad calls.
    assert!(s.execute("printrev.compare", &json!({})).is_err());
    assert!(s.execute("printrev.compare", &json!({"against": out.join("none.dxf").display().to_string()})).is_err());
    assert!(s.execute("printrev.compare", &json!({"against": out.join("rev2.dxf").display().to_string(), "tolerance": -1})).is_err());
    std::fs::remove_dir_all(&out).ok();
}
