//! A model's takeoff is the same after saving it as DXF, DWG or .cdy and opening it again, and its
//! conduit fittings agree with `bim.count` (Q6376): a made-up two-storey model with one conduit
//! run per level, the first one turning a corner.

use std::collections::BTreeMap;

use cadaddy_engine::Session;
use serde_json::{Value, json};

fn hooks() {
    cadaddy_engine::cmd::file::set_io(cadaddy_engine::cmd::file::IoHooks {
        read: |b, n| cadaddy_io::read(b, n).map_err(|e| e.to_string()),
        write: |d, n| cadaddy_io::write(d, n).map_err(|e| e.to_string()),
        plot: None,
        read_pdf: None,
        read_dwf: None,
        write_dwfx: None,
    });
    cadaddy_engine::cmd::file::set_write_as(|d, n, o| {
        cadaddy_io::write_with(d, n, &cadaddy_io::SaveOptions { version: o.version, binary: o.binary }).map_err(|e| e.to_string())
    });
    cadaddy_engine::cmd::file::set_write_streamed(|d, n, o, open| {
        cadaddy_io::write_streamed(d, n, &cadaddy_io::SaveOptions { version: o.version, binary: o.binary }, open).map_err(|e| e.to_string())
    });
}

fn layer(s: &mut Session, name: &str) {
    if s.execute("layer.new", &json!({"name": name, "current": true})).is_err() {
        s.execute("layer.current", &json!({"name": name})).unwrap();
    }
}

fn rect(s: &mut Session, a: [f64; 2], b: [f64; 2]) {
    s.execute("pline", &json!({"vertices": [a, [b[0], a[1]], b, [a[0], b[1]]], "closed": true})).unwrap();
}

/// One storey drawn at `o`: a room's walls and a 3/4" conduit run (round a corner when `turn`).
fn storey(s: &mut Session, o: [f64; 2], turn: bool) {
    let p = |x: f64, y: f64| [o[0] + x, o[1] + y];
    layer(s, "A-WALL");
    rect(s, p(-12.0, -12.0), p(492.0, 372.0));
    rect(s, p(-6.0, -6.0), p(486.0, 366.0));
    layer(s, "E-COND");
    let run = if turn { vec![p(60.0, 60.0), p(300.0, 60.0), p(300.0, 240.0)] } else { vec![p(60.0, 200.0), p(360.0, 200.0)] };
    s.execute("pline", &json!({"vertices": run})).unwrap();
    s.execute("text", &json!({"at": p(120.0, 66.0 + if turn { 0.0 } else { 140.0 }), "text": "3/4\" C BOT @ 10'-0\"", "height": 4})).unwrap();
}

/// The two-storey model, read and built.
fn built() -> Session {
    let mut s = Session::new();
    storey(&mut s, [0.0, 0.0], true);
    storey(&mut s, [2100.0, 0.0], false);
    s.execute(
        "bspec.set",
        &json!({"replace": true, "spec": {
            "levels": [
                {"name": "Level 1", "elevation_ft": 0, "height_ft": 12, "ceiling_ft": 10.5, "region": [-200, -200, 1000, 1000], "base": [0, 0]},
                {"name": "Level 2", "elevation_ft": 12, "height_ft": 12, "ceiling_ft": 10.5, "region": [1800, -200, 3000, 1000], "base": [2100, 0]}
            ],
            "layers": [{"layer": "A-WALL", "role": "wall"}]
        }}),
    )
    .unwrap();
    s.execute("mep.read", &json!({"apply": true})).unwrap();
    s.execute("model.build", &json!({})).unwrap();
    s
}

/// The takeoff as `level | item | size | unit` → quantity.
fn takeoff(s: &mut Session) -> BTreeMap<String, f64> {
    let t = s.execute("bim.takeoff", &json!({"group_by": "level", "keep": false})).unwrap();
    t["lines"]
        .as_array()
        .unwrap()
        .iter()
        .map(|l| {
            let k = format!(
                "{} | {} | {} | {}",
                l["level"].as_str().unwrap_or(""),
                l["item"].as_str().unwrap_or(""),
                l["size"].as_str().unwrap_or(""),
                l["unit"].as_str().unwrap_or("")
            );
            (k, l["qty"].as_f64().unwrap_or(0.0))
        })
        .fold(BTreeMap::new(), |mut m, (k, q)| {
            *m.entry(k).or_insert(0.0) += q;
            m
        })
}

/// Where a drawing keeps the model's count index.
const INDEX: &str = "atoms/index.json";

fn count(s: &mut Session, kind: &str) -> f64 {
    s.execute("bim.count", &json!({"kind": kind})).unwrap()["count"].as_f64().unwrap_or(0.0)
}

/// The takeoff's conduit fittings of one kind (its items name them: `Locknut`, `elbow`, `bend`).
fn taken(t: &BTreeMap<String, f64>, word: &str) -> f64 {
    t.iter().filter(|(k, _)| k.split(" | ").nth(1).is_some_and(|i| i.to_ascii_lowercase().contains(word))).map(|(_, q)| q).sum()
}

#[test]
fn takeoff_is_the_same_after_dxf_dwg_and_cdy_round_trips() {
    hooks();
    let dir = std::env::temp_dir().join(format!("cadaddy-takeoffroundtrip-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let mut s = built();
    let before = takeoff(&mut s);
    let counts: Vec<(&str, f64)> = ["locknut", "elbow", "bend", "connector", "coupling", "strap"].iter().map(|k| (*k, count(&mut s, k))).collect();
    eprintln!("before: {before:#?}\ncounts: {counts:?}");
    assert!(counts.iter().any(|(k, n)| *k == "locknut" && *n > 0.0), "the runs end in locknuts: {counts:?}");
    for (name, version) in [("m.dxf", None), ("m.dwg", None), ("m2004.dwg", Some(2004)), ("m2000.dxf", Some(2000)), ("m.cdy", None)] {
        let path = dir.join(name).display().to_string();
        let mut p = json!({"path": path});
        if let Some(v) = version {
            p["version"] = json!(v);
        }
        s.execute("saveas", &p).unwrap();
        let mut t = Session::new();
        t.execute("open", &json!({"path": path})).unwrap();
        let after = takeoff(&mut t);
        let again: Vec<(&str, f64)> = counts.iter().map(|(k, _)| (*k, count(&mut t, k))).collect();
        assert_eq!(again, counts, "{name}: bim.count after reopening");
        assert_eq!(after, before, "{name}: the takeoff after reopening");
        // A file that came without the model's count index (one a format or another program
        // couldn't keep it in): the same elements give the same counts and the same takeoff.
        t.doc_mut().unwrap().embedded.remove(INDEX);
        let again: Vec<(&str, f64)> = counts.iter().map(|(k, _)| (*k, count(&mut t, k))).collect();
        assert_eq!(again, counts, "{name} without its index: bim.count");
        assert_eq!(takeoff(&mut t), before, "{name} without its index: the takeoff");
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn takeoff_fittings_agree_with_the_count() {
    let mut s = built();
    let t = takeoff(&mut s);
    // A locknut counted is a locknut taken off; an elbow or a field bend counted is one taken off.
    assert_eq!(taken(&t, "locknut"), count(&mut s, "locknut"), "{t:#?}");
    assert_eq!(taken(&t, "elbow") + taken(&t, "bend"), count(&mut s, "elbow") + count(&mut s, "bend"), "{t:#?}");
}

#[test]
fn field_bends_are_field_work_never_bought_or_priced() {
    let mut s = built();
    let r = s.execute("bim.takeoff", &json!({"group_by": "level", "keep": false})).unwrap();
    let lines = r["lines"].as_array().unwrap();
    let bends: Vec<&Value> = lines.iter().filter(|l| l["item"].as_str().is_some_and(|i| i.contains("field bend"))).collect();
    assert_eq!(bends.len(), 1, "{lines:#?}");
    let b = bends[0];
    assert_eq!((b["status"].as_str(), b["level"].as_str(), b["qty"].as_f64()), (Some("field work"), Some("Level 1"), Some(1.0)), "{b}");
    assert_eq!(b["element_count"], json!(1), "the bend's own piece: {b}");
    assert_eq!(r["totals"]["field_work"], json!({"lines": 1, "count": 1.0}), "{}", r["totals"]);
    // Out of the bought totals: the totals are the other lines'.
    let bought: f64 = lines.iter().filter(|l| l["status"] != "field work" && l["status"] != "by others").filter_map(|l| l["count"].as_f64()).sum();
    assert!((r["totals"]["count"].as_f64().unwrap() - bought).abs() < 1e-6, "{}", r["totals"]);
    assert!(r["csv"].as_str().unwrap().contains("electrical (field work)"), "{}", r["csv"]);
    // The estimate prices what is bought: no field bend on it.
    let e = s.execute("bim.estimate", &json!({"rows": [{"item": "EMT 90 elbow", "unit": "each", "material": 2.0, "trade": "electrical"}]})).unwrap();
    let all: Vec<&Value> = ["lines", "unmatched"].iter().flat_map(|k| e[*k].as_array().into_iter().flatten()).collect();
    assert!(!all.iter().any(|l| l["item"].as_str().is_some_and(|i| i.contains("field bend"))), "{e:#}");
}
