//! Converter and exchange notes (Q206): DWGCONVERT on a made-up drawing with every entity and table type.
//!
//! - DWG → .cdy → DWG → .cdy is stable after the first round, in every DWG version and DXF.
//! - The exchange notes are true: each object, table record and part the notes list as not kept
//!   really differs in the file written, and each that really differs is listed. The differences
//!   are found here on their own (object by object by handle, record by record by name), not with
//!   the converter's code.
//! - Folders: nested, read-only files, names with spaces and non-Latin letters, a damaged file in
//!   the middle (the rest still convert), outputs that can't be written.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use cadaddy_engine::Session;
use cadaddy_engine::doc::Drawing;
use serde_json::{Value, json};

#[path = "support/kitchen.rs"]
mod kitchen;
use kitchen::kitchen_sink;

fn hooks() {
    cadaddy_engine::cmd::file::set_io(cadaddy_engine::cmd::file::IoHooks {
        read: |b, n| cadaddy_io::read(b, n).map_err(|e| e.to_string()),
        write: |d, n| cadaddy_io::write(d, n).map_err(|e| e.to_string()),
        plot: Some(|d, space, opts| cadaddy_io::plotter::plot(d, space, opts).map_err(|e| e.to_string())),
        read_pdf: Some(cadaddy_io::pdf_read::read_json),
    });
    cadaddy_engine::cmd::file::set_write_as(|d, n, o| {
        cadaddy_io::write_with(d, n, &cadaddy_io::SaveOptions { version: o.version, binary: o.binary }).map_err(|e| e.to_string())
    });
}

fn folder(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("cadaddy-convertertest-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn read(path: &Path) -> Drawing {
    cadaddy_io::read(&std::fs::read(path).unwrap(), &path.to_string_lossy()).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

fn convert(s: &mut Session, from: &Path, to: &str, version: Option<&str>, out: &Path) -> (PathBuf, Value) {
    let mut p = json!({ "files": [from], "to": to, "out": out, "overwrite": "overwrite", "bak": false });
    if let Some(v) = version {
        p["version"] = json!(v);
    }
    let r = s.execute("dwgconvert", &p).unwrap_or_else(|e| panic!("{p}: {e}"));
    let c = r["converted"].as_array().unwrap_or_else(|| panic!("{r}"));
    assert_eq!(c.len(), 1, "{from:?} → {to} {version:?}: {r}");
    (PathBuf::from(c[0]["to"].as_str().unwrap()), c[0].clone())
}

/// Whether `b` holds every value `a` sets (numbers to 1e-6 relative): unset values (null, false,
/// zero, empty) are things a file may fill in, not losses.
fn holds(a: &Value, b: &Value) -> bool {
    match (a, b) {
        // As the notes compare (`kept::kept`): a false that came back true is a change.
        (Value::Bool(false), Value::Bool(true)) => false,
        (Value::Null, _) | (Value::Bool(false), _) => true,
        (Value::String(x), _) if x.is_empty() => true,
        (Value::Array(x), _) if x.is_empty() => true,
        (Value::Object(x), _) if x.is_empty() => true,
        (Value::Number(x), _) if x.as_f64() == Some(0.0) => true,
        (Value::Number(x), Value::Number(y)) => {
            let (x, y) = (x.as_f64().unwrap(), y.as_f64().unwrap());
            (x - y).abs() <= 1e-6 * x.abs().max(y.abs()).max(1.0)
        }
        (Value::Array(x), Value::Array(y)) => x.len() == y.len() && x.iter().zip(y).all(|(a, b)| holds(a, b)),
        (Value::Object(x), Value::Object(y)) => x.iter().all(|(k, a)| holds(a, y.get(k).unwrap_or(&Value::Null))),
        _ => a == b,
    }
}

/// An object as compared: its kind and properties, without what a writer renumbers (handle,
/// owner, a viewport's number).
fn object(e: &cadaddy_engine::doc::Entity) -> Value {
    let mut kind = serde_json::to_value(&e.kind).unwrap();
    if kind["type"] == "viewport" {
        kind.as_object_mut().unwrap().remove("id");
    }
    let mut common = serde_json::to_value(&e.common).unwrap();
    for k in ["handle", "owner"] {
        common.as_object_mut().unwrap().remove(k);
    }
    json!({ "kind": kind, "common": common })
}

/// What really differs between `a` and `b` (`a` written and read back): objects by handle,
/// table records by "table/NAME", and whole parts by name.
#[derive(Debug, Default, PartialEq)]
struct Diff {
    objects: BTreeSet<String>,
    records: BTreeSet<String>,
    parts: BTreeSet<String>,
}

fn diff(a: &Drawing, b: &Drawing) -> Diff {
    let mut out = Diff::default();
    let mut stores = vec![(&a.model, Some(&b.model))];
    for l in &a.layouts {
        let m = b.layouts.iter().find(|x| x.name.eq_ignore_ascii_case(&l.name));
        if m.is_none() {
            out.records.insert(format!("layouts/{}", l.name.to_uppercase()));
        }
        stores.push((&l.entities, m.map(|m| &m.entities)));
    }
    for (n, blk) in a.blocks.iter().filter(|(_, b)| !b.anonymous) {
        let m = b.blocks.get(n);
        if m.is_none() {
            out.records.insert(format!("blocks/{}", n.to_uppercase()));
        }
        stores.push((&blk.entities, m.map(|m| &m.entities)));
    }
    for (x, y) in stores {
        for e in x.iter() {
            let f = y.and_then(|y| y.iter().find(|f| f.handle == e.handle));
            let same = f.is_some_and(|f| {
                let (p, q) = (object(e), object(f));
                p["kind"]["type"] == q["kind"]["type"] && holds(&p, &q)
            });
            if !same {
                out.objects.insert(e.handle.hex());
            }
        }
    }
    let v = |t: &dyn erased::Ser| t.value();
    let tables: [(&str, Value, Value); 7] = [
        ("layers", v(&a.layers), v(&b.layers)),
        ("linetypes", v(&a.linetypes), v(&b.linetypes)),
        ("text styles", v(&a.text_styles), v(&b.text_styles)),
        ("dimension styles", v(&a.dim_styles), v(&b.dim_styles)),
        ("multileader styles", v(&a.mleader_styles), v(&b.mleader_styles)),
        ("table styles", v(&a.table_styles), v(&b.table_styles)),
        ("multiline styles", v(&a.mline_styles), v(&b.mline_styles)),
    ];
    for (t, x, y) in tables {
        for r in x.as_array().unwrap() {
            let name = r["name"].as_str().unwrap_or_default().to_uppercase();
            let back = y.as_array().unwrap().iter().find(|s| s["name"].as_str().unwrap_or_default().to_uppercase() == name);
            let mut r = r.clone();
            r.as_object_mut().unwrap().remove("name");
            if !back.is_some_and(|b| holds(&r, b)) {
                out.records.insert(format!("{t}/{name}"));
            }
        }
    }
    let parts: [(&str, Value, Value); 10] = [
        ("named views", v(&a.views), v(&b.views)),
        ("viewport configurations", v(&a.vports), v(&b.vports)),
        ("UCSs", v(&a.ucss), v(&b.ucss)),
        ("layer states", v(&a.layer_states), v(&b.layer_states)),
        ("groups", v(&a.groups), v(&b.groups)),
        ("plot style tables", v(&a.plot_style_tables), v(&b.plot_style_tables)),
        ("geometric and dimensional constraints", v(&a.constraints), v(&b.constraints)),
        ("parameters", v(&a.parametric), v(&b.parametric)),
        ("the scale list", v(&a.scales), v(&b.scales)),
        ("materials", v(&a.materials), v(&b.materials)),
    ];
    for (p, x, y) in parts {
        if !holds(&x, &y) {
            out.parts.insert(p.to_string());
        }
    }
    out
}

mod erased {
    pub trait Ser {
        fn value(&self) -> serde_json::Value;
    }
    impl<T: serde::Serialize> Ser for T {
        fn value(&self) -> serde_json::Value {
            serde_json::to_value(self).unwrap()
        }
    }
}

/// What the converter's notes (`losses` of one converted file) say differs, in [`Diff`]'s terms.
fn noted(c: &Value) -> Diff {
    const TABLES: [&str; 9] =
        ["layers", "linetypes", "text styles", "dimension styles", "multileader styles", "table styles", "multiline styles", "layouts", "blocks"];
    const PARTS: [&str; 10] = [
        "named views",
        "viewport configurations",
        "UCSs",
        "layer states",
        "groups",
        "plot style tables",
        "geometric and dimensional constraints",
        "parameters",
        "the scale list",
        "materials",
    ];
    let mut out = Diff::default();
    for l in c["losses"].as_array().into_iter().flatten() {
        let feature = l["feature"].as_str().unwrap();
        let table = feature.split(':').next().unwrap_or_default();
        let handles = l["handles"].as_array().into_iter().flatten();
        let names = l["names"].as_array().into_iter().flatten();
        if PARTS.contains(&feature) {
            out.parts.insert(feature.to_string());
        } else if l["handles"].as_array().is_some_and(|h| !h.is_empty()) {
            out.objects.extend(handles.map(|h| h.as_str().unwrap().to_string()));
        } else if TABLES.contains(&table) {
            out.records.extend(names.map(|n| format!("{table}/{}", n.as_str().unwrap().to_uppercase())));
        } else {
            panic!("a note this test doesn't place: {l}");
        }
    }
    out
}

/// What Release 12 DXF holds as itself (the R12 DXF reference's entities CADaddy models):
/// everything else is written as these or left out.
const R12_KINDS: [&str; 13] =
    ["line", "point", "circle", "arc", "text", "attdef", "insert", "solid", "trace", "face3d", "lwpolyline", "polyline3d", "viewport"];

/// The objects whose notes are right for a Release 12 file, which has no handles to match objects
/// by: every object of a type R12 doesn't have, and of each type it has, as many as the file
/// holds fewer of (per space and block). A note on any other object must be about one of its
/// fields, not the object.
fn r12_changed(a: &Drawing, b: &Drawing, c: &Value) -> BTreeSet<String> {
    let notes: Vec<&Value> = c["losses"].as_array().into_iter().flatten().collect();
    let noted_as = |h: &str| -> Vec<&str> {
        notes.iter().filter(|l| l["handles"].as_array().is_some_and(|x| x.iter().any(|x| x == h))).map(|l| l["feature"].as_str().unwrap()).collect()
    };
    let mut want = BTreeSet::new();
    let mut stores = vec![(&a.model, Some(&b.model))];
    stores.extend(a.layouts.iter().map(|l| (&l.entities, b.layouts.iter().find(|x| x.name.eq_ignore_ascii_case(&l.name)).map(|m| &m.entities))));
    stores.extend(a.blocks.iter().filter(|(_, x)| !x.anonymous).map(|(n, x)| (&x.entities, b.blocks.get(n).map(|m| &m.entities))));
    // A layout or block the file doesn't have is noted as one ("layouts dropped"), not object by object.
    for (x, y) in stores.into_iter().filter_map(|(x, y)| Some((x, Some(y?)))) {
        let kind = |e: &cadaddy_engine::doc::Entity| serde_json::to_value(&e.kind).unwrap()["type"].as_str().unwrap().to_ascii_lowercase();
        let mut have = std::collections::BTreeMap::<String, usize>::new();
        for e in y.into_iter().flat_map(|y| y.iter()) {
            *have.entry(kind(e)).or_default() += 1;
        }
        for e in x.iter() {
            let (k, h) = (kind(e), e.handle.hex());
            let native = R12_KINDS.contains(&k.as_str());
            let left = have.get_mut(&k).filter(|n| native && **n > 0);
            match left {
                Some(n) => {
                    *n -= 1;
                    // Kept as itself: only field notes.
                    for f in noted_as(&h) {
                        assert!(!f.contains("objects"), "{h} ({k}) is in the file as itself but noted as {f}");
                        want.insert(h.clone());
                    }
                }
                None => {
                    if std::env::var_os("CONVERTERTEST_SHOW").is_some() {
                        eprintln!("R12: {h} {k} expected noted");
                    }
                    want.insert(h);
                }
            }
        }
    }
    want
}

const VERSIONS: [(&str, &str); 8] =
    [("dwg", "2000"), ("dwg", "2004"), ("dwg", "2007"), ("dwg", "2010"), ("dwg", "2013"), ("dwg", "2018"), ("dxf", "2018"), ("dxf", "12")];

/// Every note matches a real difference and every real difference has a note: on the first
/// conversion out of .cdy (where the older versions lose things) and on the second (where
/// nothing more is lost, so there must be no notes at all).
#[test]
fn exchange_notes_match_real_differences() {
    hooks();
    let dir = folder("notes");
    let d0 = kitchen_sink(&dir);
    let cdy0 = dir.join("Sample Floor Plan.cdy");
    std::fs::write(&cdy0, cadaddy_io::write(&d0, "x.cdy").unwrap()).unwrap();
    let mut s = Session::new();
    let mut problems = Vec::new();
    for (to, ver) in VERSIONS {
        let out = dir.join(format!("{to}{ver}"));
        let (x1, c1) = convert(&mut s, &cdy0, to, Some(ver), &out.join("1"));
        let (mut real, said) = (diff(&read(&cdy0), &read(&x1)), noted(&c1));
        if ver == "12" {
            real.objects = r12_changed(&read(&cdy0), &read(&x1), &c1);
        }
        if std::env::var_os("CONVERTERTEST_SHOW").is_some() {
            eprintln!("{to} {ver}: real {real:?}\n  notes {}", c1["losses"]);
        }
        // DWG 2000 has no multileaders or meshes: the check is not empty.
        if ver == "2000" {
            assert!(!real.objects.is_empty() && !real.records.is_empty(), "{real:?}");
        }
        // Notes say what an object became, in words.
        let features: Vec<String> = c1["losses"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|l| format!("{} {}", l["feature"].as_str().unwrap(), l["what"].as_str().unwrap()))
            .collect();
        if ["2000", "2004"].contains(&ver) {
            assert!(features.contains(&"mleader objects (written as leader, mtext) changed".to_string()), "{ver}: {features:?}");
            assert!(!features.iter().any(|f| f.starts_with("mleader objects dropped")), "{ver}: {features:?}");
        }
        if ["2000", "2004", "2007"].contains(&ver) {
            assert!(features.contains(&"mesh objects (written as a polyface mesh) changed".to_string()), "{ver}: {features:?}");
            assert!(!features.iter().any(|f| f.starts_with("mesh kind")), "{ver}: {features:?}");
        }
        if ver == "12" {
            // Paper space is in R12: Layout1's line comes back; only its viewport is noted.
            let back = read(&x1);
            let paper = back
                .layouts
                .iter()
                .find(|l| l.name.eq_ignore_ascii_case("Layout1"))
                .map(|l| l.entities.iter().filter(|e| e.kind.type_name() == "Line").count());
            assert_eq!(paper, Some(1), "the paper-space line in R12");
            assert!(features.iter().any(|f| f == "viewport objects dropped"), "{features:?}");
        }
        if real != said {
            problems.push(format!("{to} {ver}, first: real {real:?}\n   notes {said:?}\n   {}", c1["losses"]));
        }
        let (cdy1, c) = convert(&mut s, &x1, "cdy", None, &out.join("2"));
        assert!(c.get("losses").is_none(), "a .cdy keeps everything: {c}");
        let (x2, c2) = convert(&mut s, &cdy1, to, Some(ver), &out.join("3"));
        let (real, said) = (diff(&read(&cdy1), &read(&x2)), noted(&c2));
        if real != said || real != Diff::default() {
            problems.push(format!("{to} {ver}, second: real {real:?}\n   notes {said:?}\n   {}", c2["losses"]));
        }
    }
    if std::env::var_os("CONVERTERTEST_SHOW").is_none() {
        let _ = std::fs::remove_dir_all(&dir);
    }
    assert!(problems.is_empty(), "{}", problems.join("\n"));
}

/// Every field of two drawings (both ways, exactly but for float noise), without what a writer
/// renumbers: what differs, as paths.
fn exact(a: &Value, b: &Value, path: &str, out: &mut BTreeSet<String>) {
    match (a, b) {
        (Value::Number(x), Value::Number(y)) => {
            let (x, y) = (x.as_f64().unwrap(), y.as_f64().unwrap());
            if (x - y).abs() > 1e-9 * x.abs().max(y.abs()).max(1.0) {
                out.insert(format!("{path}: {x} → {y}"));
            }
        }
        (Value::Object(x), Value::Object(y)) => {
            let keys: BTreeSet<&String> = x.keys().chain(y.keys()).collect();
            for k in keys {
                exact(x.get(k).unwrap_or(&Value::Null), y.get(k).unwrap_or(&Value::Null), &format!("{path}.{k}"), out);
            }
        }
        (Value::Array(x), Value::Array(y)) if x.len() == y.len() => {
            for (i, (p, q)) in x.iter().zip(y).enumerate() {
                exact(p, q, &format!("{path}[{i}]"), out);
            }
        }
        _ if a != b => {
            let short = |v: &Value| v.to_string().chars().take(60).collect::<String>();
            out.insert(format!("{path}: {} → {}", short(a), short(b)));
        }
        _ => {}
    }
}

/// A drawing's content as compared for stability: everything the .cdy holds, objects keyed by
/// handle, without per-save bookkeeping (header times and the like are left to the file).
fn content(d: &Drawing) -> Value {
    let objects = |s: &cadaddy_engine::doc::EntityStore| -> Value {
        s.iter().map(|e| (e.handle.hex(), object(e))).collect::<serde_json::Map<String, Value>>().into()
    };
    let layouts: serde_json::Map<String, Value> = d.layouts.iter().map(|l| (l.name.clone(), objects(&l.entities))).collect();
    let blocks: serde_json::Map<String, Value> =
        d.blocks.iter().filter(|(_, b)| !b.anonymous).map(|(n, b)| (n.clone(), objects(&b.entities))).collect();
    let v = |t: &dyn erased::Ser| t.value();
    json!({
        "model": objects(&d.model), "layouts": layouts, "blocks": blocks,
        "layers": v(&d.layers), "linetypes": v(&d.linetypes), "textStyles": v(&d.text_styles), "dimStyles": v(&d.dim_styles),
        "mleaderStyles": v(&d.mleader_styles), "tableStyles": v(&d.table_styles), "mlineStyles": v(&d.mline_styles),
        "views": v(&d.views), "vports": v(&d.vports), "ucss": v(&d.ucss), "layerStates": v(&d.layer_states), "groups": v(&d.groups),
        "plotStyleTables": v(&d.plot_style_tables), "constraints": v(&d.constraints), "parametric": v(&d.parametric),
        "scales": v(&d.scales), "materials": v(&d.materials),
    })
}

/// DWG → .cdy → DWG → .cdy: the two .cdy files hold the same drawing (after the first round
/// trip nothing more changes), in every DWG version and DXF.
#[test]
fn round_trips_are_stable_after_the_first() {
    hooks();
    let dir = folder("stable");
    let d0 = kitchen_sink(&dir);
    let mut s = Session::new();
    let mut problems = Vec::new();
    for (to, ver) in VERSIONS {
        let out = dir.join(format!("{to}{ver}"));
        std::fs::create_dir_all(&out).unwrap();
        // The drawing as another program would hand it over: a DWG (or DXF) file.
        let x0 = out.join(format!("Project A.{to}"));
        let opts = cadaddy_io::SaveOptions { version: cadaddy_io::Version::parse(ver), binary: false };
        std::fs::write(&x0, cadaddy_io::write_with(&d0, &x0.to_string_lossy(), &opts).unwrap()).unwrap();
        let (cdy1, _) = convert(&mut s, &x0, "cdy", None, &out.join("1"));
        let (x1, _) = convert(&mut s, &cdy1, to, Some(ver), &out.join("2"));
        let (cdy2, _) = convert(&mut s, &x1, "cdy", None, &out.join("3"));
        let mut paths = BTreeSet::new();
        exact(&content(&read(&cdy1)), &content(&read(&cdy2)), "", &mut paths);
        if !paths.is_empty() {
            problems.push(format!("{to} {ver}: {} differences\n   {}", paths.len(), paths.into_iter().take(15).collect::<Vec<_>>().join("\n   ")));
        }
        // Converting a file to its own format and version again changes nothing either.
        let (x2, c) = convert(&mut s, &x1, to, Some(ver), &out.join("4"));
        if c.get("losses").is_some() {
            problems.push(format!("{to} {ver}: {} → {}: notes {}", x1.display(), x2.display(), c["losses"]));
        }
    }
    if std::env::var_os("CONVERTERTEST_SHOW").is_none() {
        let _ = std::fs::remove_dir_all(&dir);
    }
    assert!(problems.is_empty(), "{}", problems.join("\n"));
}

/// SAVEAS to Release 12 DXF lists what R12 changed: an ellipse written as a polyline, an xline
/// left out. Not the line and circle R12 keeps as they are (R12 has no handles, so its objects
/// were matched to the wrong ones and reported as "written as lwpolyline").
#[test]
fn saving_release_12_notes_only_what_r12_changes() {
    hooks();
    let dir = folder("saveas12");
    let mut s = Session::new();
    s.execute("hatch", &json!({ "points": [[0, 0], [10, 0], [10, 10], [0, 10]], "pattern": "ANSI31" })).ok();
    for (id, p) in [
        ("line", json!({ "points": [[0, 0], [10, 5]] })),
        ("circle", json!({ "center": [20, 0], "radius": 4 })),
        ("ellipse", json!({ "center": [40, 0], "major": [5, 1], "ratio": 0.4 })),
        ("xline", json!({ "base": [70, 0], "angle": 30 })),
        ("text", json!({ "at": [0, 60], "text": "Project A", "height": 2.5 })),
    ] {
        s.execute(id, &p).unwrap();
    }
    let r = s.execute("saveas", &json!({ "path": dir.join("Sample Floor Plan.dxf"), "version": "12" })).unwrap();
    let features: Vec<&str> = r["notKeptDetail"].as_array().unwrap().iter().map(|l| l["feature"].as_str().unwrap()).collect();
    assert!(features.contains(&"ellipse objects (written as lwpolyline)"), "{r}");
    assert!(features.contains(&"xline objects"), "{r}");
    for kept in ["line", "circle", "text"] {
        assert!(!features.iter().any(|f| f.starts_with(kept)), "{kept} is kept as itself: {r}");
    }
    let _ = std::fs::remove_dir_all(&dir);
}
