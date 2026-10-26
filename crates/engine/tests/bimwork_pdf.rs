//! Q5242 bimwork on a model built from a print: a sheet with two floor plans (3RD and 4TH
//! FLOOR) goes through our PDF exporter, `bim.from_pdf` builds it, and the takeoff, clash and
//! estimate work on what was built (levels from the plan titles, walls from the wall linework,
//! thousands of sheet objects before the built elements).

use cadaddy_engine::Session;
use cadaddy_engine::doc::geom::Vec3;
use cadaddy_engine::doc::{Circle, Common, EntityKind, Handle, Space};
use serde_json::{Value, json};

fn io() {
    use cadaddy_engine::cmd::file::{IoHooks, set_io};
    set_io(IoHooks {
        read: |b, name| cadaddy_io::read(b, name).map_err(|e| e.to_string()),
        write: |d, name| cadaddy_io::write(d, name).map_err(|e| e.to_string()),
        plot: Some(|d, space, opts| cadaddy_io::plotter::plot(d, space, opts).map_err(|e| e.to_string())),
        read_pdf: Some(cadaddy_io::pdf_read::read_json),
        read_dwf: None,
        write_dwfx: None,
    });
}

fn layer(s: &mut Session, name: &str, lw: u16) {
    if s.execute("layer.new", &json!({"name": name, "current": true, "lineweight": f64::from(lw) / 100.0})).is_err() {
        s.execute("layer.current", &json!({"name": name})).unwrap();
    }
}

fn rect(s: &mut Session, a: [f64; 2], b: [f64; 2]) {
    s.execute("pline", &json!({"vertices": [a, [b[0], a[1]], b, [a[0], b[1]]], "closed": true})).unwrap();
}

fn text(s: &mut Session, at: [f64; 2], t: &str, h: f64) {
    s.execute("text", &json!({"at": at, "text": t, "height": h})).unwrap();
}

/// One floor at `y0` (inches): a 60' × 30' plan, three rooms in 6" walls, two door swings, room
/// names and the plan's title under it.
fn floor(s: &mut Session, y0: f64, title: &str) {
    layer(s, "WALLS", 70);
    rect(s, [0.0, y0], [720.0, y0 + 360.0]);
    rect(s, [6.0, y0 + 6.0], [234.0, y0 + 354.0]);
    rect(s, [246.0, y0 + 6.0], [474.0, y0 + 354.0]);
    rect(s, [486.0, y0 + 6.0], [714.0, y0 + 354.0]);
    layer(s, "THIN", 18);
    for x in [60.0, 300.0] {
        s.execute("arc", &json!({"center": [x, y0 + 6.0], "radius": 36, "start": 0, "end": 90})).unwrap();
        s.execute("line", &json!({"points": [[x, y0 + 6.0], [x, y0 + 42.0]]})).unwrap();
    }
    layer(s, "TEXT", 25);
    text(s, [90.0, y0 + 180.0], "OFFICE", 9.0);
    text(s, [330.0, y0 + 180.0], "LOBBY", 9.0);
    text(s, [570.0, y0 + 180.0], "STORAGE", 9.0);
    text(s, [200.0, y0 - 60.0], title, 18.0);
}

/// The sheet: 3RD FLOOR PLAN under 4TH FLOOR PLAN, a scale note and a sheet number.
fn sheet() -> Session {
    let mut s = Session::new();
    floor(&mut s, 0.0, "3RD FLOOR PLAN");
    floor(&mut s, 600.0, "4TH FLOOR PLAN");
    layer(&mut s, "TEXT", 25);
    text(&mut s, [0.0, -120.0], "SCALE: 1/8\" = 1'-0\"", 6.0);
    text(&mut s, [640.0, -120.0], "A101", 12.0);
    s
}

fn tmp(name: &str) -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("cadaddy-bimwork-pdf-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// The sheet plotted to a PDF at 1/8" = 1'-0" without layers, then `bim.from_pdf` on it.
fn built_from_print(dir: &std::path::Path) -> (Session, Value) {
    io();
    let src = sheet();
    let opts =
        json!({"layers": false, "fit": false, "center": true, "scale": 0.125 / 12.0, "paper": "ANSI D", "landscape": true, "lineweights": true});
    let bytes = cadaddy_io::pdf::plot(src.doc().unwrap(), &Space::Model, &opts).unwrap();
    let pdf = dir.join("Sample Floors.pdf");
    std::fs::write(&pdf, bytes).unwrap();
    let mut s = Session::new();
    let r = s.execute("bim.from_pdf", &json!({"path": pdf.to_string_lossy(), "force": true})).unwrap();
    (s, r)
}

/// A sheet's own objects: `n` small circles off to the side, put ahead of everything in the
/// drawing (a real print's sheet has tens of thousands of objects before the built model).
fn sheet_clutter(s: &mut Session, n: usize) {
    let d = s.doc_mut().unwrap();
    for i in 0..n {
        let c = Circle { center: Vec3::new(-2000.0 - (i % 300) as f64 * 3.0, (i / 300) as f64 * 3.0, 0.0), radius: 1.0 };
        d.add(&Space::Model, Common::default(), EntityKind::Circle(c)).unwrap();
    }
    d.model.sort_by_key(|e| matches!(e.kind, EntityKind::Mesh(_)));
}

fn line<'a>(t: &'a Value, level: &str, category: &str, item: &str) -> &'a Value {
    t["lines"]
        .as_array()
        .unwrap()
        .iter()
        .find(|l| l["level"] == level && l["category"] == category && l["item"] == item)
        .unwrap_or_else(|| panic!("no {level} {category} {item} line in {}", t["csv"]))
}

#[test]
fn takeoff_and_estimate_on_a_model_built_from_a_print() {
    let dir = tmp("takeoff");
    let (mut s, built) = built_from_print(&dir);
    let levels: Vec<&str> = built["levels"].as_array().unwrap().iter().filter_map(Value::as_str).collect();
    assert!(levels.contains(&"Level 3") && levels.contains(&"Level 4"), "{built}");
    sheet_clutter(&mut s, 60_000);

    let t = s.execute("bim.takeoff", &json!({"group_by": "level"})).unwrap();
    let lines = t["lines"].as_array().unwrap();
    // Per floor: the walls the plan's wall linework built (no wall marks in the spec), with their
    // length and face area, the floor's ceilings and its rooms, each with its elements.
    for lv in ["Level 3", "Level 4"] {
        let w = line(&t, lv, "walls", "Wall");
        assert_eq!(w["unit"], "lf");
        // Three 19' × 29' rooms in a 60' × 30' box: 3 + 3 walls of 19' (north, south), 29' at
        // each end and 2 × 29' between the rooms: 10 walls, 230' (the walls' plan diagonals:
        // within 10%). Before the stroke OCR read the plotted lettering, its strokes were left as
        // linework and two more "walls" were built from the letters' parallel strokes.
        assert_eq!(w["element_count"], 10, "{w}");
        let lf = w["qty"].as_f64().unwrap();
        assert!((225.0..=253.0).contains(&lf), "{lv} wall lf {lf}");
        let a = line(&t, lv, "walls", "Wall area");
        assert!(a["qty"].as_f64().unwrap() > lf * 5.0, "{a}");
        assert!(line(&t, lv, "ceilings", "Ceiling")["element_count"].as_u64().unwrap() >= 3, "{}", t["csv"]);
        let r = line(&t, lv, "rooms", "Room");
        assert_eq!(r["qty"], 3.0, "{r}");
        assert_eq!(r["element_count"], 3, "{r}");
        // A line's elements are its level's.
        for h in w["elements"].as_array().unwrap() {
            let e = s.doc().unwrap().entity(Handle::parse_hex(h.as_str().unwrap()).unwrap()).unwrap();
            assert!(e.common.xdata.iter().any(|t| t.code == 1000 && t.value == lv), "{h} not on {lv}");
        }
    }
    let with_elements = lines.iter().filter(|l| l["element_count"].as_u64().unwrap_or(0) > 0).count();
    assert!(with_elements >= 8, "{}", t["csv"]);
    assert!(t["levels"].as_array().unwrap().len() >= 2, "{}", t["levels"]);
    // The board line the BOM gives for the whole job points at the walls (both floors' 10).
    let board = lines.iter().find(|l| l["category"] == "drywall").expect("a drywall line");
    assert!(board["element_count"].as_u64().unwrap() >= 20, "{board}");

    // Clash between trades that are not in the model: ran, nothing found.
    let c = s.execute("bim.clash", &json!({"a": "electrical", "b": "architecture"})).unwrap();
    assert_eq!(c["total"], 0, "{c}");
    assert!(c["clashes"].as_array().is_some(), "{c}");

    // A cost table with made-up rows, named as estimators name them.
    let table = dir.join("costs.csv");
    std::fs::write(
        &table,
        "item,unit,material,labor_hrs\nInterior partition metal stud,lf,4.10,0.12\nGypsum wallboard 5/8 in,sf,0.62,0.02\nAcoustical ceiling tile,sf,2.25,0.03\nDuplex receptacle,ea,6.50,0.4\n",
    )
    .unwrap();
    let e = s.execute("bim.estimate", &json!({"cost_table": table.to_string_lossy()})).unwrap();
    let total = e["total"].as_f64().unwrap();
    assert!(total > 0.0, "{e}");
    let priced: Vec<&str> = e["lines"].as_array().unwrap().iter().filter_map(|l| l["cost_item"].as_str()).collect();
    for row in ["Interior partition metal stud", "Gypsum wallboard 5/8 in", "Acoustical ceiling tile"] {
        assert!(priced.contains(&row), "{row} not used: {priced:?} {}", e["unmatched"]);
    }
    // Rooms have no row: listed, never priced at zero.
    assert!(e["unmatched"].as_array().unwrap().iter().any(|u| u["item"] == "Room"), "{}", e["unmatched"]);
    let _ = std::fs::remove_dir_all(&dir);
}
