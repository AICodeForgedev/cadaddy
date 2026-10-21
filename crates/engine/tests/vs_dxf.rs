//! The made-up 2D test set for DXF exchange with other programs (Q229): every 2D entity type, layers
//! with colours/linetypes/lineweights, a text style, a block with attributes, every dimension type,
//! leaders and hatches, written as DXF R12 and 2018.
//!
//! `CADADDY_VS_OUT=<folder> cargo test -p cadaddy-engine --test vs_dxf -- --ignored` writes the
//! files to open in the other programs. The tests here check that CADaddy reads its own files of
//! the set back with every entity, and the fixes the comparison found (each with the made-up input
//! that showed it).

use cadaddy_engine::Session;
use cadaddy_engine::cmd::file::{IoHooks, set_io};
use cadaddy_engine::doc::geom::Vec3;
use cadaddy_engine::doc::{Common, Drawing, EntityKind, Solid, Space};
use cadaddy_io::{SaveOptions, Version};
use serde_json::{Value, json};

fn hooks() {
    set_io(IoHooks {
        read: |b, name| cadaddy_io::read(b, name).map_err(|e| e.to_string()),
        write: |d, name| cadaddy_io::write(d, name).map_err(|e| e.to_string()),
        plot: None,
        read_pdf: None,
    });
    cadaddy_engine::cmd::file::set_write_as(|d, n, o| {
        cadaddy_io::write_with(d, n, &SaveOptions { version: o.version, binary: o.binary }).map_err(|e| e.to_string())
    });
}

/// The set: a drawing built with CADaddy's commands, and the commands that failed (none expected).
pub fn test_set() -> (Drawing, Vec<String>) {
    hooks();
    let mut s = Session::new();
    let mut failed = Vec::new();
    let mut run = |s: &mut Session, id: &str, p: Value| {
        if let Err(e) = s.execute(id, &p) {
            failed.push(format!("{id}: {e}"));
        }
    };
    let last = |s: &Session| s.doc().map(|d| d.model.iter().last().map(|e| e.handle.hex()).unwrap_or_default()).unwrap_or_default();
    // Tables.
    run(&mut s, "linetype", json!({ "load": "DASHED" }));
    run(&mut s, "linetype", json!({ "load": "CENTER" }));
    run(&mut s, "layer.new", json!({ "name": "WALLS", "color": "red", "lineweight": 0.5 }));
    run(&mut s, "layer.new", json!({ "name": "HIDDEN", "color": 30, "linetype": "DASHED" }));
    run(&mut s, "layer.new", json!({ "name": "AXES", "color": "blue", "linetype": "CENTER" }));
    run(&mut s, "layer.new", json!({ "name": "NOTES", "color": 3 }));
    run(&mut s, "layer.new", json!({ "name": "DIMS", "color": 6 }));
    run(&mut s, "style", json!({ "name": "Notes", "font": "Arial", "height": 0, "current": false }));
    // Curves.
    run(&mut s, "layer.current", json!({ "name": "WALLS" }));
    run(&mut s, "line", json!({ "points": [[0, 0], [40, 0]] }));
    run(&mut s, "point", json!({ "at": [2, 2] }));
    run(&mut s, "circle", json!({ "center": [60, 10], "radius": 8 }));
    run(&mut s, "arc", json!({ "center": [85, 10], "radius": 6, "start": 10, "end": 160 }));
    run(&mut s, "ellipse", json!({ "center": [110, 10], "major": [8, 2], "ratio": 0.5 }));
    run(&mut s, "ellipse", json!({ "center": [130, 10], "major": [6, 0], "ratio": 0.4, "start": 0, "end": 180 }));
    run(&mut s, "pline", json!({ "vertices": [[0, 20], { "p": [15, 20], "bulge": 0.5 }, [30, 30]], "closed": false }));
    run(&mut s, "pline", json!({ "vertices": [[40, 20], { "p": [50, 20], "startWidth": 0.5, "endWidth": 1.5 }, [55, 28]], "closed": false }));
    run(&mut s, "rectang", json!({ "p1": [60, 20], "p2": [75, 30] }));
    run(&mut s, "spline", json!({ "fit": [[0, 40], [8, 46], [16, 40], [24, 46]] }));
    run(&mut s, "spline", json!({ "control": [[30, 40], [36, 48], [42, 40], [48, 48]], "degree": 3 }));
    run(&mut s, "layer.current", json!({ "name": "AXES" }));
    run(&mut s, "ray", json!({ "base": [150, 0], "through": [151, 1] }));
    run(&mut s, "xline", json!({ "base": [160, 0], "angle": 90 }));
    run(&mut s, "layer.current", json!({ "name": "HIDDEN" }));
    run(&mut s, "line", json!({ "points": [[0, -10], [40, -10]] }));
    // Text.
    run(&mut s, "layer.current", json!({ "name": "NOTES" }));
    run(&mut s, "text", json!({ "at": [0, 60], "text": "PLAN %%c25", "height": 2.5 }));
    run(&mut s, "text", json!({ "at": [40, 60], "text": "Middle centre", "height": 2, "rotation": 15, "justify": "MC" }));
    run(&mut s, "text", json!({ "at": [80, 60], "text": "Right", "height": 2, "justify": "R" }));
    run(&mut s, "mtext", json!({ "at": [0, 75], "text": "First line\\PSecond line", "width": 30, "height": 2 }));
    run(&mut s, "mtext", json!({ "at": [40, 75], "text": "{\\fArial|b1;Bold} and plain", "width": 40, "height": 2 }));
    // A block with attributes, inserted twice.
    run(&mut s, "attdef", json!({ "tag": "ROOM", "prompt": "Room number", "default": "101", "at": [0, -3], "height": 1 }));
    let mut hs = vec![last(&s)];
    run(&mut s, "circle", json!({ "center": [0, 0], "radius": 2 }));
    hs.push(last(&s));
    run(&mut s, "line", json!({ "points": [[-2, 0], [2, 0]] }));
    hs.push(last(&s));
    run(&mut s, "block", json!({ "name": "TAG", "base": [0, 0], "handles": hs, "keep": "delete" }));
    run(&mut s, "insert", json!({ "name": "TAG", "at": [100, 60], "attribs": { "ROOM": "204" } }));
    run(&mut s, "insert", json!({ "name": "TAG", "at": [115, 60], "scale": [1.5, 1.5], "rotation": 30, "attribs": { "ROOM": "205" } }));
    // Dimensions and leaders.
    run(&mut s, "layer.current", json!({ "name": "DIMS" }));
    run(&mut s, "dimlinear", json!({ "p1": [0, 0], "p2": [40, 0], "at": [20, -5] }));
    run(&mut s, "dimaligned", json!({ "p1": [60, 20], "p2": [75, 30], "at": [64, 30] }));
    run(&mut s, "dimangular", json!({ "vertex": [0, 100], "p1": [15, 100], "p2": [10, 110], "at": [12, 106] }));
    run(&mut s, "dimradius", json!({ "center": [60, 10], "point": [68, 10] }));
    run(&mut s, "dimdiameter", json!({ "center": [60, 10], "point": [60, 18] }));
    run(&mut s, "dimordinate", json!({ "feature": [40, 100], "leader": [45, 108] }));
    run(&mut s, "leader", json!({ "points": [[60, 100], [65, 106], [72, 106]], "text": "NOTE" }));
    run(&mut s, "mleader", json!({ "points": [[85, 100], [92, 107]], "text": "Callout" }));
    // Hatches: a pattern and a solid fill, and a 2D solid.
    run(&mut s, "layer.current", json!({ "name": "WALLS" }));
    run(&mut s, "rectang", json!({ "p1": [0, 120], "p2": [20, 135] }));
    run(&mut s, "hatch", json!({ "points": [[10, 127]], "pattern": "ANSI31", "scale": 2, "angle": 0 }));
    run(&mut s, "circle", json!({ "center": [40, 127], "radius": 6 }));
    run(&mut s, "hatch", json!({ "points": [[40, 127]], "pattern": "SOLID" }));
    let mut d = s.doc().map_or_else(|_| Drawing::new_metric(), |d| d.clone());
    // A 2D solid (no command makes one).
    let corners = [Vec3::new(60.0, 120.0, 0.0), Vec3::new(70.0, 120.0, 0.0), Vec3::new(60.0, 130.0, 0.0), Vec3::new(70.0, 130.0, 0.0)];
    let walls = Common { layer: "WALLS".into(), ..Common::default() };
    if let Err(e) = d.add(&Space::Model, walls, EntityKind::Solid(Solid { corners })) {
        failed.push(format!("solid: {e}"));
    }
    (d, failed)
}

/// Entity type names and their counts in model space.
fn census(d: &Drawing) -> std::collections::BTreeMap<String, usize> {
    let mut m = std::collections::BTreeMap::new();
    for e in d.model.iter() {
        *m.entry(e.kind.type_name().to_string()).or_insert(0) += 1;
    }
    m
}

#[test]
fn the_set_is_complete_and_reads_back() {
    let (d, failed) = test_set();
    assert!(failed.is_empty(), "{failed:#?}");
    let before = census(&d);
    for kind in [
        "Line",
        "Circle",
        "Arc",
        "Ellipse",
        "Polyline",
        "Spline",
        "Text",
        "MText",
        "Block Reference",
        "Dimension",
        "Hatch",
        "Leader",
        "Multileader",
        "2D Solid",
        "Ray",
        "Xline",
    ] {
        assert!(before.keys().any(|k| k.eq_ignore_ascii_case(kind)), "{kind} missing: {before:?}");
    }
    let after =
        cadaddy_io::read(&cadaddy_io::write_with(&d, "set.dxf", &SaveOptions { version: Some(Version::R2018), binary: false }).unwrap(), "set.dxf")
            .unwrap();
    assert_eq!(census(&after), before);
}

/// Write the set for the other programs (`CADADDY_VS_OUT`).
#[test]
#[ignore]
fn write_the_set() {
    let Some(dir) = std::env::var_os("CADADDY_VS_OUT").map(std::path::PathBuf::from) else { return };
    std::fs::create_dir_all(&dir).unwrap();
    let (d, failed) = test_set();
    assert!(failed.is_empty(), "{failed:#?}");
    // 2018 as is; R12 through SAVEAS, which brings the drawing down to what R12 holds first.
    let new = dir.join("cadaddy-2018.dxf");
    std::fs::write(&new, cadaddy_io::write_with(&d, "set.dxf", &SaveOptions { version: Some(Version::R2018), binary: false }).unwrap()).unwrap();
    let mut s = Session::new();
    s.execute("open", &json!({ "path": new.to_string_lossy() })).unwrap();
    let r = s.execute("saveas", &json!({ "path": dir.join("cadaddy-r12.dxf").to_string_lossy(), "version": 12 })).unwrap();
    std::fs::write(dir.join("r12-saveas.json"), serde_json::to_string_pretty(&r).unwrap()).unwrap();
    std::fs::write(dir.join("census.json"), serde_json::to_string_pretty(&census(&d)).unwrap()).unwrap();
}

/// Field paths where two values differ (numbers to 1e-4 relative; array indices out of the path).
fn differ(a: &Value, b: &Value, path: &str, out: &mut std::collections::BTreeMap<String, String>) {
    let short = |v: &Value| v.to_string().chars().take(70).collect::<String>();
    match (a, b) {
        (Value::Number(x), Value::Number(y)) => {
            let (x, y) = (x.as_f64().unwrap_or(0.0), y.as_f64().unwrap_or(0.0));
            if (x - y).abs() > 1e-4 * (1.0 + x.abs().max(y.abs())) {
                out.entry(path.to_string()).or_insert_with(|| format!("{} → {}", short(a), short(b)));
            }
        }
        (Value::Object(x), Value::Object(y)) => {
            let keys: std::collections::BTreeSet<&String> = x.keys().chain(y.keys()).collect();
            for k in keys {
                let p = if path.is_empty() { k.clone() } else { format!("{path}.{k}") };
                differ(x.get(k).unwrap_or(&Value::Null), y.get(k).unwrap_or(&Value::Null), &p, out);
            }
        }
        (Value::Array(x), Value::Array(y)) => {
            if x.len() != y.len() {
                out.entry(format!("{path}[len]")).or_insert_with(|| format!("{} → {}", x.len(), y.len()));
            }
            for (i, j) in x.iter().zip(y) {
                differ(i, j, &format!("{path}[]"), out);
            }
        }
        _ if a != b => {
            out.entry(path.to_string()).or_insert_with(|| format!("{} → {}", short(a), short(b)));
        }
        _ => {}
    }
}

/// Model space entities by type, each as its layer, colour, linetype and geometry.
fn by_type(d: &Drawing) -> std::collections::BTreeMap<String, Vec<Value>> {
    let mut m: std::collections::BTreeMap<String, Vec<Value>> = std::collections::BTreeMap::new();
    for e in d.model.iter() {
        let v = json!({
            "layer": e.common.layer.to_ascii_uppercase(),
            "color": serde_json::to_value(e.common.color).unwrap_or(Value::Null),
            "linetype": e.common.linetype.to_ascii_uppercase(),
            "kind": serde_json::to_value(&e.kind).unwrap_or(Value::Null),
        });
        m.entry(e.kind.type_name().to_string()).or_default().push(v);
    }
    m
}

/// Compare the other programs' saved files (`CADADDY_VS_OUT/<program>/*.dxf`) with the set: per
/// file, the types gained or lost and the fields that changed. Writes `compare.md`.
#[test]
#[ignore]
fn compare_their_files() {
    let Some(dir) = std::env::var_os("CADADDY_VS_OUT").map(std::path::PathBuf::from) else { return };
    let (set, _) = test_set();
    let ours = by_type(&set);
    let mut report = String::from("# Their DXF read by CADaddy, against the set\n");
    let mut files: Vec<std::path::PathBuf> = std::fs::read_dir(&dir)
        .unwrap()
        .flatten()
        .filter(|e| e.path().is_dir())
        .flat_map(|e| std::fs::read_dir(e.path()).unwrap().flatten().map(|f| f.path()))
        .filter(|p| p.extension().is_some_and(|x| x.eq_ignore_ascii_case("dxf")))
        .collect();
    files.sort();
    for f in files {
        let name = f.strip_prefix(&dir).unwrap().to_string_lossy().into_owned();
        report.push_str(&format!("\n## {name}\n\n"));
        let d = match cadaddy_io::read(&std::fs::read(&f).unwrap(), &name) {
            Ok(d) => d,
            Err(e) => {
                report.push_str(&format!("READ FAILED: {e}\n"));
                continue;
            }
        };
        let theirs = by_type(&d);
        let types: std::collections::BTreeSet<&String> = ours.keys().chain(theirs.keys()).collect();
        for t in types {
            let (a, b) = (ours.get(t).map_or(&[][..], Vec::as_slice), theirs.get(t).map_or(&[][..], Vec::as_slice));
            if a.len() != b.len() {
                report.push_str(&format!("- {t}: count {} → {}\n", a.len(), b.len()));
            }
            let mut diffs = std::collections::BTreeMap::new();
            for (x, y) in a.iter().zip(b) {
                differ(x, y, "", &mut diffs);
            }
            for (path, ex) in diffs {
                report.push_str(&format!("- {t}: {path}: {ex}\n"));
            }
        }
        let layers = |d: &Drawing| -> Vec<String> {
            let mut v: Vec<String> = d
                .layers
                .iter()
                .map(|l| format!("{} {:?} {} {:?}", l.name.to_ascii_uppercase(), l.color, l.linetype.to_ascii_uppercase(), l.lineweight))
                .collect();
            v.sort();
            v
        };
        let (la, lb) = (layers(&set), layers(&d));
        for l in la.iter().filter(|l| !lb.contains(l)) {
            report.push_str(&format!("- layer changed or lost: {l}\n"));
        }
    }
    std::fs::write(dir.join("compare.md"), &report).unwrap();
    println!("{report}");
}
