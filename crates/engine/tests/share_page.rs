//! The shared project page's model files (Q350 `share.export`): a made-up IFC project and a
//! drawing's building model written as a bundle — manifest, base, parts and thumbnails, all named
//! relative to the page — loaded back part by part, with every planted name, address, firm and
//! person absent from every byte of the bundle and listed in the strip report beside it.

use cadaddy_engine::Session;
use cadaddy_engine::sample_ifc::{PLANTED, share_project_ifc};
use cadaddy_walk3d::share::{MANIFEST, MODEL_DIR, ShareLoad, ShareManifest};
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
}

fn dir(name: &str) -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("cadaddy-sharepage-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn files(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
    for e in std::fs::read_dir(dir).unwrap().flatten() {
        let p = e.path();
        if p.is_dir() { files(&p, out) } else { out.push(p) }
    }
}

/// Every file of the bundle, none holding any of `planted` (case ignored).
fn assert_clean(page: &std::path::Path, planted: &[&str]) -> Vec<std::path::PathBuf> {
    let mut all = Vec::new();
    files(page, &mut all);
    for p in &all {
        let text = String::from_utf8_lossy(&std::fs::read(p).unwrap()).to_lowercase();
        for n in planted {
            assert!(!text.contains(&n.to_lowercase()), "{n:?} left in {}", p.display());
        }
    }
    all
}

fn load(page: &std::path::Path) -> (ShareManifest, cadaddy_walk3d::field::FieldPackage) {
    let m = ShareManifest::from_json(&std::fs::read(page.join(MODEL_DIR).join(MANIFEST)).unwrap()).unwrap();
    let pkg = ShareLoad::read_all(m.clone(), |f| std::fs::read(page.join(MODEL_DIR).join(f)).map_err(|e| e.to_string())).unwrap();
    (m, pkg)
}

#[test]
fn an_ifc_project_shares_without_its_names() {
    hooks();
    let d = dir("ifc");
    // The file's own name is one of the planted names too.
    let src = d.join("Lakeside Example Inn.ifc");
    std::fs::write(&src, share_project_ifc(2, 24)).unwrap();
    let mut s = Session::empty();
    s.execute("ifc.open", &json!({"path": src.display().to_string()})).unwrap();
    let page = d.join("page");
    let r = s
        .execute(
            "share.export",
            &json!({"dir": page.display().to_string(), "title": "Sample Wing", "source": src.display().to_string(), "allow_download": true, "chunk": 100}),
        )
        .unwrap();
    assert_eq!((r["levels"].clone(), r["rooms"].clone(), r["source"].clone()), (json!(2), json!(48), json!("ifc")), "{r}");
    assert!(r["parts"].as_u64().unwrap() >= 2, "{r}");
    assert_eq!(r["download"], "download/model.field.json");
    // Relative names only.
    for f in r["files"].as_array().unwrap() {
        let f = f.as_str().unwrap();
        assert!(!f.starts_with('/') && !f.contains(':') && !f.contains(".."), "{f}");
    }

    let mut planted: Vec<&str> = PLANTED.to_vec();
    planted.push("555-010-0142");
    let all = assert_clean(&page, &planted);
    assert!(all.iter().any(|p| p.ends_with("overview.svg")) && all.iter().any(|p| p.ends_with("level-2.svg")));
    // The report sits beside the page and lists what went.
    let report = std::fs::read_to_string(r["report"].as_str().unwrap()).unwrap();
    assert!(!std::path::Path::new(r["report"].as_str().unwrap()).starts_with(&page));
    for n in ["Quillonby", "Fernhollow Design Group", "Thistledown Lighting Works"] {
        assert!(report.contains(n), "{n} not in the report:\n{report}");
    }

    let (m, pkg) = load(&page);
    assert_eq!((m.title.as_str(), pkg.project.as_str(), m.stripped), ("Sample Wing", "Sample Wing", true));
    assert_eq!(pkg.elements.len(), m.elements);
    assert_eq!(pkg.levels.iter().map(|l| l.name.as_str()).collect::<Vec<_>>(), ["Level 1", "Level 2"]);
    for t in ["arch", "struct", "mech", "elec"] {
        assert!(pkg.trades.iter().any(|x| x.key == t), "{t}: {:?}", pkg.trade_counts());
    }
    // A light: its model kept, its manufacturer gone; in its room.
    let light = pkg.elements.iter().find(|e| e.kind.starts_with("LightFixture")).unwrap();
    assert!(light.props.iter().any(|p| p.key == "ModelLabel" && p.value == "DL-12"), "{:?}", light.props);
    assert!(!light.props.iter().any(|p| p.key == "Manufacturer"));
    assert_eq!(light.mark, "Downlight A");
    assert!(!light.room.is_empty());
    // A duct: its system, and its status from the file.
    let duct = pkg.elements.iter().find(|e| e.kind.starts_with("DuctSegment")).unwrap();
    assert_eq!(duct.status, "New");
    assert!(duct.props.iter().any(|p| p.key == "System" && p.value == "Supply Air"));
    assert_eq!(duct.trade, "mech");
    // Rooms keep their numbers and ordinary names; the manager's office loses the name.
    let r101 = pkg.rooms.iter().find(|r| r.number == "101").unwrap();
    assert_eq!(r101.name, "Manager office");
    assert!(pkg.rooms.iter().any(|r| r.number == "208" && r.name == "Guest Room"));
}

fn rect(s: &mut Session, a: [f64; 2], b: [f64; 2]) {
    s.execute("pline", &json!({"vertices": [a, [b[0], a[1]], b, [a[0], b[1]]], "closed": true})).unwrap();
}

fn layer(s: &mut Session, name: &str) {
    if s.execute("layer.new", &json!({"name": name, "current": true})).is_err() {
        s.execute("layer.current", &json!({"name": name})).unwrap();
    }
}

/// Two rooms off a corridor, a typed duct; the drawing's properties, a title block on the sheet
/// and the duct's spec name made-up people, a firm, an address and the project.
fn building() -> Session {
    let mut s = Session::new();
    layer(&mut s, "A-WALL");
    rect(&mut s, [0.0, 0.0], [312.0, 360.0]);
    rect(&mut s, [312.0, 0.0], [624.0, 360.0]);
    layer(&mut s, "A-ANNO");
    s.execute("text", &json!({"at": [120, 180], "text": "King", "height": 6})).unwrap();
    s.execute("text", &json!({"at": [430, 180], "text": "Marigold Quillonby Suite", "height": 6})).unwrap();
    layer(&mut s, "M-DUCT-SUPP");
    s.execute("pline", &json!({"vertices": [[24, 300], [580, 300]]})).unwrap();
    s.execute("text", &json!({"at": [200, 306], "text": "20x10 BOT @ 7'-6\"", "height": 4})).unwrap();
    s.execute(
        "bspec.set",
        &json!({"replace": true, "spec": {
            "levels": [{"name": "Level 2", "elevation_ft": 12, "height_ft": 11, "ceiling_ft": 9, "region": [-100, -100, 1000, 1000], "base": [0, 0]}],
            "layers": [{"layer": "A-WALL", "role": "wall"}],
            "rooms": [
                {"name": "King", "number": "214", "level": "Level 2", "ceiling_ft": 9},
                {"name": "Marigold Quillonby Suite", "number": "215", "level": "Level 2", "ceiling_ft": 9}
            ],
            "rcp": {"ceilings": [
                {"level": "Level 2", "room": "214", "outline": [[0, 0], [312, 0], [312, 360], [0, 360]], "height_ft": 9},
                {"level": "Level 2", "room": "215", "outline": [[312, 0], [624, 0], [624, 360], [312, 360]], "height_ft": 9}
            ]}
        }}),
    )
    .unwrap();
    s.execute("mep.read", &json!({"apply": true})).unwrap();
    s.execute("model.build", &json!({})).unwrap();
    let duct = s.doc().unwrap().model.iter().find(|e| e.common.layer == "M-DUCT-SUPP" && e.common.xdata.is_empty()).unwrap().handle;
    s.execute(
        "bim.set_spec",
        &json!({"handles": [duct.hex()], "mark": "SD-1", "create": {"category": "duct", "fields": {
            "material": {"value": "galvanized steel", "source": "Lakeside Example Inn M-001.pdf", "confidence": "stated"},
            "installer": {"value": "Fernhollow Design Group LLC", "source": "bid", "confidence": "stated"}
        }}}),
    )
    .unwrap();
    // The title block: texts on the sheet (moved there from model space).
    for t in ["Lakeside Example Inn", "4821 Bramblewood Lane, Zephyrine Hollow"] {
        s.execute("text", &json!({"at": [0, -100], "text": t, "height": 4})).unwrap();
        let d = s.doc_mut().unwrap();
        let e = d.model.iter().last().unwrap().as_ref().clone();
        d.model.remove(e.handle);
        d.layouts.first_mut().unwrap().entities.push(e);
    }
    let d = s.doc_mut().unwrap();
    d.properties.author = "Marigold Quillonby".into();
    d.properties.title = "Lakeside Example Inn".into();
    d.properties.set_custom("Client", Some("Fernhollow Design Group LLC"));
    s
}

#[test]
fn a_building_model_shares_without_its_title_block_people_or_firms() {
    hooks();
    let d = dir("cdy");
    let mut s = building();
    let src = d.join("model.cdy");
    s.execute("saveas", &json!({"path": src.display().to_string()})).unwrap();
    let page = d.join("page");
    let r = s.execute("share.export", &json!({"dir": page.display().to_string(), "title": "Sample Rooms", "source": src.display().to_string()})).unwrap();
    assert_eq!((r["rooms"].clone(), r["source"].clone(), r["stripped"].clone()), (json!(2), json!("building model"), json!(true)), "{r}");
    assert!(r.get("download").is_none(), "no download unless allowed");
    let planted = ["Marigold", "Quillonby", "Lakeside Example Inn", "Fernhollow", "Bramblewood", "Zephyrine", ".pdf"];
    assert_clean(&page, &planted);
    let (m, pkg) = load(&page);
    assert!(m.download.is_none());
    assert_eq!(pkg.rooms.iter().map(|r| (r.number.as_str(), r.name.as_str())).collect::<Vec<_>>(), [("214", "King"), ("215", "Suite")]);
    let ty = pkg.types.iter().find(|t| t.mark == "SD-1").unwrap();
    assert!(ty.fields.iter().any(|f| f.key == "material" && f.value == "galvanized steel" && f.source == "file"), "{:?}", ty.fields);
    assert!(!ty.fields.iter().any(|f| f.key == "installer"));
    assert!(pkg.elements.iter().any(|e| e.trade == "mech" && e.mark == "SD-1"));

    // Not stripped, with a download: the source file itself goes in (the publisher's choice).
    let open = d.join("open");
    let r = s
        .execute(
            "share.export",
            &json!({"dir": open.display().to_string(), "strip": false, "allow_download": true, "source": src.display().to_string()}),
        )
        .unwrap();
    assert_eq!(r["download"], "download/model.cdy");
    assert!(r.get("report").is_none());
    assert_eq!(std::fs::read(open.join(MODEL_DIR).join("download/model.cdy")).unwrap(), std::fs::read(&src).unwrap());
    let (m, pkg) = load(&open);
    assert!(!m.stripped);
    assert_eq!(pkg.project, "Shared Model");
    assert!(pkg.rooms.iter().any(|r| r.name == "Marigold Quillonby Suite"));
}

#[test]
fn hostile_share_params_are_refused() {
    hooks();
    let d = dir("hostile");
    let mut s = building();
    let page = d.join("page");
    let err = |s: &mut Session, p: Value| s.execute("share.export", &p).unwrap_err().to_string();
    assert!(err(&mut s, json!({})).contains("dir required"));
    assert!(err(&mut s, json!({"dir": "  "})).contains("dir required"));
    let inside = page.join("report.txt").display().to_string();
    assert!(err(&mut s, json!({"dir": page.display().to_string(), "report": inside})).contains("can't go in the page's folder"));
    assert!(!page.exists(), "nothing written when refused");
    let r = s
        .execute(
            "share.export",
            &json!({"dir": page.display().to_string(), "title": "x".repeat(10_000), "chunk": u64::MAX, "source": d.join("missing.ifc").display().to_string(), "allow_download": true}),
        )
        .unwrap();
    assert_eq!(r["parts"], 1);
    assert_eq!(r["download"], "download/model.field.json");
    let (m, _) = load(&page);
    assert_eq!(m.title.chars().count(), 120);
    // An empty drawing has nothing to share.
    let mut empty = Session::new();
    assert!(err(&mut empty, json!({"dir": d.join("empty").display().to_string()})).contains("model.build"));
}
