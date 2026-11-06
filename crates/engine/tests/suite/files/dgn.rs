//! DGN commands (Q158): DGNEXPORT then DGNIMPORT bring the drawing back (a cell as a block
//! reference, layers as levels); DGNATTACH shows a design file as an underlay that draws, hides
//! levels (DGNLAYERS), clips (DGNCLIP), fades (DGNADJUST) and is kept in DXF as a DGNUNDERLAY; a
//! DGN V8 file is refused with a clear message.

use crate::helper::hooks;
use cadaddy_engine::Session;
use serde_json::{Value, json};

fn dir(tag: &str) -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("cadaddy-dgn-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// A site plan: a boundary on SITE, a building on BLDG, a tree block placed three times.
fn site(s: &mut Session) {
    s.execute("layer.new", &json!({"name": "SITE", "color": 3, "current": true})).unwrap();
    s.execute("rectang", &json!({"p1": [0, 0], "p2": [120, 80]})).unwrap();
    s.execute("layer.new", &json!({"name": "BLDG", "color": 1, "current": true})).unwrap();
    s.execute("rectang", &json!({"p1": [30, 20], "p2": [70, 50]})).unwrap();
    s.execute("circle", &json!({"center": [0, 0], "radius": 2})).unwrap();
    let h = s.doc().unwrap().model.iter().last().unwrap().handle.hex();
    s.execute("block", &json!({"name": "TREE", "base": [0, 0], "handles": [h], "keep": "delete"})).unwrap();
    for p in [[90, 20], [100, 60], [15, 65]] {
        s.execute("insert", &json!({"name": "TREE", "at": p})).unwrap();
    }
    s.execute("text", &json!({"at": [35, 35], "height": 2, "text": "OFFICE"})).unwrap();
}

fn count(s: &Session, kind: &str) -> usize {
    s.doc().unwrap().model.iter().filter(|e| e.kind.type_name() == kind).count()
}

#[test]
fn export_then_import_brings_the_drawing_back() {
    hooks();
    let d = dir("roundtrip");
    let path = d.join("site.dgn").display().to_string();
    let mut s = Session::new();
    site(&mut s);
    let r = s.execute("dgnexport", &json!({"path": path})).unwrap();
    assert_eq!(r["notes"], json!([]), "{r}");
    assert!(r["bytes"].as_u64().unwrap() > 1536);
    let mut t = Session::new();
    let r = t.execute("dgnimport", &json!({"path": path})).unwrap();
    assert!(r["added"].as_u64().unwrap() >= 6, "{r}");
    assert_eq!(count(&t, "Polyline"), 2);
    assert_eq!(count(&t, "Text"), 1);
    // The trees: cells → block references to one tree block.
    let inserts: Vec<String> = t
        .doc()
        .unwrap()
        .model
        .iter()
        .filter_map(|e| if let cadaddy_engine::doc::EntityKind::Insert(i) = &e.kind { Some(i.block.clone()) } else { None })
        .collect();
    assert_eq!(inserts.len(), 3, "{inserts:?}");
    assert!(inserts.iter().all(|b| t.doc().unwrap().block(b).is_some_and(|blk| blk.entities.len() == 1)));
    let distinct: std::collections::BTreeSet<&String> = inserts.iter().collect();
    assert_eq!(distinct.len(), 1, "three copies of one cell reference one block: {inserts:?}");
    // Levels became layers, one each for SITE and BLDG.
    let layers: std::collections::BTreeSet<String> = t.doc().unwrap().model.iter().map(|e| e.common.layer.to_string()).collect();
    assert!(layers.iter().all(|l| l.starts_with("Level ")) && layers.len() >= 2, "{layers:?}");
    // The import's temporary block is gone.
    assert!(t.doc().unwrap().blocks.keys().all(|k| !k.starts_with("DGN-")));
}

#[test]
fn a_design_file_as_an_underlay() {
    hooks();
    let d = dir("underlay");
    let path = d.join("site.dgn").display().to_string();
    let mut src = Session::new();
    site(&mut src);
    src.execute("dgnexport", &json!({"path": path})).unwrap();
    let mut s = Session::new();
    let r = s.execute("dgnattach", &json!({"path": path})).unwrap();
    let h = r["handle"].as_str().unwrap().to_string();
    assert_eq!(r["size"], json!([120.0, 80.0]), "{r}");
    let levels: Vec<String> = r["levels"].as_array().unwrap().iter().map(|l| l.as_str().unwrap().to_string()).collect();
    assert_eq!(levels.len(), 2, "SITE and BLDG (the trees were placed on BLDG): {levels:?}");
    // Placed at its own coordinates: the frame's corner is the design's.
    let u = match &s.doc().unwrap().entity(cadaddy_engine::doc::Handle::parse_hex(&h).unwrap()).unwrap().kind {
        cadaddy_engine::doc::EntityKind::Underlay(u) => u.clone(),
        _ => panic!("not an underlay"),
    };
    assert!(u.is_dgn() && u.insert.xy().len() < 1e-6, "{u:?}");
    // It draws its content, not just a frame; turning a level off draws less.
    let prims = |s: &Session| cadaddy_engine::render::build(s.doc().unwrap(), &cadaddy_engine::doc::Space::Model, &Default::default()).prims.len();
    let all = prims(&s);
    assert!(all > 4, "{all}");
    let r = s.execute("dgnlayers", &json!({"handle": h, "off": [levels[levels.len() - 1]]})).unwrap();
    assert!(r["layers"].as_array().unwrap().iter().any(|l| l["on"] == false));
    assert!(prims(&s) < all);
    s.execute("dgnclip", &json!({"handle": h, "rect": [[0, 0], [60, 40]]})).unwrap();
    // DGNOSNAP (on unless 0) is a drawing variable SETVAR takes.
    s.execute("setvar", &json!({"name": "DGNOSNAP", "value": 0})).unwrap();
    assert_eq!(s.execute("getvar", &json!({"name": "DGNOSNAP"})).unwrap()["value"], 0);
    s.execute("setvar", &json!({"name": "DGNOSNAP", "value": 1})).unwrap();
    let r = s.execute("dgnadjust", &json!({"handles": [h], "fade": 60})).unwrap();
    assert_eq!(r["fade"], 60);
    // Kept in DXF as a DGNUNDERLAY with its DGNDEFINITION, and back as a DGN underlay.
    let dxf = d.join("host.dxf");
    s.execute("saveas", &json!({"path": dxf.display().to_string()})).unwrap();
    let text = std::fs::read_to_string(&dxf).unwrap();
    assert!(text.contains("DGNUNDERLAY") && text.contains("DGNDEFINITION") && text.contains("ACAD_DGNDEFINITIONS"), "the DXF names it");
    let mut t = Session::empty();
    t.execute("open", &json!({"path": dxf.display().to_string()})).unwrap();
    let back = t
        .doc()
        .unwrap()
        .model
        .iter()
        .find_map(|e| if let cadaddy_engine::doc::EntityKind::Underlay(u) = &e.kind { Some(u.clone()) } else { None })
        .unwrap();
    assert!(back.is_dgn() && back.clipping && back.fade == 60 && back.layers_off.len() == 1, "{back:?}");
}

#[test]
fn dgn_v8_and_bad_input_are_refused() {
    hooks();
    let d = dir("v8");
    let v8 = d.join("new.dgn");
    let mut b = vec![0xD0, 0xCF, 0x11, 0xE0, 0xA1, 0xB1, 0x1A, 0xE1];
    b.extend(vec![0u8; 512]);
    std::fs::write(&v8, b).unwrap();
    let mut s = Session::new();
    for c in ["dgnimport", "dgnattach"] {
        let e = s.execute(c, &json!({"path": v8.display().to_string()})).unwrap_err().to_string();
        assert!(e.contains("DGN V8 isn't supported yet"), "{c}: {e}");
    }
    for (c, p) in [
        ("dgnimport", json!({})),
        ("dgnimport", json!({"path": "x.dwg"})),
        ("dgnexport", json!({"path": "x.dxf"})),
        ("dgnattach", json!({"path": "missing.dgn"})),
    ] {
        assert!(s.execute(c, &p).is_err(), "{c} {p}");
    }
    let _: Value = json!(null);
}

#[test]
fn attaching_a_damaged_design_file_passes_the_reader_warnings_on() {
    hooks();
    let d = dir("damaged");
    let path = d.join("site.dgn").display().to_string();
    let mut s = Session::new();
    site(&mut s);
    s.execute("dgnexport", &json!({"path": path})).unwrap();
    let whole = s.execute("dgnattach", &json!({"path": path})).unwrap();
    assert_eq!(whole["warnings"], json!([]), "{whole}");
    let bytes = std::fs::read(&path).unwrap();
    let mut warned = 0;
    for cut in (1600..bytes.len()).step_by(7) {
        let cut_path = d.join("cut.dgn").display().to_string();
        std::fs::write(&cut_path, &bytes[..cut]).unwrap();
        let mut t = Session::new();
        if let Ok(r) = t.execute("dgnattach", &json!({"path": cut_path})) {
            let size = r["size"].as_array().unwrap();
            assert!(size.iter().all(|v| v.as_f64().unwrap() <= 1e12), "{r}");
            warned += usize::from(!r["warnings"].as_array().unwrap().is_empty());
        }
    }
    assert!(warned > 0, "no cut file attached with a warning");
}

#[test]
fn dist_and_id_refuse_values_too_large_to_report() {
    let mut s = Session::new();
    let e = s.execute("dist", &json!({"p1": [-1e308, 0], "p2": [1e308, 5]})).unwrap_err().to_string();
    assert!(e.contains("the distance is too large to compute"), "{e}");
    let e = s.execute("id", &json!({"at": [1e308, 0]})).unwrap_err().to_string();
    assert!(e.contains("too large"), "{e}");
    let r = s.execute("dist", &json!({"p1": [0, 0], "p2": [3, 4]})).unwrap();
    assert!(r["message"].as_str().unwrap().starts_with("Distance = 5.0000"), "{r}");
}
